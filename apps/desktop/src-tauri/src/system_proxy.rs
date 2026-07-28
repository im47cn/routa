use std::time::Duration;

/// Build an HTTP client that respects the macOS system proxy when configured.
pub(crate) fn build_http_client() -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder().timeout(Duration::from_secs(15));

    #[cfg(target_os = "macos")]
    let builder =
        match read_macos_system_proxy().and_then(|proxy_url| reqwest::Proxy::all(proxy_url).ok()) {
            Some(proxy) => builder.proxy(proxy),
            None => builder,
        };

    builder
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))
}

#[derive(Default)]
struct ProxySettings {
    enabled: bool,
    host: Option<String>,
    port: Option<u16>,
}

impl ProxySettings {
    fn url(&self) -> Option<String> {
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
        Some(format!("http://{authority}:{port}"))
    }
}

/// Parse the output of `scutil --proxy`, preferring HTTPS over HTTP settings.
fn parse_macos_system_proxy(output: &str) -> Option<String> {
    let mut https = ProxySettings::default();
    let mut http = ProxySettings::default();

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
            _ => {}
        }
    }

    https.url().or_else(|| http.url())
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
    use super::parse_macos_system_proxy;

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
}
