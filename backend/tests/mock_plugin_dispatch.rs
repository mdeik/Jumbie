//! Regression: `MockMetadataPlugin::handle_custom_method` previously fell
//! back to `PluginInstance::call`, which in turn calls `handle_custom_method`
//! for unrecognised methods — creating mutual infinite recursion that caused a
//! stack overflow (SIGABRT) whenever a test invoked a method the mock did not
//! explicitly handle (e.g. `mode_switch` tests).

mod common;

use jumbie::plugins::PluginInstance;
use jumbie_shared::plugin::Capability;
use std::sync::Arc;

#[tokio::test]
async fn unknown_method_returns_error_not_stack_overflow() {
    let plugin = Arc::new(common::MockMetadataPlugin {
        instance_id: "dispatch-regression".to_string(),
        display_name: "Dispatch Regression Mock".to_string(),
        capabilities: vec![Capability::MetadataProviderNormal],
        series_identifier_label: None,
        series_name: "Regression Show".to_string(),
        overview: String::new(),
        aliases: vec![],
        episodes: vec![],
    });

    // If the regression reappears this call will overflow the stack instead of
    // returning, and the test binary will abort with SIGABRT before the assert.
    let result = plugin
        .call("a_method_that_does_not_exist_in_the_mock", None)
        .await;

    assert!(
        result.is_err(),
        "expected MethodNotSupported error, got: {:?}",
        result
    );
}
