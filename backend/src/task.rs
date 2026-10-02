use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::trace;

use tokio_util::sync::CancellationToken;

/// A simple registry to keep track of background tasks.
pub struct TaskRegistry {
    tasks: Vec<(&'static str, JoinHandle<()>)>,
    // CancellationToken integrates with tokio::select!, so a task can be
    // interrupted mid-sleep; a plain AtomicBool checked at loop boundaries would
    // delay shutdown by the full sleep duration.
    token: CancellationToken,
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskRegistry {
    /// Create a new TaskRegistry with an independent cancellation token.
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            token: CancellationToken::new(),
        }
    }

    /// Create a new TaskRegistry whose cancellation token is a child of `parent`.
    /// When the parent token is cancelled, all registered tasks will see the signal.
    /// This is useful for sharing a single shutdown token across AppState and background tasks.
    pub fn with_parent_token(parent: &CancellationToken) -> Self {
        Self {
            tasks: Vec::new(),
            token: parent.child_token(),
        }
    }

    /// Register a one-shot background task that runs exactly once and exits.
    /// Supports an optional initial delay for sequencing startup tasks.
    /// The `CancellationToken` allows the task to skip execution if shutdown
    /// is requested during the delay.
    pub fn register_once<F, Fut>(
        &mut self,
        name: &'static str,
        initial_delay: Option<Duration>,
        job: F,
    ) where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        tracing::info!("Registering one-shot background task: {}", name);
        let token = self.token.clone();
        trace!(task_name = %name, initial_delay = ?initial_delay, "Spawning one-shot task");
        let handle = tokio::spawn(async move {
            if let Some(delay) = initial_delay {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {},
                    _ = token.cancelled() => {
                        tracing::debug!("One-shot task '{}' cancelled during initial delay", name);
                        return;
                    }
                }
            }
            if token.is_cancelled() {
                tracing::debug!("One-shot task '{}' cancelled before execution", name);
                return;
            }
            job(token).await;
        });
        self.tasks.push((name, handle));
    }

    /// Register a basic recurring background task.
    /// The task function `job` receives a `CancellationToken` and must evaluate to a `Duration`, which is
    /// how long the loop will sleep before the next iteration.
    /// Returning a Duration from the job (rather than using a fixed interval) allows
    /// the task to implement adaptive polling — e.g., sleep 5 minutes when idle,
    /// but drop to 30 seconds when work is pending — without cluttering the loop logic here.
    /// The `CancellationToken` allows long-running job iterations to check for shutdown
    /// mid-operation and bail out gracefully.
    pub fn register_recurring<F, Fut>(
        &mut self,
        name: &'static str,
        // Some(delay) staggers startup so tasks don't all wake at the same instant.
        initial_delay: Option<Duration>,
        mut job: F,
    ) where
        F: FnMut(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Duration> + Send + 'static,
    {
        tracing::info!("Registering background task: {}", name);
        let token = self.token.clone();
        trace!(task_name = %name, initial_delay = ?initial_delay, "Spawning background task");
        let handle = tokio::spawn(async move {
            if let Some(delay) = initial_delay {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {},
                    _ = token.cancelled() => {
                        tracing::debug!("Background task '{}' cancelled during initial delay", name);
                        return;
                    }
                }
            }
            loop {
                if token.is_cancelled() {
                    tracing::debug!("Background task '{}' exiting due to shutdown signal", name);
                    break;
                }

                // The job returns the next sleep duration and may check the token mid-cycle.
                let sleep_duration = job(token.clone()).await;

                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {},
                    _ = token.cancelled() => {
                        tracing::debug!("Background task '{}' exiting due to shutdown signal during sleep", name);
                        break;
                    }
                }
            }
        });
        self.tasks.push((name, handle));
    }

    /// Get a reference to the cancellation token for external use (e.g. HTTP handlers).
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Cancel all registered tasks.
    pub fn cancel(&self) {
        tracing::info!(
            "Signalling shutdown to {} background tasks",
            self.tasks.len()
        );
        self.token.cancel();
    }

    /// Wait for all tasks to complete (or be aborted).
    pub async fn wait_all(mut self) {
        for (name, handle) in self.tasks.drain(..) {
            if let Err(e) = handle.await {
                tracing::debug!(
                    "Background task '{}' finished with error/abort: {}",
                    name,
                    e
                );
            }
        }
    }
}

/// Check whether a cancellation token has been signalled; if so, log `ctx` and
/// return `true`. Callers should stop processing when this returns `true`.
pub fn is_shutdown_requested(token: &CancellationToken, ctx: &str) -> bool {
    if token.is_cancelled() {
        tracing::info!("Shutdown signalled: stopping {}", ctx);
        true
    } else {
        false
    }
}

/// Compute the remaining sleep time for a fixed-interval task loop.
///
/// When a task overruns (`elapsed >= expected_interval`), sleeping the full
/// interval again would compound the drift, so sleep minimally and re-enter to
/// catch up on the next tick rather than busy-looping.
pub fn compute_sleep_duration(start: std::time::Instant, expected_interval: Duration) -> Duration {
    let elapsed = start.elapsed();
    if expected_interval > elapsed {
        let remaining = expected_interval - elapsed;
        trace!(
            ?elapsed,
            ?expected_interval,
            ?remaining,
            "Task sleeping normally"
        );
        remaining
    } else {
        trace!(
            ?elapsed,
            ?expected_interval,
            "Task overran interval, sleeping minimal duration"
        );
        // Yield/sleep minimally after overrun.
        Duration::from_millis(100)
    }
}
