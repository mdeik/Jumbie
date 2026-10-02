// HTTP client factory — SSoT for all outbound HTTP. Every module that makes HTTP
// requests must use `create_client()` or `create_client_builder()`, centralising a
// consistent User-Agent (so indexers recognise the app), a 30s timeout (callers
// needing longer build their own client), and automatic proxy support.

use jumbie_shared::config::Config;
use reqwest::{Client, ClientBuilder, Proxy};
use std::time::Duration;

/// Creates a reqwest::ClientBuilder that respects the proxy settings from the configuration.
pub fn create_client_builder(config: &Config) -> ClientBuilder {
    let mut builder = ClientBuilder::new()
        .user_agent(jumbie_shared::APP_USER_AGENT)
        .timeout(Duration::from_secs(30));

    // HTTP and HTTPS proxies are configured independently (deployments often use
    // different ones). Invalid proxy URLs are silently skipped so a misconfigured
    // proxy doesn't break unrelated HTTP calls.
    if config.proxy.enabled {
        if !config.proxy.http.is_empty()
            && let Ok(proxy) = Proxy::http(&config.proxy.http)
        {
            builder = builder.proxy(proxy);
        }
        if !config.proxy.https.is_empty()
            && let Ok(proxy) = Proxy::https(&config.proxy.https)
        {
            builder = builder.proxy(proxy);
        }
    }

    builder
}

/// Creates a reqwest::Client that respects the proxy settings from the configuration.
///
/// Falls back to a default `Client::new()` if the builder fails (e.g., incompatible
/// TLS backend) so the application can still function — albeit without proxy
/// support — rather than crashing at startup over a proxy configuration issue.
pub fn create_client(config: &Config) -> Client {
    create_client_builder(config)
        .build()
        .unwrap_or_else(|_| Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_config() -> Config {
        let json = serde_json::json!({
            "database": "test.db",
            "organization": {
                "destination_roots": [],
                "collision_handling": "rename"
            }
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn test_create_client_builder_no_proxy() {
        let mut config = mock_config();
        config.proxy.enabled = false;

        let _builder = create_client_builder(&config);
    }

    #[test]
    fn test_create_client_builder_with_proxy() {
        let mut config = mock_config();
        config.proxy.enabled = true;
        config.proxy.http = "http://localhost:8080".to_string();
        config.proxy.https = "http://localhost:8081".to_string();

        let _builder = create_client_builder(&config);
    }

    #[test]
    fn test_create_client_builder_invalid_proxy_url() {
        let mut config = mock_config();
        config.proxy.enabled = true;
        config.proxy.http = "not-a-url".to_string();

        let _builder = create_client_builder(&config);
    }
}
