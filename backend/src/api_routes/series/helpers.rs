use std::collections::HashMap;

use crate::api::AppState;
use crate::error::AppError;
use crate::validation::SeriesPathClaim;

/// Fetch a series mapping by id, returning `AppError::NotFound` when it does
/// not exist.
pub async fn get_series_mapping_or_404(
    state: &AppState,
    id: &str,
) -> Result<jumbie_shared::types::MappingRule, AppError> {
    // Reject malformed IDs before hitting the DB.
    crate::validation::validate_id(id, "Series ID").map_err(|e| {
        tracing::debug!(
            "get_series_mapping_or_404: invalid series ID '{}': {}",
            id,
            e
        );
        AppError::BadRequest(e.0)
    })?;
    state
        .db
        .get_series_mapping(id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e.to_string())))?
        .ok_or_else(|| AppError::NotFound(format!("Series {} not found", id)))
}

/// If a hidden series claims the given path, delete its data and mapping so a
/// new series can be created/moved there with a fresh UUID.
///
/// Returns `true` if a hidden series was found and deleted.
pub async fn delete_hidden_series_at_path(
    state: &std::sync::Arc<AppState>,
    path: &std::path::Path,
    exclude_series_id: Option<&str>,
    all_mappings: &HashMap<String, jumbie_shared::types::MappingRule>,
    org: &jumbie_shared::config::organization::OrganizationConfig,
) -> bool {
    if let Some((old_id, _)) =
        crate::validation::find_colliding_series(path, exclude_series_id, all_mappings, org)
            .filter(|(_, m)| m.hidden_in_library)
    {
        tracing::info!(
            "Path '{}' is claimed by hidden series '{}', deleting old data",
            path.display(),
            old_id
        );
        crate::api_routes::system::organized_series::stop_series_tracking(state, old_id).await;
        return true;
    }
    false
}

/// Typed failure for folder-path resolution.
///
/// Typed rather than `String` because `create_series` needs to detect a
/// visible-series claim so it can return the claiming series id in the error
/// payload (the timeout-then-re-add flow then takes the user to the existing
/// series). Other failures have no series behind them.
pub enum FolderResolveError {
    /// A visible series owns this exact path (the claim is always rejected).
    ClaimedBySeries(SeriesPathClaim),
    /// Any other folder-resolution failure — no series behind it.
    Other(String),
}

impl std::fmt::Display for FolderResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClaimedBySeries(claim) => write!(f, "{}", claim.message),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

/// Compute the effective folder path for a series being created or moved:
/// sanitize the folder name per the illegal-char policy, reclaim hidden-series
/// paths, reject visible-series claims, then resolve folder collisions per the
/// collision config.
///
/// This is the SSoT shared by `create_series` and `update_series` — the
/// validate-path endpoint composes the same steps read-only, so the previews
/// always show the folder that will actually be created/moved to.
///
/// `resolve_collisions = false` (explicit custom paths) skips sanitization and
/// folder-collision resolution but still performs hidden reclaim + visible-claim
/// rejection.
pub async fn resolve_series_folder_path(
    state: &std::sync::Arc<AppState>,
    path: &std::path::Path,
    org: &jumbie_shared::config::organization::OrganizationConfig,
    exclude_series_id: Option<&str>,
    resolve_collisions: bool,
) -> Result<std::path::PathBuf, FolderResolveError> {
    let all_mappings = state.db.get_all_series_mappings().await.unwrap_or_default();

    // Sanitize the folder name per the illegal-char policy (root-derived names
    // only — explicit custom paths are honored verbatim).
    let sanitized = if resolve_collisions {
        crate::paths::sanitize_folder_name(path, org)
    } else {
        path.to_path_buf()
    };

    // Verbatim custom paths must already be legal on the host filesystem;
    // reject early instead of failing at mkdir time (Windows NTFS refuses `:`
    // etc. with a raw OS error that would surface as a confusing 500).
    if !resolve_collisions {
        crate::paths::check_host_legal_components(&sanitized).map_err(FolderResolveError::Other)?;
    }

    // Hidden-series reclaim: the folder is claimed as-is (no collision config).
    if delete_hidden_series_at_path(state, &sanitized, exclude_series_id, &all_mappings, org).await
    {
        return Ok(sanitized);
    }

    // Visible-series claims are always rejected regardless of config. The
    // returned claim carries the id so the caller can link the user to the
    // existing series.
    if let Some(claim) =
        crate::validation::find_series_path_claim(&sanitized, exclude_series_id, &all_mappings, org)
    {
        return Err(FolderResolveError::ClaimedBySeries(claim));
    }

    if resolve_collisions {
        crate::paths::resolve_folder_collision(&sanitized, org, |p| {
            crate::validation::find_colliding_series(p, exclude_series_id, &all_mappings, org)
                .is_some()
        })
        .map_err(FolderResolveError::Other)
    } else {
        Ok(sanitized)
    }
}
