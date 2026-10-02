// Plugin Type Host
//
// ONE process per plugin TYPE. All instances of a type live inside that process,
// routed by `instance_id` on every JSON-RPC request. Adding, removing, or
// reconfiguring an instance is an RPC — never a process spawn — so instance
// lifecycle is cheap and the process stays warm for the life of the type.
//
// Queue-based IPC (nothing is dropped): requests go through an UNBOUNDED mpsc
// queue owned by the lifecycle task, so a slow or hung instance can never cause
// a request to be dropped — callers wait (or time out after 30s). The queue
// survives process restarts: on respawn the lifecycle task first REPLAYS
// `initialize` for every live instance (configs cached in `TypeProcessState`),
// then drains queued requests into the fresh process.
//
// Backpressure: the unbounded queue IS the backpressure mechanism — per-call 30s
// timeouts bound how long a caller waits, and the per-instance `PolicyPlugin`
// wrapper (rate limiter + failure cooldowns) throttles request generation. There
// is deliberately NO semaphore that would refuse requests when the process is
// slow, since that would drop them, which is what the queue exists to prevent.
//
// IPC authentication: the host generates a random 256-bit secret at spawn time,
// set as `JUMBIE_PLUGIN_SECRET` in the child's environment AND stored in the
// host's `auth_secret`. Every outgoing JSON-RPC request carries `"auth"` with the
// secret, which the plugin SDK validates on every call.
//
// Timeouts: every RPC call has a 30-second timeout; the process health probe has
// a separate 10-second timeout so a slow-but-alive plugin doesn't trigger
// false-positive restarts.

use super::{PluginCallError, PluginInstance};
use anyhow::Result;
use async_trait::async_trait;
use base64::Engine as _;
use plugin_sdk::{JsonRpcRequest, JsonRpcResponse};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, trace, warn};

/// One process per plugin type. Holds the request queue and per-instance state;
/// the lifecycle task owns the child process.
pub struct PluginTypeHost {
    /// Canonical TYPE id (derived_id format, e.g. "test.restart_plugin").
    canonical_id: String,
    display_name: String,
    executable_path: PathBuf,
    /// Type-level PluginTypeInfo from the discovery probe / get_info.
    cached_info: std::sync::Mutex<Option<plugin_sdk::traits::PluginTypeInfo>>,
    /// Process health as of the last lifecycle probe (in-process read for the
    /// status page — no RPC needed). True once the process passes the startup
    /// handshake; false on death/shutdown or after missed health checks.
    healthy: Arc<AtomicBool>,
    /// Unbounded queue — requests are never dropped. The lifecycle task owns
    /// the receiver and survives process restarts.
    request_tx: mpsc::UnboundedSender<HostRequest>,
    /// Per-type process statistics (in-process reads — no RPC).
    stats: Arc<PluginTypeStats>,
    next_id: AtomicU64,
    auth_secret: Option<String>,
    shutdown_token: CancellationToken,
    /// Shared with the lifecycle task: configs of live instances, replayed
    /// after a process restart.
    process_state: Arc<TypeProcessState>,
    /// Shared with the lifecycle task: in-flight request routing (id → oneshot).
    /// Stored here so callers can drop stale entries when they time out (P1).
    state: Arc<HostState>,
}

/// Shared state between the type host (writer side) and its lifecycle task.
struct TypeProcessState {
    /// instance_id → config, for `set_config` replay after a crash/restart.
    instance_configs: Mutex<HashMap<String, Value>>,
}

/// Per-type process statistics (in-process reads — no RPC).
#[derive(Debug, Default)]
pub struct PluginTypeStats {
    /// Total RPC calls routed to the process.
    pub calls: AtomicU64,
    /// Failed RPC calls (timeout / error / process-dead).
    pub errors: AtomicU64,
    /// Process spawn/restart count.
    pub restarts: AtomicU64,
    /// Last observed request-queue depth (unbounded — visibility for hangs).
    pub queue_depth: AtomicU64,
}

impl PluginTypeStats {
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.calls.load(Ordering::SeqCst),
            self.errors.load(Ordering::SeqCst),
            self.restarts.load(Ordering::SeqCst),
            self.queue_depth.load(Ordering::SeqCst),
        )
    }
}

struct HostRequest {
    req: JsonRpcRequest,
    response_tx: oneshot::Sender<Result<Value>>,
}

// Shared state between the type host and the response reader task.
struct HostState {
    pending_requests: Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>,
}

impl PluginTypeHost {
    /// Spawn a plugin process for a type and start its lifecycle manager.
    pub async fn spawn(
        canonical_id: &str,
        display_name: &str,
        executable_path: &Path,
        shutdown_token: CancellationToken,
    ) -> Result<Arc<Self>> {
        // AUTH IS FAIL-CLOSED: without a per-spawn secret there is no
        // authenticated IPC, so the spawn fails rather than running
        // unauthenticated.
        let auth_secret = Self::generate_auth_secret().ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to generate IPC auth secret for plugin type '{}'",
                canonical_id
            )
        })?;
        let auth_secret = Some(auth_secret);

        let (request_tx, request_rx) = mpsc::unbounded_channel::<HostRequest>();
        let state = Arc::new(HostState {
            pending_requests: Mutex::new(HashMap::new()),
        });
        let state_clone = state.clone();
        let process_state = Arc::new(TypeProcessState {
            instance_configs: Mutex::new(HashMap::new()),
        });
        let healthy = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(PluginTypeStats::default());

        let host = Arc::new(Self {
            canonical_id: canonical_id.to_string(),
            display_name: display_name.to_string(),
            executable_path: executable_path.to_path_buf(),
            cached_info: std::sync::Mutex::new(None),
            healthy: healthy.clone(),
            request_tx,
            stats: stats.clone(),
            next_id: AtomicU64::new(1),
            auth_secret,
            shutdown_token,
            process_state,
            state,
        });

        let auth_secret_clone = host.auth_secret.clone();
        let process_state_clone = host.process_state.clone();
        let canonical_id_clone = host.canonical_id.clone();
        let executable_clone = host.executable_path.clone();
        let shutdown_clone = host.shutdown_token.clone();
        let healthy_clone = healthy.clone();
        let stats_clone = stats.clone();

        tokio::spawn(async move {
            manage_plugin_lifecycle(PluginLifecycle {
                canonical_id: canonical_id_clone,
                executable_path: executable_clone,
                request_rx,
                state: state_clone,
                process_state: process_state_clone,
                shutdown_token: shutdown_clone,
                auth_secret: auth_secret_clone,
                healthy: healthy_clone,
                stats: stats_clone,
            })
            .await;
        });

        Ok(host)
    }

    pub fn canonical_id(&self) -> &str {
        &self.canonical_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Process health as of the last lifecycle probe (in-process read — no RPC).
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    /// Per-type process statistics (in-process read — no RPC).
    pub fn stats(&self) -> &PluginTypeStats {
        &self.stats
    }

    /// Store the type-level PluginTypeInfo (from the discovery probe / get_info).
    pub fn set_cached_info(&self, info: plugin_sdk::traits::PluginTypeInfo) {
        *self.cached_info.lock().unwrap() = Some(info);
    }

    pub fn cached_info(&self) -> Option<plugin_sdk::traits::PluginTypeInfo> {
        self.cached_info.lock().unwrap().clone()
    }

    /// Send one RPC into the process queue. `instance_id: None` = type-level.
    async fn rpc(&self, instance_id: Option<&str>, method: &str, params: Value) -> Result<Value> {
        let span = tracing::info_span!(
            "plugin_call",
            plugin = %self.canonical_id,
            instance = ?instance_id,
            method = %method
        );

        async move {
            let started = std::time::Instant::now();
            let outcome: Result<Value> = async {
                let req_id = self.next_id.fetch_add(1, Ordering::SeqCst);
                let mut req = JsonRpcRequest::new(
                    method,
                    params,
                    Some(json!(req_id)),
                    instance_id.map(String::from),
                );
                if let Some(ref secret) = self.auth_secret {
                    req.auth = Some(secret.clone());
                }

                let (response_tx, response_rx) = oneshot::channel();
                let host_req = HostRequest { req, response_tx };
                // Unbounded queue: this send never fails due to a slow instance.
                if self.request_tx.send(host_req).is_err() {
                    anyhow::bail!(PluginCallError::ProcessDead);
                }

                match tokio::time::timeout(Duration::from_secs(30), response_rx).await {
                    Ok(Ok(res)) => res,
                    Ok(Err(e)) => Err(e.into()),
                    Err(_) => {
                        // Drop the stale pending entry so a late or never-arriving
                        // response doesn't accumulate until process death (P1).
                        drop_pending(&self.state, req_id).await;
                        Err(anyhow::Error::new(PluginCallError::Timeout))
                    }
                }
            }
            .await;

            // Per-call completion event inside the `plugin_call` span, giving
            // every request an instance-attributed, duration-tagged log line.
            self.stats.calls.fetch_add(1, Ordering::SeqCst);
            match &outcome {
                Ok(_) => tracing::debug!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "plugin call ok"
                ),
                Err(e) => {
                    self.stats.errors.fetch_add(1, Ordering::SeqCst);
                    tracing::warn!(
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        error = %e,
                        "plugin call failed"
                    );
                }
            }
            outcome
        }
        .instrument(span)
        .await
    }

    /// Type-level `get_info` (discovery / cached metadata refresh).
    pub async fn get_info(&self) -> Result<Value> {
        self.rpc(None, "get_info", Value::Null).await
    }

    /// Set (create or update) one instance's config inside the shared process.
    /// Idempotent — config is input, so there is no separate "reconfigure"
    /// RPC. On success the config is cached for replay after a process restart.
    pub async fn set_config(&self, instance_id: &str, config: Value) -> Result<()> {
        self.rpc(Some(instance_id), "set_config", config.clone())
            .await?;
        self.process_state
            .instance_configs
            .lock()
            .await
            .insert(instance_id.to_string(), config);
        Ok(())
    }

    /// Remove one instance from the shared process.
    pub async fn shutdown_instance(&self, instance_id: &str) -> Result<()> {
        let res = self
            .rpc(Some(instance_id), "shutdown_instance", Value::Null)
            .await;
        self.process_state
            .instance_configs
            .lock()
            .await
            .remove(instance_id);
        res.map(|_| ())
    }

    fn generate_auth_secret() -> Option<String> {
        use ring::rand::SecureRandom;
        let rng = ring::rand::SystemRandom::new();
        let mut secret = [0u8; 32];
        rng.fill(&mut secret).ok()?;
        Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret))
    }
}

/// A handle to one instance living inside a type's shared process. Implements
/// `PluginInstance` so the manager treats external instances exactly like
/// internal ones; every call is routed by `instance_id` into the type process.
///
/// Host-managed fields (priority / enabled / refresh_interval) live on the
/// `PolicyPlugin` wrapper, so this handle reports neutral defaults.
pub struct InstanceHandle {
    instance_id: String,
    host: Arc<PluginTypeHost>,
}

impl InstanceHandle {
    pub fn new(instance_id: String, host: Arc<PluginTypeHost>) -> Self {
        Self { instance_id, host }
    }

    pub fn type_host(&self) -> &Arc<PluginTypeHost> {
        &self.host
    }
}

#[async_trait]
impl PluginInstance for InstanceHandle {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn is_healthy(&self) -> bool {
        self.host.is_healthy()
    }

    fn plugin_info(&self) -> plugin_sdk::traits::PluginTypeInfo {
        self.host
            .cached_info()
            .unwrap_or_else(|| plugin_sdk::traits::PluginTypeInfo {
                display_name: self.host.display_name.clone(),
                version: "unknown".to_string(),
                author: "external".to_string(),
                description: String::new(),
                capabilities: vec![],
                supported_protocols: None,
                series_identifier_label: None,
                series_identifier_placeholder: None,
                rate_limit: None,
                supports_test: true,
            })
    }

    fn supported_protocols(&self) -> Option<&[String]> {
        None
    }

    // Host fields live on the PolicyPlugin wrapper — neutral defaults here.
    fn priority(&self) -> i32 {
        0
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn refresh_interval(&self) -> Option<u64> {
        None
    }

    fn set_priority(&self, _new_priority: i32) {}
    fn set_enabled(&self, _enabled: bool) {}
    fn set_refresh_interval(&self, _minutes: Option<u64>) {}

    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.host
            .rpc(
                Some(&self.instance_id),
                method,
                params.unwrap_or(Value::Null),
            )
            .await
    }

    async fn set_config(&self, config: Value) -> Result<Value> {
        self.host
            .rpc(Some(&self.instance_id), "set_config", config)
            .await
    }
}

/// Long-lived handles a plugin-type lifecycle task owns for its whole run.
/// Grouped into a struct so the spawned future does not carry a long argument
/// list.
struct PluginLifecycle {
    canonical_id: String,
    executable_path: PathBuf,
    request_rx: mpsc::UnboundedReceiver<HostRequest>,
    state: Arc<HostState>,
    process_state: Arc<TypeProcessState>,
    shutdown_token: CancellationToken,
    auth_secret: Option<String>,
    healthy: Arc<AtomicBool>,
    stats: Arc<PluginTypeStats>,
}

async fn manage_plugin_lifecycle(plugin: PluginLifecycle) {
    let PluginLifecycle {
        canonical_id,
        executable_path,
        mut request_rx,
        state,
        process_state,
        shutdown_token,
        auth_secret,
        healthy,
        stats,
    } = plugin;
    trace!(%canonical_id, "Managing plugin type lifecycle");
    let mut retry_count = 0;
    const MAX_RETRIES: u32 = 5;
    const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(60);
    let mut consecutive_restarts: u32 = 0;
    let mut direct_id: u64 = 0;

    loop {
        // A dropped host (all `request_tx` senders gone) or a cancelled
        // shutdown token must stop the respawn loop. Without this, a plugin
        // that can't pass the handshake respawns FOREVER — under CI parallel
        // load those orphaned respawns become a process storm that cascades
        // into other tests' handshake timeouts.
        if shutdown_token.is_cancelled() || request_rx.is_closed() {
            info!(
                "Stopping plugin lifecycle for {} (shutdown or host dropped)",
                canonical_id
            );
            return;
        }
        info!(
            "Spawning plugin process: {} (path: {})",
            canonical_id,
            executable_path.display()
        );
        let mut cmd = {
            #[cfg(windows)]
            {
                if executable_path.extension().is_some_and(|ext| ext == "py") {
                    // SSoT: `python_command()` picks the interpreter (python3
                    // first, python fallback) — the same choice the tests use.
                    let mut c =
                        Command::new(jumbie_shared::plugin::python_command().unwrap_or("python"));
                    c.arg(&executable_path);
                    c
                } else {
                    Command::new(&executable_path)
                }
            }
            #[cfg(not(windows))]
            {
                Command::new(&executable_path)
            }
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Environment isolation
        // The child gets ONLY the allowlist — never the backend's full
        // environment (DB credentials, API keys, tokens). This is the single
        // biggest secret-leak control: plugins inherit nothing by default.
        let mut envs: Vec<(String, String)> = Vec::new();
        if let Some(ref secret) = auth_secret {
            envs.push(("JUMBIE_PLUGIN_SECRET".to_string(), secret.clone()));
        }
        for key in [
            "PATH",
            "HOME",
            "TMPDIR",
            "TEMP",
            "TMP",
            "LANG",
            "LC_ALL",
            "__CF_USER_TEXT_ENCODING",
            "SystemRoot",
            "SystemDrive",
            "WINDIR",
            "PATHEXT",
            // Windows user/system location vars. Python 3.14's `pymanager`
            // launcher reads these to locate runtimes and user data — with
            // `env_clear()` they're absent and the child crashes at startup
            // ("expected str, bytes or os.PathLike object, not NoneType" →
            // "No runtimes are installed") instead of running the plugin.
            "LOCALAPPDATA",
            "APPDATA",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "COMSPEC",
            "OS",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "PROGRAMDATA",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            if let Ok(v) = std::env::var(key) {
                envs.push((key.to_string(), v));
            }
        }
        cmd.env_clear().envs(envs);

        // Per-plugin resource limits (Unix): set IN THE CHILD before exec so
        // they bound the plugin, not the host (a parent-side setrlimit would
        // constrain the backend itself). Only async-signal-safe libc calls are
        // allowed here; failures are best-effort.
        #[cfg(unix)]
        {
            unsafe {
                cmd.pre_exec(|| {
                    // `setrlimit`'s resource arg differs by platform: macOS/BSD use
                    // `c_int`, Linux gnu uses `__rlimit_resource_t` (u32). Take the
                    // constant as `c_int`, then `as _` picks the platform type.
                    fn limit(resource: libc::c_int, soft: u64, hard: u64) {
                        let l = libc::rlimit {
                            rlim_cur: soft as libc::rlim_t,
                            rlim_max: hard as libc::rlim_t,
                        };
                        unsafe {
                            libc::setrlimit(resource as _, &l);
                        }
                    }
                    limit(libc::RLIMIT_CPU as libc::c_int, 600, 600);
                    limit(libc::RLIMIT_NOFILE as libc::c_int, 1024, 1024);
                    limit(
                        libc::RLIMIT_FSIZE as libc::c_int,
                        1024 * 1024 * 1024,
                        1024 * 1024 * 1024,
                    );
                    // Deliberately NO RLIMIT_NPROC: on Linux it counts the
                    // USER's total processes, and an unprivileged child cannot
                    // raise it above its inherited hard limit — so on busy
                    // machines the plugin's own legitimate fork fails with
                    // EAGAIN. Runaway process-count abuse is bounded by
                    // group-kill on crash/shutdown plus deployment cgroup pids
                    // limits.
                    // Heap/anon-mmap cap (bounds runaway allocation; AS would
                    // break legitimate interpreters on some platforms).
                    limit(
                        libc::RLIMIT_DATA as libc::c_int,
                        3 * 1024 * 1024 * 1024,
                        3 * 1024 * 1024 * 1024,
                    );
                    Ok(())
                });
            }
        }

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = match cmd.spawn() {
            Ok(c) => {
                retry_count = 0;
                stats.restarts.fetch_add(1, Ordering::SeqCst);
                super::sandbox::track_pid(c.id().unwrap_or(0));
                c
            }
            Err(e) => {
                retry_count += 1;
                error!(
                    "Failed to spawn plugin {}: {}. Attempt {}/{}",
                    canonical_id, e, retry_count, MAX_RETRIES
                );
                if retry_count >= MAX_RETRIES {
                    error!(
                        "Max retries reached for plugin {}. Giving up.",
                        canonical_id
                    );
                    break;
                }
                tokio::time::sleep(BACKOFF_BASE).await;
                continue;
            }
        };
        let spawn_instant = std::time::Instant::now();
        // Capture the pid NOW: after `child.wait()` resolves, `child.id()`
        // returns None, but the group-kill still needs the pgid to reap
        // grandchildren (process_group(0) made this pid the group id).
        let child_pid: Option<i32> = child.id().map(|p| p as i32);

        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");

        // Bounded stderr: forwarded to the app's own logging with a line cap,
        // so a log-flooding plugin can no longer fill the disk via inherited
        // stderr.
        let stderr = child.stderr.take().expect("Failed to open stderr");
        {
            let canonical_id_stderr = canonical_id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                let mut count: u64 = 0;
                const MAX_LINES: u64 = 10_000;
                while let Ok(Some(line)) = lines.next_line().await {
                    count += 1;
                    if count <= MAX_LINES {
                        debug!(target: "plugin_stderr", "[{canonical_id_stderr}] {line}");
                    } else if count == MAX_LINES + 1 {
                        warn!(
                            "[{}] stderr output truncated after {} lines",
                            canonical_id_stderr, MAX_LINES
                        );
                    }
                }
            });
        }

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

        let state_reader = state.clone();
        let canonical_id_reader = canonical_id.clone();
        let stop_tx_reader = stop_tx.clone();
        let reader_auth_secret = auth_secret.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let raw_response = match serde_json::from_str::<JsonRpcResponse>(line) {
                    Ok(resp) => resp,
                    Err(e) => {
                        warn!(
                            "Plugin {} sent invalid JSON-RPC response: {}",
                            canonical_id_reader, e
                        );
                        continue;
                    }
                };

                // Bidirectional auth validation — FAIL-CLOSED: the response
                // must echo the request's auth token (presence AND match).
                // A missing echo is just as suspect as a wrong one — a rogue
                // process injecting into stdout cannot know the secret.
                if let Some(ref expected) = reader_auth_secret {
                    let resp_auth = match &raw_response {
                        JsonRpcResponse::Result(r) => r.auth.as_deref(),
                        JsonRpcResponse::Error(e) => e.auth.as_deref(),
                    };
                    if resp_auth != Some(expected.as_str()) {
                        warn!(
                            "Plugin {} response auth missing or mismatched",
                            canonical_id_reader
                        );
                        continue;
                    }
                }

                match raw_response {
                    JsonRpcResponse::Result(res) => {
                        if let Some(id_val) = res.id
                            && let Some(id) = id_val.as_u64()
                            && let Some(tx) = state_reader.pending_requests.lock().await.remove(&id)
                        {
                            let _ = tx.send(Ok(res.result));
                        }
                    }
                    JsonRpcResponse::Error(res) => {
                        if let Some(id_val) = res.id
                            && let Some(id) = id_val.as_u64()
                            && let Some(tx) = state_reader.pending_requests.lock().await.remove(&id)
                        {
                            let error_payload = serde_json::json!({
                                "code": res.error.code,
                                "message": res.error.message,
                                "data": res.error.data,
                            });
                            let _ = tx.send(Err(anyhow::anyhow!(
                                "{}",
                                serde_json::to_string(&error_payload).unwrap_or_else(|_| format!(
                                    "RPC Error {}: {}",
                                    res.error.code, res.error.message
                                ))
                            )));
                        }
                    }
                }
            }
            info!("Plugin {} stdout closed", canonical_id_reader);
            let _ = stop_tx_reader.send(()).await;
        });

        // Authenticated startup handshake: `hello` proves the process is alive,
        // the auth round-trip works in BOTH directions (a wrong/missing echo is
        // discarded by the reader, so this times out), and the protocol versions
        // match. Runs before instance replay / queue drain so a broken or foreign
        // process fails fast, and is raced against process death so an already-dead
        // process fails immediately instead of waiting out the handshake timeout.
        let handshake_fut = hello_handshake(
            &mut stdin,
            &state,
            &mut direct_id,
            &canonical_id,
            auth_secret.as_deref(),
        );
        let handshake_result = tokio::select! {
            res = handshake_fut => res,
            status = child.wait() => Err(anyhow::anyhow!(
                "process exited during handshake: {:?}", status
            )),
            _ = stop_rx.recv() => Err(anyhow::anyhow!(
                "stdout closed during handshake"
            )),
            // A cancelled shutdown must abort the handshake promptly instead
            // of waiting out the (deliberately generous) startup timeout.
            // Host-drop is handled at the top of the respawn loop via
            // `request_rx.is_closed()`, so termination is bounded either way.
            _ = shutdown_token.cancelled() => Err(anyhow::anyhow!(
                "shutdown requested during handshake"
            )),
        };
        if let Err(e) = handshake_result {
            error!(
                "Plugin {} failed startup handshake: {} — respawning",
                canonical_id, e
            );
            let _ = kill_plugin_group(&mut child, child_pid).await;
            // Fail any queued requests immediately instead of leaving them to
            // hit their 30s timeout through respawn cycles (the process is dead
            // or incompatible — retrying is pointless until it respawns). This
            // is SAFE for crash recovery: a healthy respawn always passes the
            // handshake, after which queued requests drain into the fresh
            // process (replay). Only broken processes reach this branch.
            while let Ok(host_req) = request_rx.try_recv() {
                let _ = host_req
                    .response_tx
                    .send(Err(anyhow::anyhow!("Plugin process crashed")));
            }
            let mut pending = state.pending_requests.lock().await;
            for (_, tx) in pending.drain() {
                let _ = tx.send(Err(anyhow::anyhow!("Plugin process crashed")));
            }
            drop(pending);
            let lived = spawn_instant.elapsed();
            consecutive_restarts = if lived > SURVIVAL_RESET {
                0
            } else {
                consecutive_restarts.saturating_add(1)
            };
            let delay = BACKOFF_BASE
                .saturating_mul(2u32.saturating_pow(consecutive_restarts.min(5)))
                .min(BACKOFF_CAP);
            tokio::time::sleep(delay).await;
            continue;
        }

        // Replay: re-initialize live instances into the fresh process.
        // Responses are awaited briefly so failures surface in the logs; the
        // requests are written directly to stdin (before the queue is drained)
        // so instance requests that queued during the downtime hit initialized
        // instances.
        let configs = process_state.instance_configs.lock().await.clone();
        let mut replay_failed = false;
        if !configs.is_empty() {
            info!(
                "Re-initializing {} instance(s) in fresh process {}",
                configs.len(),
                canonical_id
            );
            for (instance_id, config) in configs {
                direct_id += 1;
                let req_id = direct_id;
                let mut req = JsonRpcRequest::new(
                    "set_config",
                    config,
                    Some(json!(req_id)),
                    Some(instance_id.clone()),
                );
                if let Some(ref secret) = auth_secret {
                    req.auth = Some(secret.clone());
                }
                let (tx, rx) = oneshot::channel();
                state.pending_requests.lock().await.insert(req_id, tx);
                let req_json = serde_json::to_string(&req).unwrap() + "\n";
                if let Err(e) = write_request(&mut stdin, &canonical_id, &req_json).await {
                    error!(
                        "Failed to write replay initialize to plugin {} stdin: {}",
                        canonical_id, e
                    );
                    replay_failed = true;
                    break;
                }
                match tokio::time::timeout(Duration::from_secs(10), rx).await {
                    Ok(Ok(Ok(_))) => trace!("Replayed initialize for {instance_id}"),
                    Ok(Ok(Err(e))) => warn!("Replay initialize failed for {instance_id}: {e}"),
                    Ok(Err(e)) => warn!("Replay initialize for {instance_id} errored: {e}"),
                    Err(_) => {
                        drop_pending(&state, req_id).await;
                        warn!("Replay initialize for {instance_id} timed out");
                    }
                }
            }
        }

        // A write timeout during replay means the fresh process is HUNG (it
        // passed the handshake seconds ago, so the pipe was working) — skip the
        // main loop and route through the same crash cleanup as a dead process:
        // kill, fail in-flight requests, back off, respawn (replay re-runs).
        if replay_failed {
            let (restarts, delay) = crash_and_backoff(
                &healthy,
                &mut child,
                child_pid,
                &state,
                &canonical_id,
                spawn_instant,
                consecutive_restarts,
            )
            .await;
            consecutive_restarts = restarts;
            tracing::warn!(
                "Plugin {} hung during instance replay — respawning in {:?} (consecutive restarts: {})",
                canonical_id,
                delay,
                consecutive_restarts
            );
            tokio::time::sleep(delay).await;
            continue;
        }

        let mut health_check_timer = tokio::time::interval(HEALTH_CHECK_INTERVAL);
        health_check_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        health_check_timer.tick().await;
        // The process is alive and passed the handshake — reflect that in the
        // host's health state (in-process read for the status page).
        healthy.store(true, Ordering::SeqCst);

        let (health_result_tx, mut health_result_rx) = mpsc::channel::<bool>(2);
        let mut consecutive_health_failures = 0;

        loop {
            tokio::select! {
                _ = shutdown_token.cancelled() => {
                    info!("Shutdown requested for plugin type {}", canonical_id);
                    healthy.store(false, Ordering::SeqCst);
                    let _ = kill_plugin_group(&mut child, child_pid).await;
                    return;
                }
                req_opt = request_rx.recv() => {
                    match req_opt {
                        Some(host_req) => {
                            if let Some(id_val) = &host_req.req.id
                                && let Some(id) = id_val.as_u64() {
                                    state.pending_requests.lock().await.insert(id, host_req.response_tx);
                                }
                            let req_json = serde_json::to_string(&host_req.req).unwrap() + "\n";
                            // Bounded write: a process that stopped reading
                            // stdin (pipe full) is HUNG — kill + respawn.
                            if let Err(e) = write_request(&mut stdin, &canonical_id, &req_json).await {
                                error!("Failed to write to plugin {} stdin: {}", canonical_id, e);
                                break;
                            }
                            // Observability: the queue is deliberately unbounded
                            // (no drops), so a hung process shows up as depth
                            // here. Log when it gets deep so operators see it.
                            let depth = request_rx.len();
                            stats.queue_depth.store(depth as u64, Ordering::SeqCst);
                            if depth > 100 && depth.is_multiple_of(50) {
                                warn!(
                                    "Plugin {} request queue depth: {} — is the process hung?",
                                    canonical_id, depth
                                );
                            }
                        }
                        None => {
                            info!("PluginTypeHost for {} dropped, shutting down management task", canonical_id);
                            healthy.store(false, Ordering::SeqCst);
                            let _ = kill_plugin_group(&mut child, child_pid).await;
                            return;
                        }
                    }
                }
                _ = stop_rx.recv() => {
                    error!("Plugin {} communication failure (stdout closed)", canonical_id);
                    break;
                }
                status = child.wait() => {
                    error!("Plugin {} process exited unexpectedly: {:?}", canonical_id, status);
                    break;
                }
                _ = health_check_timer.tick() => {
                    let (tx, rx) = oneshot::channel();
                    direct_id += 1;
                    let hc_id = direct_id;
                    state.pending_requests.lock().await.insert(hc_id, tx);
                    let hc_tx = health_result_tx.clone();
                    let hc_state = state.clone();
                    tokio::spawn(async move {
                        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(10), rx).await {
                            let _ = hc_tx.send(true).await;
                        } else {
                            // No response — drop the stale pending entry too (P1).
                            hc_state.pending_requests.lock().await.remove(&hc_id);
                            let _ = hc_tx.send(false).await;
                        }
                    });

                    let mut hc_req = JsonRpcRequest::new("health_check", Value::Null, Some(json!(hc_id)), None);
                    if let Some(ref secret) = auth_secret {
                        hc_req.auth = Some(secret.clone());
                    }
                    let req_json = serde_json::to_string(&hc_req).unwrap() + "\n";
                    // Bounded write — a hung process must not wedge the
                    // lifecycle task (health checks keep firing on schedule).
                    if let Err(e) = write_request(&mut stdin, &canonical_id, &req_json).await {
                        error!("Health check failed to write to plugin {} stdin: {}", canonical_id, e);
                        break;
                    }
                }
                res = health_result_rx.recv() => {
                    if let Some(true) = res {
                        trace!(%canonical_id, health_result = true, "Plugin health check passed");
                        consecutive_health_failures = 0;
                    } else {
                        trace!(%canonical_id, health_result = false, "Plugin health check failed");
                        consecutive_health_failures += 1;
                        warn!("Plugin {} missed health check. Failures: {}", canonical_id, consecutive_health_failures);
                        if consecutive_health_failures >= 3 {
                            error!("Plugin {} failed health checks 3 times. Restarting...", canonical_id);
                            break;
                        }
                    }
                }
            }
        }

        // Crash cleanup (shared: dead process, hung process, replay failure):
        // kill, fail in-flight requests, then back off before respawning.
        let (restarts, delay) = crash_and_backoff(
            &healthy,
            &mut child,
            child_pid,
            &state,
            &canonical_id,
            spawn_instant,
            consecutive_restarts,
        )
        .await;
        consecutive_restarts = restarts;
        tracing::warn!(
            "Plugin {} process ended after {:?} (consecutive restarts: {}) — respawning in {:?}",
            canonical_id,
            spawn_instant.elapsed(),
            consecutive_restarts,
            delay
        );
        tokio::time::sleep(delay).await;
    }
}

/// Kill a dead/hung process, fail every in-flight request, and compute the
/// respawn backoff — the shared cleanup for the crash path AND the
/// replay-write-failure path (a process that hangs on replay writes is
/// treated exactly like a crashed one).
///
/// The exponential backoff: a process that died quickly is likely broken
/// (crash loop) — back off up to a cap. A process that survived a long time
/// resets the counter (its crash was probably a one-off).
async fn crash_and_backoff(
    healthy: &Arc<AtomicBool>,
    child: &mut tokio::process::Child,
    child_pid: Option<i32>,
    state: &Arc<HostState>,
    canonical_id: &str,
    spawn_instant: std::time::Instant,
    consecutive_restarts: u32,
) -> (u32, Duration) {
    healthy.store(false, Ordering::SeqCst);
    let _ = kill_plugin_group(child, child_pid).await;

    let mut pending = state.pending_requests.lock().await;
    info!(
        "Plugin {} crashed or disconnected. Clearing {} pending requests.",
        canonical_id,
        pending.len()
    );
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(anyhow::anyhow!("Plugin process crashed")));
    }
    drop(pending);

    let lived = spawn_instant.elapsed();
    let consecutive_restarts = if lived > SURVIVAL_RESET {
        0
    } else {
        consecutive_restarts.saturating_add(1)
    };
    let delay = BACKOFF_BASE
        .saturating_mul(2u32.saturating_pow(consecutive_restarts.min(5)))
        .min(BACKOFF_CAP);
    (consecutive_restarts, delay)
}

/// Drop a pending-request entry that will never be answered (the caller timed
/// out). Without this, late or never-arriving responses accumulate in
/// `pending_requests` until process death (P1).
async fn drop_pending(state: &Arc<HostState>, id: u64) {
    state.pending_requests.lock().await.remove(&id);
}

/// Bound for writes to the plugin's stdin (P2).
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Respawn backoff: 2s → 4s → ... → capped at 60s. Resets when a process
/// survives long enough that the previous crashes look like one-off events.
const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(60);
/// A process that survived this long is not crash-looping — reset the backoff.
const SURVIVAL_RESET: Duration = Duration::from_secs(300);

/// Write one request line to the plugin's stdin, bounded by [`WRITE_TIMEOUT`].
///
/// The bound exists because a process that stops reading stdin fills the OS
/// pipe buffer (64 KiB on Linux) and blocks `write_all` forever, which would
/// wedge the lifecycle task (health checks would never fire, queue grows
/// unbounded). A timeout here means the process is HUNG; the caller kills and
/// respawns it (queued requests survive; configs replay).
async fn write_request(
    stdin: &mut tokio::process::ChildStdin,
    canonical_id: &str,
    req_json: &str,
) -> Result<()> {
    match tokio::time::timeout(WRITE_TIMEOUT, async {
        stdin.write_all(req_json.as_bytes()).await?;
        stdin.flush().await
    })
    .await
    {
        Err(_) => Err(anyhow::anyhow!(
            "timed out writing to plugin {canonical_id} — process hung"
        )),
        Ok(Err(e)) => Err(anyhow::anyhow!(
            "failed to write to plugin {canonical_id} stdin: {e}"
        )),
        Ok(Ok(())) => Ok(()),
    }
}

/// Authenticated startup handshake: `hello` → `{protocol_version}`.
///
/// The response echo is validated by the stdout reader (discarded on mismatch),
/// so a successful response here proves the auth round-trip works both ways.
async fn hello_handshake(
    stdin: &mut tokio::process::ChildStdin,
    state: &Arc<HostState>,
    direct_id: &mut u64,
    canonical_id: &str,
    auth_secret: Option<&str>,
) -> Result<()> {
    *direct_id += 1;
    let req_id = *direct_id;
    let mut req = JsonRpcRequest::new("hello", Value::Null, Some(json!(req_id)), None);
    if let Some(secret) = auth_secret {
        req.auth = Some(secret.to_string());
    }
    let (tx, rx) = oneshot::channel();
    state.pending_requests.lock().await.insert(req_id, tx);
    let req_json = serde_json::to_string(&req).unwrap() + "\n";
    write_request(stdin, canonical_id, &req_json).await?;
    // Startup handshake bound, generous enough that a healthy but slow process
    // (cold cache, antivirus, loaded CI) isn't mislabelled as crashed, but not
    // so long that a spawned-but-unresponsive child holds a live process for its
    // full duration (under parallel load that accumulates until new spawns
    // fail). A dead process still fails fast — the outer `select!` races this
    // against `child.wait()`.
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(Ok(res))) => {
            let version = res
                .get("protocol_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if version != plugin_sdk::rpc::PROTOCOL_VERSION {
                Err(anyhow::anyhow!(
                    "protocol version mismatch: plugin speaks {version}, host speaks {}",
                    plugin_sdk::rpc::PROTOCOL_VERSION
                ))
            } else {
                tracing::info!("Plugin {} handshake ok (protocol v{version})", canonical_id);
                Ok(())
            }
        }
        Ok(Ok(Err(e))) => Err(anyhow::anyhow!("hello error: {e}")),
        Ok(Err(e)) => Err(anyhow::anyhow!("hello channel error: {e}")),
        Err(_) => {
            drop_pending(state, req_id).await;
            Err(anyhow::anyhow!(
                "hello timed out (dead process or bad auth echo)"
            ))
        }
    }
}

/// Kill the plugin's whole process group so grandchildren (shell-outs, helpers)
/// die too — a plugin must not leave orphans behind.
///
/// `pid` is captured at spawn time: after `child.wait()` resolves,
/// `child.id()` is `None`, but the process group (pgid == leader pid) still
/// needs the original pid to target.
async fn kill_plugin_group(child: &mut tokio::process::Child, pid: Option<i32>) -> Result<()> {
    // `process_group(0)` made the child a group leader (pgid == pid); kill the
    // group so grandchildren (shell-outs) don't outlive it. On Windows there's
    // no process-group kill, so `pid` is unused there.
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
    child.kill().await.map_err(|e| anyhow::anyhow!("kill: {e}"))
}
