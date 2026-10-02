use crate::file_manager::PlannedMove;
/// Abstracts the destination path from any plan item type, so collision detection
/// works uniformly on `PlannedMove`, rename previews, and batch operations.
use jumbie_shared::config::organization::OrganizationConfig;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub trait PlanItem {
    fn dst(&self) -> &PathBuf;
}

impl PlanItem for PlannedMove {
    fn dst(&self) -> &PathBuf {
        &self.dst
    }
}

/// Tracks destination paths claimed during a batch operation and counts collisions.
/// The in-memory `HashSet` avoids TOCTOU races within one plan evaluation: two items
/// targeting the same path are detected regardless of whether it exists on disk.
pub struct CollisionDetector {
    claimed: HashSet<PathBuf>,
    collision_count: usize,
}

impl Default for CollisionDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CollisionDetector {
    pub fn new() -> Self {
        Self {
            claimed: HashSet::new(),
            collision_count: 0,
        }
    }

    /// Two-layer collision detection: another item in the same batch already claimed
    /// `dst`, or `dst` exists and isn't one of `batch_sources` (a file at a target that
    /// this batch is itself replacing is an upgrade, not a collision).
    pub fn check(&mut self, dst: &PathBuf, batch_sources: &HashSet<PathBuf>) -> bool {
        let is_collision = if !self.claimed.insert(dst.clone()) {
            true // Duplicate in batch
        } else {
            // File exists and isn't being replaced by this batch
            dst.exists() && !batch_sources.contains(dst)
        };

        if is_collision {
            self.collision_count += 1;
        }

        is_collision
    }

    pub fn collision_count(&self) -> usize {
        self.collision_count
    }

    pub fn has_collisions(&self) -> bool {
        self.collision_count > 0
    }

    pub fn claimed_mut(&mut self) -> &mut HashSet<PathBuf> {
        &mut self.claimed
    }
}

/// Validates an entire plan without stopping at the first collision, returning the
/// total count so the caller can decide whether to abort or proceed with the rest.
pub fn detect_collisions_in_plan(
    plan: &[impl PlanItem],
    batch_sources: &HashSet<PathBuf>,
) -> usize {
    let mut detector = CollisionDetector::new();
    for item in plan {
        detector.check(item.dst(), batch_sources);
    }
    detector.collision_count()
}

/// Find the first series mapping that claims `candidate_path`, comparing normalized
/// paths so symlinks/bind mounts resolve to the same directory and template mappings
/// are compared against their sanitized folder name. `exclude_series_id` skips the
/// series' own path during an update.
pub fn find_colliding_series<'a>(
    candidate_path: &Path,
    exclude_series_id: Option<&str>,
    all_mappings: &'a HashMap<String, jumbie_shared::types::MappingRule>,
    org: &OrganizationConfig,
) -> Option<(&'a str, &'a jumbie_shared::types::MappingRule)> {
    let candidate_canonical = super::normalize_path(candidate_path)
        .to_string_lossy()
        .to_string();

    for (id, mapping) in all_mappings {
        if let Some(exclude) = exclude_series_id
            && id == exclude
        {
            continue;
        }

        // Empty paths (no settings.path) have no concrete claim.
        let resolved_path = crate::paths::mapping_path(mapping, org);
        if resolved_path.as_os_str().is_empty() {
            continue;
        }
        let existing_canonical = super::normalize_path(&resolved_path)
            .to_string_lossy()
            .to_string();

        if candidate_canonical == existing_canonical {
            return Some((id, mapping));
        }
    }

    None
}

/// User-facing message when a series path is already claimed by another series.
pub fn series_path_taken_message(
    path: &Path,
    mapping: &jumbie_shared::types::MappingRule,
) -> String {
    // WHY omit the UUID: it is internal plumbing that adds noise to user-facing
    // toasts; the conflicting series name is enough to identify it.
    format!(
        "The path '{}' is already used by series '{}'",
        path.display(),
        mapping.target_title,
    )
}

/// A path claimed by an existing series: the user-facing rejection message and the
/// claiming series id, carried separately so callers that need the id (e.g.
/// `create_series`) can use it without re-parsing the message.
pub struct SeriesPathClaim {
    pub message: String,
    pub series_id: String,
}

/// Performs the canonicalized lookup ([`find_colliding_series`]) and builds the
/// rejection message ([`series_path_taken_message`]) for callers that need the id.
pub fn find_series_path_claim(
    candidate_path: &Path,
    exclude_series_id: Option<&str>,
    all_mappings: &HashMap<String, jumbie_shared::types::MappingRule>,
    org: &OrganizationConfig,
) -> Option<SeriesPathClaim> {
    find_colliding_series(candidate_path, exclude_series_id, all_mappings, org).map(
        |(id, mapping)| SeriesPathClaim {
            message: series_path_taken_message(candidate_path, mapping),
            series_id: id.to_string(),
        },
    )
}

/// Message-only wrapper around [`find_series_path_claim`].
pub fn check_series_path_not_taken(
    candidate_path: &Path,
    exclude_series_id: Option<&str>,
    all_mappings: &HashMap<String, jumbie_shared::types::MappingRule>,
    org: &OrganizationConfig,
) -> Result<(), String> {
    match find_series_path_claim(candidate_path, exclude_series_id, all_mappings, org) {
        Some(claim) => Err(claim.message),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::types::{MappingRule, SeriesSettings};

    fn make_mapping(title: &str, path: Option<&str>) -> (String, MappingRule) {
        (
            format!("id_{}", title.to_lowercase().replace(' ', "_")),
            MappingRule {
                target_title: title.to_string(),
                settings: SeriesSettings {
                    path: path.map(|s| s.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
    }

    fn make_mappings(entries: &[(&str, Option<&str>)]) -> HashMap<String, MappingRule> {
        entries.iter().map(|(t, p)| make_mapping(t, *p)).collect()
    }

    fn org() -> OrganizationConfig {
        OrganizationConfig::default()
    }

    #[test]
    fn test_no_mappings_returns_ok() {
        let mappings = HashMap::new();
        let result = check_series_path_not_taken(Path::new("/some/path"), None, &mappings, &org());
        assert!(result.is_ok());
    }

    #[test]
    fn test_candidate_matches_existing_path() {
        let path = std::env::temp_dir().join("jumbie_test_collision");
        std::fs::create_dir_all(&path).unwrap();

        let mappings = make_mappings(&[("Existing Show", Some(path.to_str().unwrap()))]);
        let result = check_series_path_not_taken(&path, None, &mappings, &org());
        assert!(result.is_err(), "Should detect collision");
        assert!(
            result.unwrap_err().contains("Existing Show"),
            "Error should mention the conflicting series name"
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn test_no_collision_with_different_paths() {
        let path_a = std::env::temp_dir().join("jumbie_test_collision_a");
        let path_b = std::env::temp_dir().join("jumbie_test_collision_b");
        std::fs::create_dir_all(&path_a).unwrap();
        std::fs::create_dir_all(&path_b).unwrap();

        let mappings = make_mappings(&[("Series A", Some(path_a.to_str().unwrap()))]);

        let result = check_series_path_not_taken(&path_b, None, &mappings, &org());
        assert!(result.is_ok(), "Different paths should not collide");

        let _ = std::fs::remove_dir_all(&path_a);
        let _ = std::fs::remove_dir_all(&path_b);
    }

    #[test]
    fn test_exclude_series_id_skips_self() {
        let path = std::env::temp_dir().join("jumbie_test_exclude_self");
        std::fs::create_dir_all(&path).unwrap();

        let (my_id, my_mapping) = make_mapping("My Show", Some(path.to_str().unwrap()));
        let mut mappings = HashMap::new();
        mappings.insert(my_id.clone(), my_mapping);

        let result = check_series_path_not_taken(&path, Some(&my_id), &mappings, &org());
        assert!(result.is_ok(), "Excluding own ID should skip collision");

        let result = check_series_path_not_taken(&path, None, &mappings, &org());
        assert!(result.is_err(), "Without exclude, own path should collide");

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn test_exclude_different_id_still_detects_collision() {
        let path = std::env::temp_dir().join("jumbie_test_exclude_other");
        std::fs::create_dir_all(&path).unwrap();

        let (my_id, my_mapping) = make_mapping("My Show", Some(path.to_str().unwrap()));
        let (_, other_mapping) = make_mapping("Other Show", Some(path.to_str().unwrap()));
        let mut mappings = HashMap::new();
        mappings.insert(my_id.clone(), my_mapping);
        mappings.insert("some_unrelated_id".to_string(), other_mapping);

        let result = check_series_path_not_taken(&path, Some(&my_id), &mappings, &org());
        assert!(result.is_err(), "Should detect collision with other series");
        assert!(
            result.unwrap_err().contains("Other Show"),
            "Error should mention the other series"
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn test_series_without_path_does_not_cause_collision() {
        let path = std::env::temp_dir().join("jumbie_test_no_path");
        std::fs::create_dir_all(&path).unwrap();

        let (_, mapping_no_path) = make_mapping("No Path Series", None);
        let mut mappings = HashMap::new();
        mappings.insert("id_no_path".to_string(), mapping_no_path);

        // A series that stores no path cannot collide with a candidate on disk.
        let result = check_series_path_not_taken(&path, None, &mappings, &org());
        assert!(
            result.is_ok(),
            "Series without path should not cause collision"
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn test_collision_with_symlink_equivalent_path() {
        let real_dir = std::env::temp_dir().join("jumbie_test_symlink_real");
        let link_dir = std::env::temp_dir().join("jumbie_test_symlink_link");
        std::fs::create_dir_all(&real_dir).unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();

            let mappings = make_mappings(&[("Real Show", Some(real_dir.to_str().unwrap()))]);

            let result = check_series_path_not_taken(&link_dir, None, &mappings, &org());
            assert!(result.is_err(), "Symlink to same dir should collide");
            assert!(
                result.unwrap_err().contains("Real Show"),
                "Error should mention the real series"
            );

            let _ = std::fs::remove_dir_all(&link_dir);
        }

        #[cfg(not(unix))]
        {
            let _ = &link_dir;
        }

        let _ = std::fs::remove_dir_all(&real_dir);
    }

    #[test]
    fn test_multi_series_no_collision_with_different_paths() {
        let path_a = std::env::temp_dir().join("jumbie_test_multi_a");
        let path_b = std::env::temp_dir().join("jumbie_test_multi_b");
        let path_c = std::env::temp_dir().join("jumbie_test_multi_c");
        std::fs::create_dir_all(&path_a).unwrap();
        std::fs::create_dir_all(&path_b).unwrap();
        std::fs::create_dir_all(&path_c).unwrap();

        let mappings = make_mappings(&[
            ("Series A", Some(path_a.to_str().unwrap())),
            ("Series B", Some(path_b.to_str().unwrap())),
        ]);

        let result = check_series_path_not_taken(&path_c, None, &mappings, &org());
        assert!(result.is_ok(), "Third unique path should not collide");

        let _ = std::fs::remove_dir_all(&path_a);
        let _ = std::fs::remove_dir_all(&path_b);
        let _ = std::fs::remove_dir_all(&path_c);
    }

    // A mapping stored as a `${series}` template claims the SANITIZED folder name
    // (where the organizer writes); a planned create at that sanitized path must be
    // rejected, while the raw unsanitized path resolves elsewhere.
    #[test]
    fn test_template_mapping_matches_sanitized_candidate() {
        let (_, mapping) = make_mapping("Show: Part", Some("/media/tv/${series}"));
        let mut mappings = HashMap::new();
        mappings.insert("id_template".to_string(), mapping);

        let candidate = Path::new("/media/tv/Show_ Part");
        let result = check_series_path_not_taken(candidate, None, &mappings, &org());
        assert!(
            result.is_err(),
            "Template mapping should collide with the sanitized candidate path"
        );
        assert!(result.unwrap_err().contains("Show: Part"));

        let raw = Path::new("/media/tv/Show: Part");
        let result = check_series_path_not_taken(raw, None, &mappings, &org());
        assert!(
            result.is_ok(),
            "Raw (unsanitized) candidate resolves to a different folder"
        );
    }
}
