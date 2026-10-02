use jumbie_shared::types::MappingRule;

use crate::file_manager::plan::PlannedMove;

/// Compute the set of human-readable causes for a rename plan.
///
/// **SSoT:** This is the single place where rename-plan causes are determined.
/// Both the rename queue preview (`get_rename_queue`) and the reorganization
/// failure tracker (`record_reorganization_failures`) call this instead of
/// duplicating the loop-and-causes logic.
///
/// # Cause selection
///
/// - Filename changes → `"Global/Series Episode File Format"`
///   (the illegal char policy is **not** reported because files on disk are
///   already sanitized — `sanitize_with_policy` is idempotent for clean names,
///   so a policy change alone never produces new rename queue items).
/// - Folder changes → `"Global/Series Season Folder Format"`
pub fn compute_plan_causes(mapping: &MappingRule, plan: &[PlannedMove]) -> Vec<String> {
    let mut causes_set = std::collections::HashSet::new();

    for p in plan {
        if p.src.file_name() != p.dst.file_name() {
            causes_set.insert(mapping.get_rename_cause());
        }

        if p.src.parent() != p.dst.parent() {
            causes_set.insert(mapping.get_folder_rename_cause());
        }
    }

    let mut causes: Vec<String> = causes_set.into_iter().collect();
    causes.sort();
    causes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_plan(src_name: &str, dst_name: &str) -> Vec<PlannedMove> {
        vec![PlannedMove {
            src: PathBuf::from(format!("/media/Show/Season 01/{}", src_name)),
            dst: PathBuf::from(format!("/media/Show/Season 01/{}", dst_name)),
            covered_episodes: vec![],
            series_title: "Show".into(),
            series_id: "s1".into(),
            season_val: "1".into(),
            episode_title: String::new(),
            part_number: None,
            aux_kind: None,
        }]
    }

    fn default_mapping() -> MappingRule {
        MappingRule::default()
    }

    #[test]
    fn filename_change_reports_format_cause() {
        let mapping = default_mapping();
        let plan = make_plan("old_name.mkv", "new_name.mkv");

        let causes = compute_plan_causes(&mapping, &plan);
        assert!(
            causes.iter().any(|c| c.contains("Episode File Format")),
            "expected format cause, got: {:?}",
            causes
        );
    }

    #[test]
    fn folder_move_reports_folder_cause() {
        let mapping = default_mapping();
        let plan = vec![PlannedMove {
            src: PathBuf::from("/media/Show/Season 01/ep.mkv"),
            dst: PathBuf::from("/media/Show/Season 02/ep.mkv"),
            covered_episodes: vec![],
            series_title: "Show".into(),
            series_id: "s1".into(),
            season_val: "2".into(),
            episode_title: String::new(),
            part_number: None,
            aux_kind: None,
        }];

        let causes = compute_plan_causes(&mapping, &plan);
        assert!(
            causes.iter().any(|c| c.contains("Season Folder")),
            "expected folder cause, got: {:?}",
            causes
        );
    }

    #[test]
    fn identical_src_and_dst_produces_no_causes() {
        let mapping = default_mapping();
        let plan = make_plan("episode.mkv", "episode.mkv");

        let causes = compute_plan_causes(&mapping, &plan);
        assert!(
            causes.is_empty(),
            "expected no causes for identical paths, got: {:?}",
            causes
        );
    }

    #[test]
    fn filename_and_folder_change_reports_both() {
        let mapping = default_mapping();
        let plan = vec![PlannedMove {
            src: PathBuf::from("/media/Show/Season 01/old_name.mkv"),
            dst: PathBuf::from("/media/Show/Season 02/new_name.mkv"),
            covered_episodes: vec![],
            series_title: "Show".into(),
            series_id: "s1".into(),
            season_val: "2".into(),
            episode_title: String::new(),
            part_number: None,
            aux_kind: None,
        }];

        let causes = compute_plan_causes(&mapping, &plan);
        assert!(
            causes.iter().any(|c| c.contains("Episode File Format")),
            "expected format cause, got: {:?}",
            causes
        );
        assert!(
            causes.iter().any(|c| c.contains("Season Folder")),
            "expected folder cause, got: {:?}",
            causes
        );
    }
}
