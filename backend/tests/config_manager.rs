// ConfigManager integration: DB-managed config sections (organization, sources,
// general, proxy, auth, security) must be loaded from the database into the
// in-memory Config at startup.

use jumbie::config_manager::ConfigManager;
use jumbie::db::DbManager;
use jumbie_shared::config::OrganizationConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CUSTOM_ROOT: &str = "/custom/root";

/// Resolve a TOML path the same way `Config::new()` does.
///
/// On Unix, paths starting with `/` are absolute and kept as-is.
/// On Windows, paths like `/data/jumbie.db` have no drive letter so they're
/// relative — resolved against `win_exe_dir()`, unless `docker-defaults` is
/// active (which skips resolution entirely, only used in Linux containers).
fn resolve_toml_path(p: &str) -> PathBuf {
    let path = Path::new(p);
    #[cfg(all(target_os = "windows", not(feature = "docker-defaults")))]
    {
        if path.is_relative() {
            return jumbie_shared::config::win_exe_dir().join(path);
        }
    }
    path.to_path_buf()
}

#[tokio::test]
async fn test_config_manager_loads_db_organization_into_memory() {
    let dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let db_path = dir.path().join("test.db");
    let toml_path = dir.path().join("config.toml");

    let db = Arc::new(
        DbManager::new(&db_path)
            .await
            .expect("Failed to create test DB"),
    );

    db.seed_defaults().await.expect("Failed to seed defaults");

    // A value DIFFERENT from Default, to prove the DB (not Default) was read.
    let custom_org = OrganizationConfig {
        destination_roots: vec![CUSTOM_ROOT.into()],
        ..OrganizationConfig::default()
    };
    db.save_organization_config(&custom_org)
        .await
        .expect("Failed to save custom organization");

    let db_for_assert = db.clone();

    // Minimal TOML: only the 4 startup-path fields.
    std::fs::write(
        &toml_path,
        r#"
database = "/data/jumbie.db"
unknown_files_tmp_dir = "/tmp/jumbie"
plugins_dir = "/plugins"
logs_dir = "/logs"
"#,
    )
    .expect("Failed to write config.toml");

    let cm = ConfigManager::new(db, toml_path.to_str().unwrap())
        .await
        .expect("Failed to create ConfigManager");

    {
        let cfg = cm.read().await;
        assert_eq!(
            cfg.organization.destination_roots,
            vec![jumbie_shared::config::DestinationRoot::from(CUSTOM_ROOT)],
            "FAIL: organization still has Default value.\n\
                 ConfigManager should have loaded destination_roots=[\"/custom/root\"]\n\
                 from the database, but got {:?}.\n\
                 This means DB sections were NOT merged into the in-memory Config.",
            cfg.organization.destination_roots
        );
    }

    // TOML fields still come from the file, not the database.
    {
        let cfg = cm.read().await;
        assert_eq!(
            cfg.database,
            resolve_toml_path("/data/jumbie.db")
                .to_string_lossy()
                .to_string()
        );
        assert_eq!(cfg.plugins_dir, resolve_toml_path("/plugins"));
        assert_eq!(cfg.unknown_files_tmp_dir, resolve_toml_path("/tmp/jumbie"));
    }

    // persist() routes each change to its correct home (DB vs TOML).
    {
        let mut cfg = cm.write().await;
        cfg.organization
            .destination_roots
            .push("/another/root".into());
        drop(cfg); // release write lock before persist (persist acquires a read lock)
        cm.persist().await.expect("Failed to persist config");
    }

    let updated_org = db_for_assert
        .get_organization_config()
        .await
        .expect("Failed to read org after persist");
    assert_eq!(
        updated_org.destination_roots,
        vec![
            jumbie_shared::config::DestinationRoot::from(CUSTOM_ROOT),
            "/another/root".into()
        ],
        "DB should reflect the change made through write() + persist()"
    );

    // TOML file must not be written with DB data.
    let toml_content =
        std::fs::read_to_string(&toml_path).expect("Failed to read config.toml after persist");
    assert!(
        !toml_content.contains("organization"),
        "TOML file must not contain organization after persist"
    );
    assert!(
        !toml_content.contains("/custom/root"),
        "TOML file must not contain DB-only values after persist"
    );

    // save() replaces the entire config atomically.
    let fresh_cfg = {
        let r = cm.read().await;
        r.clone()
    };
    cm.save(fresh_cfg).await.expect("Failed to save config");
}
