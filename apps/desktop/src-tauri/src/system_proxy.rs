use std::time::Duration;

#[cfg(any(target_os = "macos", test))]
const PROXY_ENV_VARS: [&str; 6] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// Build an HTTP client for the small ACP registry response.
pub(crate) fn build_registry_http_client() -> Result<reqwest::Client, String> {
    build_http_client(reqwest::Client::builder().timeout(Duration::from_secs(15)))
}

/// Build an HTTP client for agent archives without imposing a total transfer deadline.
pub(crate) fn build_download_http_client() -> Result<reqwest::Client, String> {
    build_http_client(
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60)),
    )
}

/// Apply the macOS system proxy only when an environment proxy is not configured.
fn build_http_client(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, String> {
    #[cfg(target_os = "macos")]
    let builder = if environment_proxy_is_configured() {
        builder
    } else {
        match read_macos_system_proxy().and_then(|proxy_url| {
            reqwest::Proxy::all(proxy_url)
                .ok()
                .map(|proxy| proxy.no_proxy(reqwest::NoProxy::from_env()))
        }) {
            Some(proxy) => builder.proxy(proxy),
            None => builder,
        }
    };

    builder
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))
}

#[cfg(any(target_os = "macos", test))]
fn environment_proxy_is_configured() -> bool {
    environment_proxy_is_configured_with(|name| std::env::var_os(name))
}

#[cfg(any(target_os = "macos", test))]
fn environment_proxy_is_configured_with(
    mut read_env: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> bool {
    PROXY_ENV_VARS
        .iter()
        .any(|name| read_env(name).is_some_and(|value| !value.is_empty()))
}

#[derive(Default)]
struct ProxySettings {
    enabled: bool,
    host: Option<String>,
    port: Option<u16>,
}

impl ProxySettings {
    fn url(&self, scheme: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }

        let host = self.host.as_deref()?;
        let port = self.port?;
        let authority = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        Some(format!("{scheme}://{authority}:{port}"))
    }
}

/// Parse `scutil --proxy`, preferring HTTPS, then HTTP, then SOCKS settings.
fn parse_macos_system_proxy(output: &str) -> Option<String> {
    let mut https = ProxySettings::default();
    let mut http = ProxySettings::default();
    let mut socks = ProxySettings::default();

    for line in output.lines() {
        let Some((key, value)) = line.trim().split_once(" : ") else {
            continue;
        };
        let value = value.trim();

        match key.trim() {
            "HTTPSEnable" => https.enabled = value == "1",
            "HTTPSProxy" => https.host = Some(value.to_string()),
            "HTTPSPort" => https.port = value.parse().ok(),
            "HTTPEnable" => http.enabled = value == "1",
            "HTTPProxy" => http.host = Some(value.to_string()),
            "HTTPPort" => http.port = value.parse().ok(),
            "SOCKSEnable" => socks.enabled = value == "1",
            "SOCKSProxy" => socks.host = Some(value.to_string()),
            "SOCKSPort" => socks.port = value.parse().ok(),
            _ => {}
        }
    }

    https
        .url("http")
        .or_else(|| http.url("http"))
        .or_else(|| socks.url("socks5h"))
}

#[cfg(target_os = "macos")]
fn read_macos_system_proxy() -> Option<String> {
    let output = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_macos_system_proxy(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::{environment_proxy_is_configured_with, parse_macos_system_proxy};
    use std::ffi::OsString;

    #[test]
    fn prefers_enabled_https_proxy() {
        let output = r#"
<dictionary> {
  HTTPEnable : 1
  HTTPPort : 8080
  HTTPProxy : http-proxy.local
  HTTPSEnable : 1
  HTTPSPort : 6152
  HTTPSProxy : 127.0.0.1
}
"#;

        assert_eq!(
            parse_macos_system_proxy(output),
            Some("http://127.0.0.1:6152".to_string())
        );
    }

    #[test]
    fn falls_back_to_enabled_http_proxy() {
        let output = r#"
<dictionary> {
  HTTPEnable : 1
  HTTPPort : 8080
  HTTPProxy : proxy.local
  HTTPSEnable : 0
  HTTPSPort : 6152
  HTTPSProxy : 127.0.0.1
}
"#;

        assert_eq!(
            parse_macos_system_proxy(output),
            Some("http://proxy.local:8080".to_string())
        );
    }

    #[test]
    fn ignores_disabled_or_incomplete_proxy_settings() {
        let output = r#"
<dictionary> {
  HTTPEnable : 0
  HTTPPort : 8080
  HTTPProxy : proxy.local
  HTTPSEnable : 1
  HTTPSPort : invalid
}
"#;

        assert_eq!(parse_macos_system_proxy(output), None);
    }

    #[test]
    fn formats_ipv6_proxy_hosts() {
        let output = r#"
<dictionary> {
  HTTPSEnable : 1
  HTTPSPort : 6152
  HTTPSProxy : ::1
}
"#;

        assert_eq!(
            parse_macos_system_proxy(output),
            Some("http://[::1]:6152".to_string())
        );
    }

    #[test]
    fn falls_back_to_enabled_socks_proxy() {
        let output = r#"
<dictionary> {
  HTTPEnable : 0
  HTTPSEnable : 0
  SOCKSEnable : 1
  SOCKSPort : 1080
  SOCKSProxy : 127.0.0.1
}
"#;

        let proxy_url = parse_macos_system_proxy(output).expect("parse SOCKS proxy");
        assert_eq!(proxy_url, "socks5h://127.0.0.1:1080");
        reqwest::Proxy::all(proxy_url).expect("SOCKS proxy feature is enabled");
    }

    #[test]
    fn recognizes_uppercase_and_lowercase_environment_proxies() {
        for configured_name in ["ALL_PROXY", "https_proxy", "HTTP_PROXY"] {
            assert!(environment_proxy_is_configured_with(|name| {
                (name == configured_name).then(|| OsString::from("http://proxy.local:8080"))
            }));
        }

        assert!(!environment_proxy_is_configured_with(|_| None));
        assert!(!environment_proxy_is_configured_with(|_| {
            Some(OsString::new())
        }));
    }
}
