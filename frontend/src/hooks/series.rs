use crate::hooks::use_shared_state::use_shared_state;
use jumbie_shared::types::SeriesInfo;

pub fn use_series_list() -> crate::hooks::use_shared_state::SharedState<Vec<SeriesInfo>> {
    use_shared_state("fetch_series".to_string(), || crate::api::fetch_series())
}

/// Hook for fetching specific series details.
pub fn use_series_details(
    id: String,
) -> crate::hooks::use_shared_state::SharedState<jumbie_shared::types::SeriesDetails> {
    let id_clone = id.clone();
    use_shared_state(format!("fetch_series_details_{}", id), move || {
        let sid = id_clone.clone();
        async move {
            match crate::api::fetch_series_details(sid).await {
                Ok(Some(d)) => Ok(d),
                Ok(None) => Err(crate::api_client::ApiError::Network(
                    "Series details not found".to_string(),
                )),
                Err(e) => Err(e),
            }
        }
    })
}
