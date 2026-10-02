#![cfg_attr(
    all(target_os = "windows", feature = "desktop"),
    windows_subsystem = "windows"
)]
// Console-output SSoT: application logging goes through `tracing`; direct print
// macros are denied, and early/pre-`tracing` diagnostics go through
// `jumbie::logging`'s sanctioned raw writers. See the equivalent note in
// `src/lib.rs`.
#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)
)]
// macOS terminal suppression requires the binary to be in a `.app` bundle with
// `LSUIElement=true` in Contents/Info.plist; `scripts/macos-release.sh` builds it.

use clap::Parser;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

mod cli;

use cli::args::Args;
use cli::setup::{SetupOutput, run_setup};

async fn run_app(args: Args, tray_cancel: CancellationToken) -> anyhow::Result<()> {
    // Phase 0: configure allocator
    jumbie::alloc::configure();

    // Phase 1: config, logging, DB setup / maintenance
    let setup = match run_setup(&args).await? {
        Some(s) => s,
        None => return Ok(()), // maintenance command handled
    };
    let SetupOutput {
        config,
        config_path,
        logs_dir,
        log_level,
        _log_guard,
        log_buffer,
    } = setup;

    // Single connection pool, created before plugin discovery so the DB-stored
    // plugins config is available when external plugin instances are spawned.
    let db_manager =
        Arc::new(jumbie::db::DbManager::new(std::path::Path::new(&config.database)).await?);

    let plugins_dir = config.plugins_dir.clone();
    if !plugins_dir.exists()
        && let Err(e) = std::fs::create_dir_all(&plugins_dir)
    {
        tracing::warn!(
            "Failed to create plugins directory at {}: {}",
            plugins_dir.display(),
            e
        );
    }

    tracing::info!("Registering internal plugins...");
    jumbie::plugins::internal::register_all().await;

    tracing::info!(
        "Discovering plugins from {} directory...",
        plugins_dir.display()
    );
    // Enforce the single-active metadata policy on load. The write endpoints
    // already enforce it (SSoT: `ensure_single_metadata_plugin`), but a
    // hand-edited config or a legacy row could still have two providers enabled —
    // normalizing here guarantees the runtime invariant for every startup path.
    let mut plugins_cfg = db_manager.get_plugins_config().await.unwrap_or_default();
    let disabled = jumbie_shared::config::ensure_single_metadata_plugin(&mut plugins_cfg, None);
    if !disabled.is_empty() {
        tracing::error!(
            "Multiple metadata providers were enabled; keeping one and disabling [{}]",
            disabled
                .iter()
                .map(|(key, instance)| format!("{}:{}", key, instance))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Err(e) = db_manager.save_plugins_config(&plugins_cfg).await {
            tracing::error!("Failed to persist normalized plugin config: {}", e);
        }
    }
    let mut pm = jumbie::plugins::PluginManager::new(plugins_dir);
    // Security policy: opt-in signature requirement for external plugins.
    pm.set_require_signatures(config.security.require_plugin_signatures);
    if let Err(e) = pm.discover_and_start(&plugins_cfg).await {
        tracing::error!("Plugin discovery failed: {}", e);
    }
    let plugin_manager = Arc::new(tokio::sync::RwLock::new(pm));

    // Shared shutdown token
    let shutdown_token = CancellationToken::new();

    // Per-series modification lock, shared with AppState
    let modifying_series: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>> =
        Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()));

    let app = jumbie::organizer::ContentOrganizer::new(
        &config.database,
        db_manager.clone(),
        plugin_manager.clone(),
        shutdown_token.clone(),
        modifying_series,
    )
    .await?;

    tracing::info!("Starting in server mode...");

    let cfg_manager = Arc::new(
        jumbie::config_manager::ConfigManager::new(db_manager.clone(), &config_path).await?,
    );

    let (router, state) = jumbie::api::create_router(jumbie::api::RouterConfig {
        cfg: cfg_manager,
        db: db_manager.clone(),
        downloader: Some(app.downloader()),
        notifications: Some(app.notifications()),
        organizer: Some(app.clone()),
        plugin_manager: plugin_manager.clone(),
        logs_dir,
        log_level,
        log_buffer,
        shutdown_token: shutdown_token.clone(),
    })
    .await;

    let mut task_registry = jumbie::task::TaskRegistry::with_parent_token(&shutdown_token);

    cli::tasks::register_all(
        &mut task_registry,
        &state,
        &db_manager,
        &app,
        &plugin_manager,
        &shutdown_token,
    )
    .await;

    cli::server::run(cli::server::ServerConfig {
        router,
        state,
        task_registry,
        plugin_manager,
        shutdown_token,
        tray_cancel,
    })
    .await?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let tray_cancel = CancellationToken::new();

    // Capped at 4: each tokio worker creates its own jemalloc arena, so 16-32
    // CPUs would mean 16-32 arenas each caching freed pages (4+ GB RSS).
    // Override with JUMBIE_WORKER_THREADS.
    let worker_threads = std::env::var("JUMBIE_WORKER_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.max(1))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().min(4))
                .unwrap_or(4)
        });
    let blocking_threads = std::env::var("JUMBIE_BLOCKING_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.max(1))
        .unwrap_or(8);

    let build_runtime = || -> anyhow::Result<tokio::runtime::Runtime> {
        Ok(tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(worker_threads)
            .max_blocking_threads(blocking_threads)
            .build()?)
    };

    // Linux: the Wayland-native (StatusNotifierItem) tray shares the tokio
    // runtime — no event loop needs the main thread, so one `block_on` suffices.
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    {
        let rt = build_runtime()?;
        rt.block_on(async move {
            let tray = if args.no_tray {
                None
            } else {
                match jumbie::tray::TrayApp::new(cli::server::api_port(), tray_cancel.clone()).await
                {
                    Ok(tray) => Some(tray),
                    Err(e) => {
                        // Pre-`tracing`-init warning: `run_setup` (and thus the
                        // log subscriber) has not run yet.
                        jumbie::logging::pre_init_warning(format_args!(
                            "could not initialize system tray ({}); running in headless mode",
                            e
                        ));
                        None
                    }
                }
            };

            let result = run_app(args, tray_cancel).await;

            if let Some(tray) = tray {
                tray.shutdown().await;
            }
            result
        })
    }

    // Windows/macOS: OS-native tray; falls through to headless mode if it fails.
    #[cfg(all(feature = "desktop", not(target_os = "linux")))]
    {
        if !args.no_tray {
            match jumbie::tray::TrayApp::new(cli::server::api_port(), tray_cancel.clone()) {
                Ok(tray_app) => {
                    let proxy = tray_app.proxy();

                    let rt = build_runtime()?;

                    let args_clone = args.clone();
                    let tray_cancel_clone = tray_cancel.clone();

                    rt.spawn(async move {
                        if let Err(e) = run_app(args_clone, tray_cancel_clone).await {
                            tracing::error!("App error: {}", e);
                        }
                        let _ = proxy.send_event(jumbie::tray::AppEvent::AppFinished);
                    });

                    tray_app.run();
                    return Ok(());
                }
                Err(e) => {
                    // Pre-`tracing`-init warning: `run_setup` (and thus the log
                    // subscriber) has not run yet.
                    jumbie::logging::pre_init_warning(format_args!(
                        "could not initialize system tray ({}); running in headless mode",
                        e
                    ));
                }
            }
        }
    }

    // Headless fallback: no desktop feature, or (non-Linux) tray creation failed.
    #[cfg(not(all(feature = "desktop", target_os = "linux")))]
    {
        let rt = build_runtime()?;
        rt.block_on(run_app(args, tray_cancel))
    }
}
