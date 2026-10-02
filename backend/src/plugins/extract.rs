// Shared param extraction helpers for plugin method params.

use anyhow::{Context, Result};
use serde_json::Value;

/// Extract a required string parameter from an `Option<Value>` params object.
pub fn extract_str(params: &Option<Value>, key: &str) -> Result<String> {
    params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .with_context(|| format!("Missing required string param '{}'", key))
}

/// Extract an optional string parameter.
pub fn extract_str_opt(params: &Option<Value>, key: &str) -> Option<String> {
    params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract a required bool parameter.
pub fn extract_bool(params: &Option<Value>, key: &str) -> Result<bool> {
    params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get(key))
        .and_then(|v| v.as_bool())
        .with_context(|| format!("Missing required bool param '{}'", key))
}

/// Extract an optional bool parameter with a default.
pub fn extract_bool_opt(params: &Option<Value>, key: &str, default: bool) -> bool {
    params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

/// Extract a required parameter and deserialize it from JSON.
pub fn extract_json<T: serde::de::DeserializeOwned>(
    params: &Option<Value>,
    key: &str,
) -> Result<T> {
    let val = params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get(key))
        .with_context(|| format!("Missing required param '{}'", key))?;
    serde_json::from_value(val.clone())
        .with_context(|| format!("Failed to deserialize param '{}'", key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_str_found() {
        let params = Some(json!({"name": "test_value"}));
        assert_eq!(extract_str(&params, "name").unwrap(), "test_value");
    }

    #[test]
    fn test_extract_str_missing() {
        let params = Some(json!({"other": "value"}));
        assert!(extract_str(&params, "name").is_err());
    }

    #[test]
    fn test_extract_str_none_params() {
        assert!(extract_str(&None, "name").is_err());
    }

    #[test]
    fn test_extract_str_opt_found() {
        let params = Some(json!({"name": "value"}));
        assert_eq!(extract_str_opt(&params, "name"), Some("value".to_string()));
    }

    #[test]
    fn test_extract_str_opt_missing() {
        let params = Some(json!({}));
        assert_eq!(extract_str_opt(&params, "name"), None);
    }

    #[test]
    fn test_extract_bool_found() {
        let params = Some(json!({"flag": true}));
        assert!(extract_bool(&params, "flag").unwrap());
    }

    #[test]
    fn test_extract_bool_missing() {
        let params = Some(json!({}));
        assert!(extract_bool(&params, "flag").is_err());
    }

    #[test]
    fn test_extract_bool_opt_default() {
        let params = Some(json!({}));
        assert!(extract_bool_opt(&params, "flag", true));
        assert!(!extract_bool_opt(&params, "other", false));
    }

    #[test]
    fn test_extract_json_valid() {
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct Inner {
            value: i32,
        }
        let params = Some(json!({"data": {"value": 42}}));
        let result: Inner = extract_json(&params, "data").unwrap();
        assert_eq!(result, Inner { value: 42 });
    }

    #[test]
    fn test_extract_json_missing_key() {
        let params = Some(json!({"other": "value"}));
        assert!(extract_json::<serde_json::Value>(&params, "data").is_err());
    }
}
