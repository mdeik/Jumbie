//! Drift guards for the unified episode↔file ownership model.
//!
//! Ownership lives in ONE place: the `episode_files` association table. These tests
//! fail loudly if the legacy storage (`episodes.file_path`, `episode_parts`,
//! `file_paths.episode_id` / `file_paths.kind`) is ever reintroduced — whether as a
//! schema column or as a raw SQL reference anywhere in the backend sources.

use jumbie::db::DbManager;

/// The physical schema must expose exactly the normalized ownership shape.
#[tokio::test]
async fn schema_has_no_legacy_ownership_columns_or_tables() {
    let tmp = tempfile::tempdir().unwrap();
    let db = DbManager::new(&tmp.path().join("test.db")).await.unwrap();
    let pool = db.get_pool();

    let columns = |table: &str| {
        let table = table.to_string();
        async move {
            let rows: Vec<(String,)> =
                sqlx::query_as("SELECT name FROM pragma_table_info(?) ORDER BY name")
                    .bind(table)
                    .fetch_all(pool)
                    .await
                    .unwrap();
            rows.into_iter().map(|(n,)| n).collect::<Vec<_>>()
        }
    };

    let episode_cols = columns("episodes").await;
    assert!(
        !episode_cols.iter().any(|c| c == "file_path"),
        "episodes.file_path must stay dropped; ownership is `episode_files`. Columns: {episode_cols:?}"
    );

    let file_path_cols = columns("file_paths").await;
    for banned in ["episode_id", "kind"] {
        assert!(
            !file_path_cols.iter().any(|c| c == banned),
            "file_paths.{banned} must stay dropped (association carries kind). Columns: {file_path_cols:?}"
        );
    }

    let episode_files_cols = columns("episode_files").await;
    for expected in ["episode_id", "file_path_id", "kind", "part_number"] {
        assert!(
            episode_files_cols.iter().any(|c| c == expected),
            "episode_files must define `{expected}`. Columns: {episode_files_cols:?}"
        );
    }

    let legacy_table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='episode_parts'",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    assert!(
        legacy_table.is_none(),
        "the legacy `episode_parts` table must not exist"
    );
}

/// No backend source file may reference the legacy ownership storage in SQL.
///
/// This is a source-level guard: it catches a reintroduced query even when no test
/// exercises that code path. Line comments and `\`-continued lines are normalized
/// away first, so a statement split across source lines still matches.
#[test]
fn no_source_references_legacy_ownership_storage() {
    const FORBIDDEN: &[&str] = &[
        "FROM episode_parts",
        "INTO episode_parts",
        "UPDATE episode_parts",
        "episodes.file_path",
        "episodes SET file_path",
        "SELECT file_path FROM episodes",
        // `episodes.file_path` is dropped, so any bare `file_path` in an
        // `episodes` predicate (e.g. a COUNT/WHEN on the old column) is invalid.
        "file_path IS NOT NULL",
        "file_paths.episode_id",
        "file_paths SET episode_id",
        "UPDATE file_paths SET episode_id",
        "file_paths.kind",
    ];

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0usize;
    for dir in ["src", "tests"] {
        visit_rs_files(&root.join(dir), &mut |path, contents| {
            // This file lists the forbidden patterns as literals; skip it.
            if path.file_name().is_some_and(|n| n == "ownership_ssot.rs") {
                return;
            }
            checked += 1;
            let normalized = normalize_source(contents);
            for pattern in FORBIDDEN {
                assert!(
                    !normalized.contains(pattern),
                    "legacy ownership reference `{pattern}` found in {}",
                    path.display()
                );
            }
        });
    }
    assert!(
        checked > 50,
        "expected to scan the backend sources, got {checked}"
    );
}

/// The normalization must stitch `\`-continued, multi-line SQL so the patterns
/// above match a statement that spans source lines.
#[test]
fn normalize_source_stitches_multi_line_sql() {
    let snippet = "sqlx::query(\n    \"UPDATE episodes \\\n     SET file_path = ? || SUBSTR(file_path, ?) \"\n)";
    let normalized = normalize_source(snippet);
    assert!(normalized.contains("episodes SET file_path"));
    assert!(!normalized.contains("SELECT file_path FROM episodes"));
}

/// Reduce Rust source to a single-line form: drop line comments, stitch
/// `\`-continued lines, and collapse whitespace. SQL split across source lines and
/// string concatenations then reads as one string for substring matching.
fn normalize_source(contents: &str) -> String {
    let mut stitched = String::with_capacity(contents.len());
    for line in contents.lines() {
        let code = line.split("//").next().unwrap_or("");
        stitched.push_str(code.trim_end().trim_end_matches('\\'));
        stitched.push(' ');
    }
    stitched.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Filesystem deletion must stay behind the collision-strategy SSoT: the DB layer
/// performs no raw removal at all, and `delete_superseded` (which the `overwrite`
/// strategy calls) is the sole site that funnels removals through the
/// platform helper. Any second removal site would delete a file without consulting
/// `collision_handling` — the "hardcoded orphan" drift this model forbids.
#[test]
fn db_layer_removes_files_only_via_the_collision_strategy() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db");
    let mut raw_sites: Vec<String> = Vec::new();
    let mut funnel_sites: Vec<String> = Vec::new();
    visit_rs_files(&root, &mut |path, contents| {
        for (i, line) in contents.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if ["remove_file(", "remove_dir(", "remove_dir_all("]
                .iter()
                .any(|needle| code.contains(needle))
            {
                raw_sites.push(format!("{}:{}", path.display(), i + 1));
            }
            if code.contains("remove_file_tolerating_locks(") {
                funnel_sites.push(format!("{}:{}", path.display(), i + 1));
            }
        }
    });
    assert!(
        raw_sites.is_empty(),
        "the DB layer must not remove files directly; use `delete_superseded`; found {raw_sites:?}"
    );
    assert_eq!(
        funnel_sites.len(),
        1,
        "the DB layer must remove files only through `delete_superseded`; found {funnel_sites:?}"
    );
    assert!(
        funnel_sites[0].contains("ownership.rs"),
        "the single removal site must be `ownership.rs`; found {funnel_sites:?}"
    );
}

fn visit_rs_files(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_rs_files(&path, f);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let contents = std::fs::read_to_string(&path).unwrap_or_default();
            f(&path, &contents);
        }
    }
}
