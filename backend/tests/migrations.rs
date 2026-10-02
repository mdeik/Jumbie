//! Dynamic coverage for the on-disk migration chain.
//!
//! The migration files are read from `migrations/` at run time and applied in
//! version order to a fresh database with production's connection settings
//! (foreign keys ON). A newly added migration is therefore exercised automatically,
//! with no per-migration test to write or keep in sync.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::{Path, PathBuf};

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

/// Migration files in apply order. The zero-padded numeric prefix makes a plain
/// lexicographic sort the version order.
fn migration_files() -> Vec<(i64, PathBuf)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(migrations_dir())
        .expect("read migrations directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let version = stem
                .split('_')
                .next()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or_else(|| {
                    panic!("migration file must start with a numeric version: {path:?}")
                });
            (version, path)
        })
        .collect()
}

/// A pooled SQLite connection using the same options as production, without
/// running any migration (that is the subject under test).
async fn fresh_pool(path: &Path) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open database")
}

#[tokio::test]
async fn every_migration_applies_in_order_on_a_fresh_database() {
    let files = migration_files();
    assert!(
        !files.is_empty(),
        "no migration files found in {}",
        migrations_dir().display()
    );

    // Versions must be unique and strictly increasing: a duplicate or an
    // out-of-order name would leave the apply order ambiguous.
    let versions: Vec<i64> = files.iter().map(|(version, _)| *version).collect();
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        versions, sorted,
        "migration versions must be unique and in ascending order"
    );

    let tmp = tempfile::tempdir().unwrap();
    let pool = fresh_pool(&tmp.path().join("test.db")).await;

    for (version, path) in &files {
        let sql = std::fs::read_to_string(path).unwrap();

        // Mirror the migrator: each file runs in its own transaction, so a failure
        // leaves the database untouched rather than half-migrated.
        let mut tx = pool.begin().await.unwrap();
        if let Err(e) = sqlx::raw_sql(&sql).execute(&mut *tx).await {
            panic!("migration {version} ({}) failed: {e}", path.display());
        }
        tx.commit().await.unwrap();

        let violations: Vec<(String, i64, String, i64)> =
            sqlx::query_as("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            violations.is_empty(),
            "foreign key violations after migration {version}: {violations:?}"
        );
    }

    // The chain must end in the normalized ownership schema.
    let legacy_table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='episode_parts'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(
        legacy_table.is_none(),
        "`episode_parts` must be dropped by the chain"
    );
}

#[tokio::test]
async fn applied_migrations_match_the_directory() {
    let expected: Vec<i64> = migration_files()
        .into_iter()
        .map(|(version, _)| version)
        .collect();

    let tmp = tempfile::tempdir().unwrap();
    let db = jumbie::db::DbManager::new(&tmp.path().join("test.db"))
        .await
        .unwrap();
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(db.get_pool())
            .await
            .unwrap();

    assert_eq!(
        applied, expected,
        "every on-disk migration must be applied exactly once, in version order"
    );
}
