// Download orchestration: fetching links, organizing files, handling subtitles, and
// matching orphan files — split into submodules so each concern can be tested
// independently.

pub mod automatic;
pub mod download;
pub mod organize;
pub mod smart_link;

/// Which preference governs a redundant (already-present) file left by a pack or
/// series-level download.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedundantHandling {
    /// Treated as an unmatched file (`unexpected_files_handling`).
    Unmatched,
    /// Treated as an unneeded episode (`unneeded_episodes_handling`).
    Unneeded,
}

/// A redundant file counts as "unmatched" (kept for review) only for a
/// **series-level** release the user explicitly picked; every episode-level
/// download — manual or automatic — keeps the "unneeded episode" semantics, so an
/// episode search and an episode auto-search route extras identically.
///
/// SSoT for the force-keep routing rule — used by `smart_link` (which list
/// the file joins) and `organize` (which preference applies).
pub(crate) fn redundant_file_handling(force_keep_review: bool) -> RedundantHandling {
    if force_keep_review {
        RedundantHandling::Unmatched
    } else {
        RedundantHandling::Unneeded
    }
}

impl RedundantHandling {
    /// The keep/delete preference string for this handling.
    pub(crate) fn keep_setting(self, cfg: &jumbie_shared::config::GeneralConfig) -> &str {
        match self {
            RedundantHandling::Unmatched => &cfg.unexpected_files_handling,
            RedundantHandling::Unneeded => &cfg.unneeded_episodes_handling,
        }
    }

    /// The review reason recorded for a file kept under this handling.
    pub(crate) fn review_reason(self) -> ReviewReason {
        match self {
            RedundantHandling::Unmatched => ReviewReason::Unmatched,
            RedundantHandling::Unneeded => ReviewReason::Unneeded,
        }
    }
}

/// Why a file was kept for manual review.
#[derive(Clone, Copy)]
pub(crate) enum ReviewReason {
    /// Pack overspill that wasn't needed (automatic download).
    Unneeded,
    /// A file that couldn't be assigned, or a redundant file from a series-level
    /// release the user picked.
    Unmatched,
}

impl ReviewReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ReviewReason::Unneeded => "unneeded",
            ReviewReason::Unmatched => "unmatched",
        }
    }
}

/// SSoT for "keep this file in the download directory for manual review": records
/// it durably, scoped by series (see `db::unmatched_files`). That table also
/// gates orphan adoption (`get_orphan_files` excludes recorded paths), so keeping
/// a file out of auto-adoption and showing it in Manage Series Files never drift
/// apart. Every keep-for-review site goes through here.
pub(crate) async fn mark_file_for_review(
    db: &crate::db::DbManager,
    file_path: &str,
    series_id: Option<&str>,
    reason: ReviewReason,
) {
    if let Err(e) = db
        .upsert_unmatched_file(file_path, series_id, reason.as_str())
        .await
    {
        // Review state is behavioural here (it also blocks orphan adoption), so a
        // failed write is worth surfacing rather than silently dropping.
        tracing::warn!(
            "Failed to record kept-for-review file '{}' ({}): {}",
            file_path,
            reason.as_str(),
            e
        );
    }
}
