use std::future::IntoFuture;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use jumbie::api::AppState;
use jumbie::plugins::PluginManager;
use jumbie::task::TaskRegistry;

pub(crate) struct ServerConfig {
    pub router: axum::Router,
    pub state: Arc<AppState>,
    pub task_registry: TaskRegistry,
    pub plugin_manager: Arc<RwLock<PluginManager>>,
    pub shutdown_token: CancellationToken,
    pub tray_cancel: CancellationToken,
}

/// SSoT: how long to wait for in-flight requests after a shutdown signal before
/// forcing the server down. Overridable via `JUMBIE_SHUTDOWN_GRACE` (seconds).
///
/// A bound is needed because `create_series` can hold a request for up to its
/// sync deadline (~100s).
pub(crate) fn shutdown_grace() -> std::time::Duration {
    let secs = std::env::var("JUMBIE_SHUTDOWN_GRACE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    std::time::Duration::from_secs(secs)
}

/// Default API port when `JUMBIE_PORT` is unset.
pub(crate) const DEFAULT_API_PORT: u16 = 3000;

/// SSoT: parse the API port from a raw `JUMBIE_PORT` value, falling back to
/// [`DEFAULT_API_PORT`] when unset or unparseable.
pub(crate) fn parse_api_port(value: Option<&str>) -> u16 {
    value
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_API_PORT)
}

/// SSoT: resolve the API port (`JUMBIE_PORT`, default [`DEFAULT_API_PORT`]).
///
/// Used both for the server bind address and for the tray's "Open in Web" URL
/// so the two can never disagree.
pub(crate) fn api_port() -> u16 {
    parse_api_port(std::env::var("JUMBIE_PORT").ok().as_deref())
}

/// Bind and serve the API server with graceful shutdown, then perform
/// orderly cleanup of background tasks and plugin processes.
///
/// Called after the router and all background tasks have been registered.
pub(crate) async fn run(config: ServerConfig) -> anyhow::Result<()> {
    let host = "0.0.0.0";
    let port = api_port();

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Starting API server on http://{} ", addr);

    // One shared cancellation source for shutdown: the signal task (Ctrl+C /
    // SIGTERM / tray quit) cancels the token, and both the graceful-shutdown
    // hook and the grace-period race observe the same token.
    let signal_token = CancellationToken::new();
    let signal_token_for_task = signal_token.clone();
    tokio::spawn(async move {
        shutdown_signal(config.tray_cancel.clone()).await;
        signal_token_for_task.cancel();
    });

    // `with_graceful_shutdown` yields a non-`Future` builder, so convert it once
    // and pin it — the same serve future is polled in both the signal race and
    // the post-signal grace race below.
    let serve = axum::serve(
        listener,
        config
            .router
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(signal_token.clone().cancelled_owned())
    .into_future();
    tokio::pin!(serve);

    // Bound the graceful drain: after a shutdown signal, in-flight requests are
    // allowed to finish, but once the grace period elapses hyper closes the
    // connections and handler futures are dropped. Dropped queue jobs are safe —
    // transactional DB writes roll back and workers clean up their dedup keys.
    //
    // The grace timer must start only AFTER the signal; racing it at boot would
    // force the server down after `grace` seconds of uptime unconditionally.
    let grace = shutdown_grace();
    tokio::select! {
        res = serve.as_mut() => {
            res?;
        }
        _ = signal_token.cancelled() => {
            tokio::select! {
                res = serve.as_mut() => {
                    res?;
                }
                _ = tokio::time::sleep(grace) => {
                    tracing::warn!(
                        "Grace period ({:?}) elapsed before in-flight requests finished; forcing shutdown",
                        grace
                    );
                }
            }
        }
    }

    tracing::info!("API server shut down. Cleaning up background tasks...");
    config.plugin_manager.read().await.shutdown();
    config.shutdown_token.cancel();
    config.state.scan_queue.shutdown();
    config.state.metadata_queue.shutdown();
    config.state.search_queue.shutdown();
    config.task_registry.cancel();
    config.task_registry.wait_all().await;
    tracing::info!("All tasks stopped. Exiting.");

    Ok(())
}

/// Handles graceful shutdown for both user-initiated (Ctrl+C) and
/// system-initiated (SIGTERM) signals, plus system-tray quit.
///
/// SIGTERM is critical in containerized environments (Docker, systemd).
/// On non-Unix platforms, SIGTERM is a future that never resolves.
pub(crate) async fn shutdown_signal(tray_cancel: CancellationToken) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let tray = async {
        tray_cancel.cancelled().await;
    };

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received CTRL+C, starting shutdown...");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, starting shutdown...");
        },
        _ = tray => {
            tracing::info!("Received Quit from system tray, starting shutdown...");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_api_port_defaults_when_unset_or_invalid() {
        assert_eq!(parse_api_port(None), DEFAULT_API_PORT);
        assert_eq!(parse_api_port(Some("not-a-port")), DEFAULT_API_PORT);
        assert_eq!(parse_api_port(Some("")), DEFAULT_API_PORT);
    }

    #[test]
    fn parse_api_port_honours_valid_value() {
        assert_eq!(parse_api_port(Some("8080")), 8080);
        assert_eq!(parse_api_port(Some("1")), 1);
    }
}
