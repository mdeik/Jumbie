use crate::models::media::MediaEntry;
use crate::plugins::PluginInstance;
use crate::plugins::extract::extract_str;

use anyhow::Result;
use async_trait::async_trait;
use jumbie_shared::validation::plugin_data::validate_and_filter;
use plugin_sdk::query::SearchResponse;

/// A minimal placeholder source that fetches raw RSS content without parsing.
///
/// Represents the *idea* of an RSS source (fetch the URL) rather than a parser —
/// `fetch_entries` discards the content. It exists mainly as a documentation
/// placeholder and for runtime type identification (the name "Generic Feed" appears
/// in logs and the UI). Actual RSS→MediaEntry parsing is done by `BasicRssSource` in
/// `rss.rs`.
pub struct GenericSource {
    instance_id: String,
    priority: i32,
    pub url: String,
}

/// Independent version of the built-in Generic Feed plugin.
/// Bump when this plugin's behavior changes — it versions the plugin, not the app.
pub const PLUGIN_VERSION: &str = "1.0.0";

impl GenericSource {
    pub fn new(url: String, instance_id: String, priority: i32) -> Self {
        Self {
            instance_id,
            priority,
            url,
        }
    }

    pub fn name(&self) -> &str {
        "Generic Feed"
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn plugin_info() -> jumbie_shared::plugin::PluginTypeInfo {
        jumbie_shared::plugin::PluginTypeInfo {
            display_name: "Generic Feed".to_string(),
            version: PLUGIN_VERSION.to_string(),
            author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
            description: "Generic Feed Source".to_string(),
            capabilities: vec![jumbie_shared::plugin::Capability::FeedProvider],
            supported_protocols: None,
            series_identifier_label: None,
            series_identifier_placeholder: None,
            rate_limit: None,
            supports_test: false,
        }
    }
}

#[async_trait]
impl PluginInstance for GenericSource {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn plugin_info(&self) -> jumbie_shared::plugin::PluginTypeInfo {
        Self::plugin_info()
    }

    async fn handle_custom_method(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        match method {
            "search" => {
                let query = extract_str(&params, "query")?;
                let entries = self.search(query.clone()).await?;
                let (clean, rejected) = validate_and_filter(entries);
                if !rejected.is_empty() {
                    tracing::debug!(
                        "GenericSource search returned {} invalid entries",
                        rejected.len()
                    );
                }
                Ok(serde_json::to_value(SearchResponse::from_manual(
                    clean, query,
                ))?)
            }
            "fetch_entries" => {
                let entries = self.fetch_entries().await?;
                let (clean, rejected) = validate_and_filter(entries);
                if !rejected.is_empty() {
                    tracing::debug!(
                        "GenericSource fetch_entries returned {} invalid entries",
                        rejected.len()
                    );
                }
                Ok(serde_json::to_value(clean)?)
            }
            _ => PluginInstance::call(self, method, params).await,
        }
    }
}

impl GenericSource {
    async fn search(&self, _query: String) -> Result<Vec<MediaEntry>> {
        Ok(vec![])
    }

    async fn fetch_entries(&self) -> Result<Vec<MediaEntry>> {
        Ok(vec![])
    }
}
