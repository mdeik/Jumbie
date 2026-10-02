use super::*;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::RwLock;

jumbie_shared::test_module! {
    struct MockDownloader {
        id: String,
        should_fail: bool,
        calls: Arc<Mutex<Vec<String>>>,
        protocols: Option<Vec<String>>,
        priority: i32,
    }

    #[async_trait]
    impl PluginInstance for MockDownloader {
        fn instance_id(&self) -> &str {
            &self.id
        }

        fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
            plugin_sdk::traits::PluginTypeInfo {
                display_name: "MockDownloader".to_string(),
                version: "1.0.0".to_string(),
                author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
                description: "Mock downloader for tests".to_string(),
                capabilities: vec![jumbie_shared::plugin::Capability::Downloader],
                supported_protocols: None,
                series_identifier_label: None,
                series_identifier_placeholder: None,
                rate_limit: None,
                supports_test: false,
            }
        }

        fn supported_protocols(&self) -> Option<&[String]> {
            self.protocols.as_deref()
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        async fn call(&self, method: &str, _params: Option<Value>) -> Result<Value> {
            self.calls.lock().unwrap().push(format!("{}:{}", self.id, method));
            if self.should_fail {
                anyhow::bail!("Mock failure");
            }
            match method {
                "add_download" => Ok(Value::Null),
                "get_completed_downloads" => Ok(serde_json::json!(Vec::<String>::new())),
                "get_download_progress" => Ok(serde_json::json!(Some(0.5f32))),
                "complete_download" => Ok(Value::Null),
                "test_connection" => Ok(Value::Null),
                "get_download_path" => Ok(serde_json::json!(Some("/tmp/downloads"))),
                "get_organizer_path" => Ok(serde_json::json!(Some("/tmp/organizer"))),
                _ => anyhow::bail!("Unsupported method"),
            }
        }
    }

    fn setup_manager(plugins: Vec<Arc<dyn PluginInstance>>) -> DownloadManager {
        let mut pm = PluginManager::new(std::env::temp_dir().join("jb_tests"));
        for p in plugins {
            pm.add_internal_plugin(p);
        }
        DownloadManager::new(Arc::new(RwLock::new(pm)))
    }

    #[tokio::test]
    async fn test_fallback_logic() {
        let calls = Arc::new(Mutex::new(Vec::new()));

        let d1 = Arc::new(MockDownloader {
            id: "d1".into(),
            should_fail: true,
            calls: calls.clone(),
            protocols: None,
            priority: 0,
        });
        let d2 = Arc::new(MockDownloader {
            id: "d2".into(),
            should_fail: false,
            calls: calls.clone(),
            protocols: None,
            priority: 0,
        });

        let manager = setup_manager(vec![d1, d2]);
        let res = manager.add_download("magnet:?", "Series", None, None).await;

        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "d2");

        let calls_vec = calls.lock().unwrap();
        assert_eq!(calls_vec.len(), 2);
        assert!(calls_vec.contains(&"d1:add_download".to_string()));
        assert!(calls_vec.contains(&"d2:add_download".to_string()));
    }

    #[tokio::test]
    async fn test_first_success() {
        let calls = Arc::new(Mutex::new(Vec::new()));

        let d1 = Arc::new(MockDownloader {
            id: "d1".into(),
            should_fail: false,
            calls: calls.clone(),
            protocols: None,
            priority: 0,
        });
        let d2 = Arc::new(MockDownloader {
            id: "d2".into(),
            should_fail: true,
            calls: calls.clone(),
            protocols: None,
            priority: 0,
        });

        let manager = setup_manager(vec![d1, d2]);
        let res = manager.add_download("magnet:?", "Series", None, None).await;

        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "d1");

        let calls_vec = calls.lock().unwrap();
        assert_eq!(calls_vec.len(), 1);
        assert_eq!(calls_vec[0], "d1:add_download");
    }

    #[tokio::test]
    async fn test_get_download_paths() {
        let d1 = Arc::new(MockDownloader {
            id: "d1".into(),
            should_fail: false,
            calls: Arc::new(Mutex::new(Vec::new())),
            protocols: None,
            priority: 0,
        });
        let manager = setup_manager(vec![d1]);
        let paths = manager.get_download_paths().await;
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], std::path::PathBuf::from("/tmp/downloads"));
    }

    #[tokio::test]
    async fn test_no_client_mode() {
        let manager = DownloadManager::no_client();
        assert!(manager.is_no_client().await);

        let res = manager.add_download("magnet:?", "Series", None, None).await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "");
    }

    #[tokio::test]
    async fn test_protocol_filtering() {
        let calls = Arc::new(Mutex::new(Vec::new()));

        let d_magnet = Arc::new(MockDownloader {
            id: "magnet_only".into(),
            should_fail: false,
            calls: calls.clone(),
            protocols: Some(vec!["magnet:*".into()]),
            priority: 0,
        });
        let d_http = Arc::new(MockDownloader {
            id: "http_only".into(),
            should_fail: false,
            calls: calls.clone(),
            protocols: Some(vec!["http://*".into()]),
            priority: 0,
        });

        let manager = setup_manager(vec![d_magnet, d_http]);

        // Attempt magnet download -> should only hit magnet_only
        let res = manager.add_download("magnet:?xt=...", "Series", None, None).await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "magnet_only");

        // Attempt http download -> should only hit http_only
        let res = manager.add_download("http://example.com/file.torrent", "Series", None, None).await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "http_only");

        // Attempt unknown protocol (e.g. ftp) -> should fail as no downloader supports it
        let res = manager.add_download("ftp://example.com/file", "Series", None, None).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_complete_download() {
        let calls = Arc::new(Mutex::new(Vec::new()));

        let d1 = Arc::new(MockDownloader {
            id: "completedl".into(),
            should_fail: false,
            calls: calls.clone(),
            protocols: None,
            priority: 0,
        });

        let manager = setup_manager(vec![d1]);
        let res = manager.complete_download("some_hash", "completedl").await;

        assert!(res.is_ok());

        let calls_vec = calls.lock().unwrap();
        assert_eq!(calls_vec.len(), 1);
        assert_eq!(calls_vec[0], "completedl:complete_download");
    }

    #[tokio::test]
    async fn test_complete_download_unknown_client() {
        let d1 = Arc::new(MockDownloader {
            id: "completedl".into(),
            should_fail: false,
            calls: Arc::new(Mutex::new(Vec::new())),
            protocols: None,
            priority: 0,
        });

        let manager = setup_manager(vec![d1]);
        let res = manager.complete_download("some_hash", "nonexistent").await;

        assert!(res.is_err());
    }

    /// A downloader that implements only `get_download_failure`.
    struct FailureDownloader {
        id: String,
        reason: Option<String>,
    }

    #[async_trait]
    impl PluginInstance for FailureDownloader {
        fn instance_id(&self) -> &str {
            &self.id
        }

        fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
            plugin_sdk::traits::PluginTypeInfo {
                display_name: "FailureDownloader".to_string(),
                version: "1.0.0".to_string(),
                author: jumbie_shared::plugin::JUMBIE_AUTHOR.to_string(),
                description: "Mock failure downloader for tests".to_string(),
                capabilities: vec![jumbie_shared::plugin::Capability::Downloader],
                supported_protocols: None,
                series_identifier_label: None,
                series_identifier_placeholder: None,
                rate_limit: None,
                supports_test: false,
            }
        }

        fn supported_protocols(&self) -> Option<&[String]> {
            None
        }

        fn priority(&self) -> i32 {
            0
        }

        async fn call(&self, method: &str, _params: Option<Value>) -> Result<Value> {
            match method {
                "get_download_failure" => Ok(serde_json::json!(self.reason)),
                _ => anyhow::bail!("Unsupported method"),
            }
        }
    }

    #[tokio::test]
    async fn test_get_download_failure_returns_first_reason() {
        // First plugin doesn't implement the method (skipped); second reports one.
        let unsupported = Arc::new(MockDownloader {
            id: "plain".into(),
            should_fail: false,
            calls: Arc::new(Mutex::new(Vec::new())),
            protocols: None,
            priority: 0,
        });
        let failing = Arc::new(FailureDownloader {
            id: "failing".into(),
            reason: Some("torrent errored".into()),
        });

        let manager = setup_manager(vec![unsupported, failing]);
        assert_eq!(
            manager.get_download_failure("abc").await,
            Some("torrent errored".to_string())
        );
    }

    #[tokio::test]
    async fn test_get_download_failure_none_when_unreported() {
        let unreported = Arc::new(FailureDownloader {
            id: "quiet".into(),
            reason: None,
        });
        let manager = setup_manager(vec![unreported]);
        assert_eq!(manager.get_download_failure("abc").await, None);
    }
}
