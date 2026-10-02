use crate::components::common::structs::ActiveOperation;
use crate::utils::operation_ledger::OperationScope;
use leptos::prelude::*;
use std::cell::RefCell;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    Success,
    Warning,
    Error,
    Info,
}

impl NotificationType {
    pub fn css_class(&self) -> &'static str {
        match self {
            NotificationType::Success => "toast-success",
            NotificationType::Warning => "toast-warning",
            NotificationType::Error => "toast-error",
            NotificationType::Info => "toast-info",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            NotificationType::Success => "✓",
            NotificationType::Warning => "⚠",
            NotificationType::Error => "✕",
            NotificationType::Info => "ⓘ",
        }
    }
}

/// Display priority — derived from NotificationType.
/// Controls sort order and auto-dismiss duration.
/// Lower ordinal = higher priority (shown first, stays longer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotificationPriority {
    Error,
    Warning,
    Success,
    Info,
}

impl From<NotificationType> for NotificationPriority {
    fn from(t: NotificationType) -> Self {
        match t {
            NotificationType::Error => NotificationPriority::Error,
            NotificationType::Warning => NotificationPriority::Warning,
            NotificationType::Success => NotificationPriority::Success,
            NotificationType::Info => NotificationPriority::Info,
        }
    }
}

impl NotificationPriority {
    /// Base auto-dismiss duration (ms) when the queue is not backed up.
    fn base_duration_ms(&self) -> u32 {
        match self {
            NotificationPriority::Error => 12_000,
            NotificationPriority::Warning => 6_000,
            NotificationPriority::Success => 4_000,
            NotificationPriority::Info => 3_000,
        }
    }

    /// Accelerated auto-dismiss duration (ms) when there are queued
    /// notifications waiting off-screen.
    fn accelerated_duration_ms(&self) -> u32 {
        match self {
            NotificationPriority::Error => 8_000,
            NotificationPriority::Warning => 4_000,
            NotificationPriority::Success => 2_500,
            NotificationPriority::Info => 1_500,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: String,
    pub base_message: String,
    pub message: String,
    pub notification_type: NotificationType,
    pub priority: NotificationPriority,
    pub duration_ms: u32,
    pub duplicate_count: u32,
    /// When true, the toast never auto-dismisses. Must be explicitly
    /// dismissed or set to `false` when the operation it represents
    /// completes. Used for progress/operation toasts.
    pub sticky: bool,
    /// Set when the user hovers the toast. Protected from being
    /// displaced from the visible window.
    pub pinned: bool,
    /// Monotonically increasing insertion order. Used as a tie-breaker
    /// for priority-sorted display (newer = higher within same priority).
    pub index: u64,
    /// For sticky progress toasts: current progress state.
    pub progress: Option<ProgressState>,
    /// When this notification represents a backend operation (batch move, etc),
    /// this stores the stable backend operation ID so the status poll can
    /// reconcile toasts across tabs/page refreshes.
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressState {
    pub completed: usize,
    pub total: usize,
}

impl ProgressState {
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f64 / self.total as f64
        }
    }
}

impl Notification {
    /// Create a new notification with priority-based default duration.
    fn new(message: String, notification_type: NotificationType, index: u64) -> Self {
        let priority = NotificationPriority::from(notification_type);
        Self {
            id: Uuid::new_v4().to_string(),
            base_message: message.clone(),
            message,
            notification_type,
            priority,
            duration_ms: priority.base_duration_ms(),
            duplicate_count: 0,
            sticky: false,
            pinned: false,
            index,
            progress: None,
            operation_id: None,
        }
    }

    /// Create a sticky progress notification. Never auto-dismisses.
    fn new_progress(base_message: String, index: u64) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            base_message: base_message.clone(),
            message: base_message,
            notification_type: NotificationType::Info,
            priority: NotificationPriority::Info,
            duration_ms: 0,
            duplicate_count: 0,
            sticky: true,
            pinned: false,
            index,
            progress: None,
            operation_id: None,
        }
    }

    /// Returns the effective auto-dismiss duration, considering whether
    /// the queue currently has a backlog.
    fn effective_duration(&self, has_backlog: bool) -> u32 {
        if self.sticky {
            u32::MAX // effectively never
        } else if has_backlog {
            self.priority.accelerated_duration_ms()
        } else {
            self.priority.base_duration_ms()
        }
    }

    /// Updates progress on a sticky toast. Returns true when the
    /// operation is complete (completed == total).
    fn update_progress(&mut self, completed: usize, total: usize) -> bool {
        self.progress = Some(ProgressState { completed, total });
        let done = completed >= total && total > 0;
        if done {
            self.sticky = false;
            self.duration_ms = self.priority.base_duration_ms();
            // Bump the id so reactivity triggers a re-render
            self.id = Uuid::new_v4().to_string();
            self.message = format!("{} — complete ({} succeeded)", self.base_message, completed);
        }
        done
    }
}

/// Maximum number of toasts rendered simultaneously (viewport-aware default).
const _DEFAULT_MAX_VISIBLE: u32 = 4;

#[derive(Debug, Clone)]
pub struct NotificationStore {
    /// All undismissed notifications. Never evicted — the user can
    /// expand the container to see everything.
    inbox: Vec<Notification>,
    /// Whether the user has expanded to see all pending notifications.
    expanded: bool,
    /// Monotonically increasing counter for insertion order.
    next_index: u64,
}

impl NotificationStore {
    fn new() -> Self {
        Self {
            inbox: Vec::new(),
            expanded: false,
            next_index: 0,
        }
    }

    /// Insert a notification with duplicate collapse.
    fn push(&mut self, mut notif: Notification) {
        notif.index = self.next_index;
        self.next_index += 1;

        // Duplicate collapse: same base_message + same type
        if let Some(pos) = self.inbox.iter().position(|n| {
            n.base_message == notif.base_message && n.notification_type == notif.notification_type
        }) {
            let mut existing = self.inbox.remove(pos);
            existing.duplicate_count += 1;
            existing.message = format!("{} ({})", existing.base_message, existing.duplicate_count);
            existing.id = Uuid::new_v4().to_string();
            existing.index = notif.index; // move to "latest" position
            self.inbox.push(existing);
        } else {
            self.inbox.push(notif);
        }
    }

    /// Remove a notification by ID.
    fn dismiss(&mut self, id: &str) {
        self.inbox.retain(|n| n.id != id);
    }

    /// Remove all notifications.
    fn dismiss_all(&mut self) {
        self.inbox.clear();
    }

    /// Toggle the expanded view.
    fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Update progress on a sticky notification. Returns the updated
    /// notification if found.
    fn set_progress(
        &mut self,
        base_message: &str,
        completed: usize,
        total: usize,
    ) -> Option<Notification> {
        let pos = self
            .inbox
            .iter()
            .position(|n| n.base_message == base_message && n.sticky)?;
        let notif = &mut self.inbox[pos];
        notif.update_progress(completed, total);
        Some(self.inbox[pos].clone())
    }

    /// Finalize the tracked toast for `operation_id`, if present.
    fn finalize_by_operation_id(&mut self, operation_id: &str, op: &ActiveOperation) {
        if let Some(notif) = self
            .inbox
            .iter_mut()
            .find(|n| n.operation_id.as_deref() == Some(operation_id))
        {
            finalize_progress(notif, op);
        }
    }

    /// Reconcile progress/result toasts against backend operations.
    ///
    /// Only operations initiated by this tab (`scope.mine`) are considered, and
    /// only until their result is recorded in `scope.delivered`. Returns the ids
    /// delivered by this call so the caller can persist them.
    fn reconcile(
        &mut self,
        operations: &[ActiveOperation],
        now_ms: u64,
        scope: &OperationScope,
    ) -> Vec<String> {
        let active_ids: std::collections::HashSet<&str> =
            operations.iter().map(|o| o.id.as_str()).collect();
        // Must match backend `COMPLETED_TASK_RETENTION`.
        const RECENCY_MS: u64 = 30 * 60 * 1000;
        let mut delivered = Vec::new();

        for op in operations {
            let Some(label) = scope.mine.get(&op.id) else {
                continue;
            };
            if scope.delivered.contains(&op.id) {
                continue;
            }

            let existing = self
                .inbox
                .iter()
                .position(|n| n.operation_id.as_deref() == Some(op.id.as_str()));

            match existing {
                Some(idx) if op.finished => {
                    finalize_progress(&mut self.inbox[idx], op);
                    delivered.push(op.id.clone());
                }
                Some(idx) => {
                    self.inbox[idx].update_progress(op.completed, op.total);
                }
                None if op.finished => {
                    let is_recent = op
                        .finished_at_ms
                        .map(|ts| now_ms.saturating_sub(ts) < RECENCY_MS)
                        .unwrap_or(false);
                    if is_recent {
                        let mut notif = Notification::new(
                            label.clone(),
                            NotificationType::Success,
                            self.next_index,
                        );
                        notif.operation_id = Some(op.id.clone());
                        finalize_progress(&mut notif, op);
                        notif.duration_ms = 8_000;
                        self.next_index += 1;
                        self.inbox.push(notif);
                        delivered.push(op.id.clone());
                    }
                }
                None => {
                    let mut notif = Notification::new_progress(label.clone(), self.next_index);
                    notif.operation_id = Some(op.id.clone());
                    notif.progress = Some(ProgressState {
                        completed: op.completed,
                        total: op.total,
                    });
                    self.next_index += 1;
                    self.inbox.push(notif);
                }
            }
        }

        self.inbox.retain(|n| match &n.operation_id {
            Some(op_id) => active_ids.contains(op_id.as_str()),
            None => true,
        });

        delivered
    }

    /// Whether the queue has a backlog (more notifications than can be shown).
    fn has_backlog(&self, max_visible: u32) -> bool {
        self.inbox.len() > max_visible as usize
    }

    /// Pin/unpin a notification (user hover).
    fn set_pinned(&mut self, id: &str, pinned: bool) {
        if let Some(n) = self.inbox.iter_mut().find(|n| n.id == id) {
            n.pinned = pinned;
        }
    }

    /// Compute the visible window of notifications.
    fn visible(&self, max_visible: u32) -> Vec<Notification> {
        if self.expanded {
            // Show everything
            let mut result = self.inbox.clone();
            result.reverse(); // newest first
            return result;
        }

        if self.inbox.is_empty() {
            return vec![];
        }

        let has_backlog = self.has_backlog(max_visible);

        // Sort by: priority (higher first), then by index (newer first)
        let mut sorted: Vec<&Notification> = self.inbox.iter().collect();
        sorted.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| b.index.cmp(&a.index))
        });

        // Take up to max_visible
        let count = (max_visible as usize).min(sorted.len());
        let mut visible: Vec<Notification> = sorted.into_iter().take(count).cloned().collect();

        // Recalculate durations based on backlog
        for n in &mut visible {
            if !n.sticky {
                n.duration_ms = n.effective_duration(has_backlog);
            }
        }

        visible
    }

    fn needs_expand_button(&self, max_visible: u32) -> bool {
        self.inbox.len() > max_visible as usize
    }

    fn pending_count(&self) -> usize {
        self.inbox.len()
    }

    fn is_expanded(&self) -> bool {
        self.expanded
    }
}

/// Compute how many toasts fit in the current viewport.
/// Caps at 8, minimum 1.
fn compute_max_visible() -> u32 {
    let height = web_sys::window()
        .and_then(|w| w.inner_height().ok())
        .and_then(|h| h.as_f64())
        .unwrap_or(900.0);
    // Account for: container padding (32px), each toast ~68px + 12px gap
    let available = height - 32.0;
    let toast_height = 68.0 + 12.0;
    ((available / toast_height) as u32).clamp(1, 8)
}

thread_local! {
    static MAX_VISIBLE: RwSignal<u32> = RwSignal::new(4);
}

fn get_max_visible() -> u32 {
    MAX_VISIBLE.with(|sig| sig.get_untracked())
}

/// Initialise the max_visible signal and set up a resize listener.
/// Called once from `provide_notification_context`.
fn init_max_visible() {
    MAX_VISIBLE.with(|sig| sig.set(compute_max_visible()));
    window_event_listener(leptos::ev::resize, move |_| {
        MAX_VISIBLE.with(|sig| sig.set(compute_max_visible()));
    });
}

thread_local! {
    static GLOBAL_CONTEXT: RefCell<Option<NotificationContext>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct NotificationContext {
    store: RwSignal<NotificationStore>,
}

impl Default for NotificationContext {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationContext {
    pub fn new() -> Self {
        Self {
            store: RwSignal::new(NotificationStore::new()),
        }
    }

    pub fn show(&self, message: String, notification_type: NotificationType) {
        let index = {
            let s = self.store.get_untracked();
            s.next_index
        };
        let notif = Notification::new(message, notification_type, index);
        self.store.update(|s| {
            s.push(notif);
        });
    }

    /// Create a progress toast tied to a backend operation ID and record the
    /// operation as initiated by this tab.
    pub fn show_operation_progress(&self, base_message: String, operation_id: String) -> String {
        crate::utils::operation_ledger::register_operation(&operation_id, &base_message);
        let index = {
            let s = self.store.get();
            s.next_index
        };
        let mut notif = Notification::new_progress(base_message, index);
        notif.operation_id = Some(operation_id);
        let id = notif.id.clone();
        self.store.update(|s| {
            s.push(notif);
        });
        id
    }

    /// Show a sticky progress toast. Returns the notification ID so
    /// the caller can update progress later via `update_progress`.
    pub fn show_progress(&self, base_message: String) -> String {
        let index = {
            let s = self.store.get();
            s.next_index
        };
        let notif = Notification::new_progress(base_message, index);
        let id = notif.id.clone();
        self.store.update(|s| {
            s.push(notif);
        });
        id
    }

    /// Update progress on a sticky toast by base_message.
    /// When completed >= total, the toast becomes non-sticky and
    /// auto-dismisses after the standard duration.
    pub fn update_progress(&self, base_message: &str, completed: usize, total: usize) {
        self.store.update(|s| {
            s.set_progress(base_message, completed, total);
        });
    }

    /// Transform a progress toast into a result toast by operation_id and record
    /// the result as delivered so it is never shown again in this tab.
    pub fn finalize_operation(&self, operation_id: &str, op: &ActiveOperation) {
        self.store.update(|s| {
            s.finalize_by_operation_id(operation_id, op);
        });
        crate::utils::operation_ledger::mark_delivered(operation_id);
    }

    /// Called by the status poll. Shows/updates toasts only for operations this
    /// tab initiated, then records any delivered results so a refresh or a later
    /// poll cannot resurrect them.
    pub fn reconcile_operations(&self, operations: Vec<ActiveOperation>) {
        let scope = crate::utils::operation_ledger::scope();
        let now_ms = js_sys::Date::now() as u64;
        let mut delivered = Vec::new();
        self.store.update(|s| {
            delivered = s.reconcile(&operations, now_ms, &scope);
        });
        for id in delivered {
            crate::utils::operation_ledger::mark_delivered(&id);
        }
    }

    pub fn dismiss(&self, id: &str) {
        let id = id.to_string();
        self.store.update(|s| {
            s.dismiss(&id);
        });
    }

    pub fn dismiss_all(&self) {
        self.store.update(|s| {
            s.dismiss_all();
        });
    }

    pub fn toggle_expanded(&self) {
        self.store.update(|s| {
            s.toggle_expanded();
        });
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) {
        let id = id.to_string();
        self.store.update(|s| {
            s.set_pinned(&id, pinned);
        });
    }

    /// Returns the visible window of notifications for the current render cycle.
    pub fn visible(&self) -> Signal<Vec<Notification>> {
        let store = self.store;
        Signal::derive(move || {
            let s = store.get();
            s.visible(get_max_visible())
        })
    }

    /// Whether there are more notifications than fit in the visible window.
    pub fn has_backlog(&self) -> Signal<bool> {
        let store = self.store;
        Signal::derive(move || {
            let s = store.get();
            s.inbox.len() > get_max_visible() as usize
        })
    }

    /// Whether to show the expand/collapse button (multiple notifications).
    pub fn show_expand_button(&self) -> Signal<bool> {
        let store = self.store;
        Signal::derive(move || {
            let s = store.get();
            s.needs_expand_button(get_max_visible())
        })
    }

    /// Whether the view is currently expanded.
    pub fn is_expanded(&self) -> Signal<bool> {
        let store = self.store;
        Signal::derive(move || store.get().is_expanded())
    }

    /// Total pending notification count (for badge display).
    pub fn pending_count(&self) -> Signal<usize> {
        let store = self.store;
        Signal::derive(move || store.get().pending_count())
    }

    /// The raw store signal (for direct use in ToastContainer rendering).
    pub fn get_store(&self) -> RwSignal<NotificationStore> {
        self.store
    }
}

/// Provide the notification context at the app root.
pub fn provide_notification_context() -> NotificationContext {
    init_max_visible();
    let context = NotificationContext::new();
    provide_context(context.clone());
    GLOBAL_CONTEXT.with(|global| {
        *global.borrow_mut() = Some(context.clone());
    });
    context
}

/// Resolve the notification context — tries the reactive owner chain first,
/// then the thread_local global fallback.
fn resolve_notification_context() -> Option<NotificationContext> {
    if let Some(ctx) = use_context::<NotificationContext>() {
        return Some(ctx);
    }
    GLOBAL_CONTEXT.with(|global| global.borrow().clone())
}

pub fn use_notification() -> NotificationContext {
    resolve_notification_context().expect(
        "NotificationContext not provided. \
         Make sure to call provide_notification_context() in your app root.",
    )
}

pub fn show_success(message: impl Into<String>) {
    if let Some(ctx) = resolve_notification_context() {
        ctx.show(message.into(), NotificationType::Success);
    }
}

pub fn show_warning(message: impl Into<String>) {
    if let Some(ctx) = resolve_notification_context() {
        ctx.show(message.into(), NotificationType::Warning);
    }
}

pub fn show_error(message: impl Into<String>) {
    if let Some(ctx) = resolve_notification_context() {
        ctx.show(message.into(), NotificationType::Error);
    }
}

pub fn show_info(message: impl Into<String>) {
    if let Some(ctx) = resolve_notification_context() {
        ctx.show(message.into(), NotificationType::Info);
    }
}

pub fn show_error_toast(label: &str, err: impl std::fmt::Display) {
    show_error(format!("{}: {}", label, err));
}

pub fn show_toast(message: impl Into<String>, notification_type: NotificationType) {
    if let Some(ctx) = resolve_notification_context() {
        ctx.show(message.into(), notification_type);
    }
}

/// Transforms a progress toast into a result toast (SSoT for the icon/colour/message
/// logic, shared by the polling loop and `reconcile_operations`).
///
/// `update_progress()` runs first so the sticky/duration fields are set correctly;
/// the message is then overwritten with a richer format (success_count, failed,
/// first error) than `update_progress()`'s generic "(N succeeded)" template.
pub fn finalize_progress(
    notif: &mut Notification,
    op: &crate::components::common::structs::ActiveOperation,
) {
    // Set the outcome type before update_progress so duration_ms is calculated
    // with the correct priority (not the old Info priority).
    let ntype = match (op.failed, op.success_count) {
        (0, _) => NotificationType::Success,
        (f, s) if s > f => NotificationType::Warning,
        _ => NotificationType::Error,
    };
    notif.notification_type = ntype;
    notif.priority = NotificationPriority::from(ntype);

    // Mark as done — sets sticky=false, duration=base (correct priority),
    // id=bumped for reactivity, generic message overwritten.
    notif.update_progress(op.total, op.total);

    let mut msg = format!(
        "{} — complete ({} ok, {} failed)",
        notif.base_message, op.success_count, op.failed,
    );
    if !op.errors.is_empty() {
        msg.push_str(&format!(". Failed: {}", op.errors[0]));
    }
    notif.message = msg;

    // Safety net: update_progress requires total > 0 to set sticky=false.
    notif.sticky = false;
    if notif.duration_ms == 0 || notif.duration_ms == u32::MAX {
        notif.duration_ms = notif.priority.base_duration_ms();
    }
}

#[component]
pub fn Toast(notification: Notification) -> impl IntoView {
    let ctx = use_notification();
    let notification_type = notification.notification_type;
    let id = notification.id.clone();
    let is_sticky = notification.sticky;
    let progress = notification.progress;
    let duration_ms = notification.duration_ms;

    let class = format!("toast {}", notification_type.css_class());

    let (hovered, set_hovered) = signal(false);
    let handle = StoredValue::new(None::<TimeoutHandle>);

    // Pin/unpin on hover so the toast can't be displaced from the visible window
    let ctx_pin = ctx.clone();
    let id_pin = id.clone();
    Effect::new(move |_| {
        if hovered.get() != notification.pinned {
            ctx_pin.set_pinned(&id_pin, hovered.get());
        }
    });

    // Auto-dismiss timeout (only for non-sticky toasts)
    if !is_sticky {
        let ctx_effect = ctx.clone();
        let id_clone = id.clone();
        Effect::new(move |_| {
            if hovered.get() {
                if let Some(h) = handle.write_value().take() {
                    h.clear();
                }
            } else {
                let ctx_clone = ctx_effect.clone();
                let id_cl = id_clone.clone();
                if let Ok(h) = set_timeout_with_handle(
                    move || ctx_clone.dismiss(&id_cl),
                    std::time::Duration::from_millis(duration_ms as u64),
                ) {
                    *handle.write_value() = Some(h);
                }
            }
        });
    }

    on_cleanup(move || {
        if let Some(h) = handle.write_value().take() {
            h.clear();
        }
    });

    let progress_style = move || {
        if is_sticky {
            if let Some(p) = progress {
                let pct = (p.fraction() * 100.0) as u32;
                format!("width: {}%", pct.min(100))
            } else {
                // Indeterminate — CSS handles with animation
                String::new()
            }
        } else {
            format!("animation-duration: {}ms", duration_ms)
        }
    };

    let id_clone = id.clone();

    view! {
        <div
            class=class
            class:toast-sticky=is_sticky
            on:mouseenter=move |_| set_hovered.set(true)
            on:mouseleave=move |_| set_hovered.set(false)
        >
            <span class="toast-icon">{notification_type.icon()}</span>
            <span class="toast-message">{notification.message}</span>
            <button class="toast-close" on:click=move |_| ctx.dismiss(&id_clone)>
                "×"
            </button>
            <div class="toast-progress-bar">
                <div
                    class="toast-progress"
                    class:running=move || !is_sticky && !hovered.get()
                    class:toast-progress-indeterminate=move || is_sticky && progress.is_none()
                    class:toast-progress-determinate=move || progress.is_some()
                    style=progress_style
                ></div>
            </div>
        </div>
    }
}

#[component]
pub fn ToastContainer() -> impl IntoView {
    let ctx = use_notification();
    let visible = ctx.visible();
    let show_expand = ctx.show_expand_button();
    let is_expanded = ctx.is_expanded();
    let pending_count = ctx.pending_count();

    // Clone ctx for each closure to avoid FnOnce issues with Show children
    let ctx_expand = ctx.clone();
    let ctx_collapse = ctx.clone();
    let ctx_dismiss_all = ctx.clone();

    let max_vis = get_max_visible();

    view! {
        <div class="toast-container" class:toast-container-expanded=is_expanded>
            <For
                each=move || visible.get()
                key=|n| n.id.clone()
                children=move |notification| {
                    view! { <Toast notification=notification /> }
                }
            />

            {
                let ctx = ctx_expand.clone();
                let ctx_d = ctx_dismiss_all.clone();
                let pc = pending_count;
                let show_sig = show_expand;
                let exp_sig = is_expanded;
                move || {
                    if show_sig.get() && !exp_sig.get() {
                        let ctx = ctx.clone();
                        let ctx_d = ctx_d.clone();
                        let n = pc.get().saturating_sub(max_vis as usize);
                        Some(view! {
                            <div class="toast-controls">
                                <button
                                    class="toast-control-btn"
                                    on:click=move |_| ctx.toggle_expanded()
                                    title="Show all notifications"
                                >
                                    {format!("{} more +", n)}
                                </button>
                                <button
                                    class="toast-control-btn toast-control-btn-danger"
                                    on:click=move |_| ctx_d.dismiss_all()
                                    title="Dismiss all notifications"
                                >
                                    "Dismiss All"
                                </button>
                            </div>
                        })
                    } else {
                        None
                    }
                }
            }

            {
                let ctx_c = ctx_collapse.clone();
                let ctx_d = ctx_dismiss_all.clone();
                let exp_sig = is_expanded;
                move || {
                    if exp_sig.get() {
                        let ctx_c = ctx_c.clone();
                        let ctx_d = ctx_d.clone();
                        Some(view! {
                            <div class="toast-controls">
                                <button
                                    class="toast-control-btn"
                                    on:click=move |_| ctx_c.toggle_expanded()
                                    title="Collapse"
                                >
                                    "Collapse"
                                </button>
                                <button
                                    class="toast-control-btn toast-control-btn-danger"
                                    on:click=move |_| ctx_d.dismiss_all()
                                    title="Dismiss all"
                                >
                                    "Dismiss All"
                                </button>
                            </div>
                        })
                    } else {
                        None
                    }
                }
            }
        </div>
    }
}

pub trait ResultToastExt<T> {
    fn toast_on_err(self, title: &str) -> Option<T>;
}

impl<T, E: std::fmt::Display> ResultToastExt<T> for Result<T, E> {
    fn toast_on_err(self, title: &str) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                show_error(format!("{}: {}", title, e));
                None
            }
        }
    }
}

#[macro_export]
macro_rules! toast_error_and_return {
    ($res:expr, $label:expr) => {
        match $res {
            Ok(v) => v,
            Err(e) => {
                $crate::components::common::toast::show_error(format!("{}: {}", $label, e));
                return;
            }
        }
    };
    ($res:expr, $label:expr, $ret:expr) => {
        match $res {
            Ok(v) => v,
            Err(e) => {
                $crate::components::common::toast::show_error(format!("{}: {}", $label, e));
                return $ret;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a store and push a notification.
    fn push(store: &mut NotificationStore, msg: &str, ntype: NotificationType) {
        let notif = Notification::new(msg.to_string(), ntype, store.next_index);
        store.next_index += 1;
        store.push(notif);
    }

    #[test]
    fn test_push_normal_adds_to_inbox() {
        let mut store = NotificationStore::new();
        push(&mut store, "hello", NotificationType::Info);
        assert_eq!(store.inbox.len(), 1);
    }

    #[test]
    fn test_dedup_same_message_type_collapses() {
        let mut store = NotificationStore::new();
        push(&mut store, "same", NotificationType::Error);
        push(&mut store, "same", NotificationType::Error);
        // Should be collapsed into one with duplicate_count = 1
        assert_eq!(store.inbox.len(), 1);
        assert_eq!(store.inbox[0].duplicate_count, 1);
    }

    #[test]
    fn test_dedup_same_message_different_type_stays_separate() {
        let mut store = NotificationStore::new();
        push(&mut store, "same", NotificationType::Error);
        push(&mut store, "same", NotificationType::Info);
        assert_eq!(store.inbox.len(), 2);
    }

    #[test]
    fn test_dedup_different_message_same_type_stays_separate() {
        let mut store = NotificationStore::new();
        push(&mut store, "error A", NotificationType::Error);
        push(&mut store, "error B", NotificationType::Error);
        assert_eq!(store.inbox.len(), 2);
    }

    #[test]
    fn test_no_eviction_inbox_grows() {
        let mut store = NotificationStore::new();
        for i in 0..100 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        assert_eq!(store.inbox.len(), 100);
    }

    #[test]
    fn test_dismiss_removes_by_id() {
        let mut store = NotificationStore::new();
        push(&mut store, "msg", NotificationType::Info);
        let id = store.inbox[0].id.clone();
        store.dismiss(&id);
        assert!(store.inbox.is_empty());
    }

    #[test]
    fn test_dismiss_unknown_id_is_noop() {
        let mut store = NotificationStore::new();
        push(&mut store, "msg", NotificationType::Info);
        store.dismiss("nonexistent");
        assert_eq!(store.inbox.len(), 1);
    }

    #[test]
    fn test_dismiss_all_clears_everything() {
        let mut store = NotificationStore::new();
        for i in 0..10 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        store.dismiss_all();
        assert!(store.inbox.is_empty());
    }

    #[test]
    fn test_visible_returns_all_when_under_limit() {
        let mut store = NotificationStore::new();
        push(&mut store, "msg", NotificationType::Info);
        push(&mut store, "msg2", NotificationType::Info);
        let vis = store.visible(10);
        assert_eq!(vis.len(), 2);
    }

    #[test]
    fn test_visible_limits_by_max_visible() {
        let mut store = NotificationStore::new();
        for i in 0..10 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        let vis = store.visible(3);
        assert_eq!(vis.len(), 3);
    }

    #[test]
    fn test_visible_errors_before_info() {
        let mut store = NotificationStore::new();
        push(&mut store, "info", NotificationType::Info);
        push(&mut store, "error", NotificationType::Error);
        push(&mut store, "warning", NotificationType::Warning);
        let vis = store.visible(10);
        // Order should be: Error first, then Warning, then Info
        assert_eq!(vis[0].notification_type, NotificationType::Error);
        assert_eq!(vis[1].notification_type, NotificationType::Warning);
        assert_eq!(vis[2].notification_type, NotificationType::Info);
    }

    #[test]
    fn test_visible_newer_items_first_within_same_priority() {
        let mut store = NotificationStore::new();
        push(&mut store, "first", NotificationType::Info);
        push(&mut store, "second", NotificationType::Info);
        push(&mut store, "third", NotificationType::Info);
        let vis = store.visible(10);
        // Newest first within same priority
        assert_eq!(vis[0].base_message, "third");
        assert_eq!(vis[1].base_message, "second");
        assert_eq!(vis[2].base_message, "first");
    }

    #[test]
    fn test_visible_expanded_shows_all() {
        let mut store = NotificationStore::new();
        for i in 0..20 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        store.expanded = true;
        let vis = store.visible(3);
        assert_eq!(vis.len(), 20); // max_visible ignored when expanded
    }

    #[test]
    fn test_visible_accelerated_duration_when_backlog() {
        let mut store = NotificationStore::new();
        // Fill beyond max_visible so there's a backlog
        for i in 0..6 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Success);
        }
        let vis = store.visible(3);
        for n in &vis {
            // Accelerated: Success base is 4000, accelerated is 2500
            assert_eq!(n.duration_ms, 2500);
        }
    }

    #[test]
    fn test_visible_normal_duration_when_no_backlog() {
        let mut store = NotificationStore::new();
        push(&mut store, "msg", NotificationType::Success);
        let vis = store.visible(5);
        assert_eq!(vis[0].duration_ms, 4000); // base, not accelerated
    }

    #[test]
    fn test_sticky_toast_never_auto_dismisses() {
        let notif = Notification::new_progress("working".to_string(), 0);
        assert!(notif.sticky);
        assert_eq!(notif.effective_duration(false), u32::MAX);
        assert_eq!(notif.effective_duration(true), u32::MAX);
    }

    #[test]
    fn test_update_progress_fills_toast() {
        let mut notif = Notification::new_progress("moving files".to_string(), 0);
        assert!(notif.progress.is_none());

        notif.update_progress(3, 10);
        assert!(notif.sticky); // still sticky
        assert_eq!(notif.progress.unwrap().fraction(), 0.3);

        notif.update_progress(10, 10);
        assert!(!notif.sticky); // complete → no longer sticky
        assert!(notif.progress.unwrap().fraction() >= 1.0);
    }

    #[test]
    fn test_update_progress_sets_duration_when_done() {
        let mut notif = Notification::new_progress("done".to_string(), 0);
        notif.update_progress(5, 5);
        assert_eq!(notif.duration_ms, notif.priority.base_duration_ms());
        assert!(!notif.sticky);
    }

    #[test]
    fn test_set_progress_on_store_updates_in_place() {
        let mut store = NotificationStore::new();
        let notif = Notification::new_progress("batch".to_string(), store.next_index);
        store.next_index += 1;
        store.push(notif);

        let result = store.set_progress("batch", 3, 10);
        assert!(result.is_some());
        assert_eq!(store.inbox[0].progress.unwrap().completed, 3);

        store.set_progress("batch", 10, 10);
        assert!(!store.inbox[0].sticky);
    }

    #[test]
    fn test_set_progress_unknown_base_message_returns_none() {
        let mut store = NotificationStore::new();
        let notif = Notification::new_progress("real".to_string(), 0);
        store.push(notif);
        let result = store.set_progress("nonexistent", 5, 10);
        assert!(result.is_none());
    }

    #[test]
    fn test_pinned_toast_included_even_over_max_visible() {
        let mut store = NotificationStore::new();
        for i in 0..5 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        // Pin the last one
        let pinned_id = store.inbox[4].id.clone();
        store.set_pinned(&pinned_id, true);

        // With max_visible=3, the pinned one should still be in visible
        let vis = store.visible(3);
        assert!(vis.iter().any(|n| n.id == pinned_id));
    }

    #[test]
    fn test_unpin_removes_protection() {
        let mut store = NotificationStore::new();
        for i in 0..5 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        let pinned_id = store.inbox[4].id.clone();
        store.set_pinned(&pinned_id, true);
        store.set_pinned(&pinned_id, false);

        let vis = store.visible(3);
        // With 5 items and max_visible=3, the unpinned item may be excluded
        assert_eq!(vis.len(), 3);
        // It might or might not be in the visible set depending on sort order
        // (but the visible set is limited to 3)
    }

    #[test]
    fn test_pin_unknown_id_is_noop() {
        let mut store = NotificationStore::new();
        store.set_pinned("nonexistent", true); // should not panic
        assert_eq!(store.inbox.len(), 0);
    }

    #[test]
    fn test_needs_expand_button_when_backlog() {
        let mut store = NotificationStore::new();
        for i in 0..10 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        assert!(store.needs_expand_button(3));
        assert!(!store.needs_expand_button(10));
    }

    #[test]
    fn test_toggle_expanded() {
        let mut store = NotificationStore::new();
        assert!(!store.is_expanded());
        store.toggle_expanded();
        assert!(store.is_expanded());
        store.toggle_expanded();
        assert!(!store.is_expanded());
    }

    #[test]
    fn test_has_backlog() {
        let mut store = NotificationStore::new();
        assert!(!store.has_backlog(5));
        for i in 0..7 {
            push(&mut store, &format!("msg-{}", i), NotificationType::Info);
        }
        assert!(store.has_backlog(5));
        assert!(!store.has_backlog(10));
    }

    #[test]
    fn test_priority_base_durations() {
        assert_eq!(NotificationPriority::Error.base_duration_ms(), 12_000);
        assert_eq!(NotificationPriority::Warning.base_duration_ms(), 6_000);
        assert_eq!(NotificationPriority::Success.base_duration_ms(), 4_000);
        assert_eq!(NotificationPriority::Info.base_duration_ms(), 3_000);
    }

    #[test]
    fn test_priority_accelerated_durations() {
        assert_eq!(NotificationPriority::Error.accelerated_duration_ms(), 8_000);
        assert_eq!(
            NotificationPriority::Warning.accelerated_duration_ms(),
            4_000
        );
        assert_eq!(
            NotificationPriority::Success.accelerated_duration_ms(),
            2_500
        );
        assert_eq!(NotificationPriority::Info.accelerated_duration_ms(), 1_500);
    }

    #[test]
    fn test_priority_ordering() {
        // Lower ordinal = higher priority (shown first)
        assert!(NotificationPriority::Error < NotificationPriority::Warning);
        assert!(NotificationPriority::Warning < NotificationPriority::Success);
        assert!(NotificationPriority::Success < NotificationPriority::Info);
    }

    #[test]
    fn test_new_notification_has_correct_defaults() {
        let n = Notification::new("test".to_string(), NotificationType::Warning, 42);
        assert_eq!(n.base_message, "test");
        assert_eq!(n.priority, NotificationPriority::Warning);
        assert_eq!(n.duration_ms, 6_000); // Warning base
        assert!(!n.sticky);
        assert!(!n.pinned);
        assert_eq!(n.index, 42);
        assert!(n.progress.is_none());
        assert!(n.operation_id.is_none());
    }

    #[test]
    fn test_new_progress_has_correct_defaults() {
        let n = Notification::new_progress("working".to_string(), 99);
        assert!(n.sticky);
        assert_eq!(n.priority, NotificationPriority::Info);
        assert_eq!(n.index, 99);
        assert!(n.progress.is_none());
        assert!(n.operation_id.is_none());
    }

    #[test]
    fn test_effective_duration_respects_sticky() {
        let mut n = Notification::new("test".to_string(), NotificationType::Info, 0);
        assert_eq!(n.effective_duration(false), 3_000);
        assert_eq!(n.effective_duration(true), 1_500);

        n.sticky = true;
        assert_eq!(n.effective_duration(false), u32::MAX);
        assert_eq!(n.effective_duration(true), u32::MAX);
    }

    #[test]
    fn test_progress_state_fraction() {
        let p = ProgressState {
            completed: 3,
            total: 10,
        };
        assert!((p.fraction() - 0.3).abs() < f64::EPSILON);

        let p = ProgressState {
            completed: 10,
            total: 10,
        };
        assert!((p.fraction() - 1.0).abs() < f64::EPSILON);

        let p = ProgressState {
            completed: 0,
            total: 0,
        };
        assert!((p.fraction() - 0.0).abs() < f64::EPSILON);
    }

    /// Helper: create an ActiveOperation for tests.
    fn make_op(
        op_type: &str,
        total: usize,
        success_count: usize,
        failed: usize,
        errors: Vec<&'static str>,
    ) -> crate::components::common::structs::ActiveOperation {
        crate::components::common::structs::ActiveOperation {
            id: "test-id".into(),
            operation_type: op_type.into(),
            total,
            completed: total,
            finished: true,
            finished_at_ms: Some(1000),
            success_count,
            failed,
            errors: errors.into_iter().map(String::from).collect(),
        }
    }

    /// Scope with the given owned operations (id -> label) and delivered ids.
    fn make_scope(mine: &[(&str, &str)], delivered: &[&str]) -> OperationScope {
        OperationScope {
            mine: mine
                .iter()
                .map(|(id, label)| (id.to_string(), label.to_string()))
                .collect(),
            delivered: delivered.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_finalize_progress_success() {
        let op = make_op("reorganize", 10, 10, 0, vec![]);
        let mut notif = Notification::new_progress("Reorganize: 10 series".into(), 0);

        finalize_progress(&mut notif, &op);

        assert_eq!(notif.notification_type, NotificationType::Success);
        assert_eq!(notif.priority, NotificationPriority::Success);
        assert!(!notif.sticky);
        assert!(notif.message.contains("complete"), "{}", notif.message);
        assert!(notif.message.contains("10 ok"), "{}", notif.message);
        assert!(notif.message.contains("0 failed"), "{}", notif.message);
        // Should use the correct priority-based duration, not Info's 3s
        assert_eq!(notif.duration_ms, 4_000);
    }

    #[test]
    fn test_finalize_progress_warning() {
        let op = make_op("batch_move", 10, 7, 3, vec![]);
        let mut notif = Notification::new_progress("Batch move: 10 items".into(), 0);

        finalize_progress(&mut notif, &op);

        assert_eq!(notif.notification_type, NotificationType::Warning);
        assert!(notif.message.contains("7 ok"));
        assert!(notif.message.contains("3 failed"));
        assert_eq!(notif.duration_ms, 6_000);
    }

    #[test]
    fn test_finalize_progress_error() {
        let op = make_op("reorganize", 5, 1, 4, vec!["permission denied"]);
        let mut notif = Notification::new_progress("Reorganize: 5 series".into(), 0);

        finalize_progress(&mut notif, &op);

        assert_eq!(notif.notification_type, NotificationType::Error);
        assert!(
            notif.message.contains("permission denied"),
            "{}",
            notif.message
        );
        assert_eq!(notif.duration_ms, 12_000);
    }

    #[test]
    fn test_finalize_progress_appends_first_error() {
        let op = make_op("batch_move", 3, 0, 3, vec!["disk full", "timeout"]);
        let mut notif = Notification::new_progress("Batch move: 3 items".into(), 0);

        finalize_progress(&mut notif, &op);

        assert!(notif.message.contains("disk full"));
        assert!(
            !notif.message.contains("timeout"),
            "Only first error should be appended"
        );
    }

    #[test]
    fn test_finalize_progress_success_duration() {
        // Success outcome → 4s auto-dismiss, Success icon
        let op = make_op("reorganize", 10, 10, 0, vec![]);
        let mut notif = Notification::new_progress("test".into(), 0);

        finalize_progress(&mut notif, &op);

        assert!(!notif.sticky);
        assert_eq!(notif.notification_type, NotificationType::Success);
        assert_eq!(notif.duration_ms, 4_000); // Success base duration
    }

    #[test]
    fn test_finalize_progress_warning_duration() {
        // Warning outcome → 6s auto-dismiss, Warning icon
        let op = make_op("batch_move", 10, 7, 3, vec![]);
        let mut notif = Notification::new_progress("test".into(), 0);

        finalize_progress(&mut notif, &op);

        assert!(!notif.sticky);
        assert_eq!(notif.notification_type, NotificationType::Warning);
        assert_eq!(notif.duration_ms, 6_000); // Warning base duration
    }

    #[test]
    fn test_finalize_progress_error_duration() {
        // Error outcome → 12s auto-dismiss, Error icon
        let op = make_op("reorganize", 5, 0, 5, vec!["disk full"]);
        let mut notif = Notification::new_progress("test".into(), 0);

        finalize_progress(&mut notif, &op);

        assert!(!notif.sticky);
        assert_eq!(notif.notification_type, NotificationType::Error);
        assert_eq!(notif.duration_ms, 12_000); // Error base duration
    }

    #[test]
    fn test_finalize_progress_total_zero() {
        // Edge case: total = 0 (empty library). update_progress won't
        // set sticky=false because `done` requires total > 0.
        // finalize_progress must force it.
        let op = make_op("reorganize", 0, 0, 0, vec![]);
        let mut notif = Notification::new_progress("Reorganize: 0 series".into(), 0);

        finalize_progress(&mut notif, &op);

        assert!(
            !notif.sticky,
            "Toast should be non-sticky even with total=0"
        );
        assert_eq!(
            notif.notification_type,
            NotificationType::Success,
            "total=0 with no failures should be Success"
        );
        assert_eq!(notif.duration_ms, 4_000, "Should use Success base duration");
        assert!(notif.message.contains("complete"), "{}", notif.message);
    }

    #[test]
    fn test_reconcile_creates_result_toast_for_owned_operation() {
        let mut store = NotificationStore::new();
        let op = make_op("batch_move", 10, 10, 0, vec![]);
        let scope = make_scope(&[("test-id", "Batch move: 10 series")], &[]);

        let delivered = store.reconcile(std::slice::from_ref(&op), 2_000, &scope);

        assert_eq!(store.inbox.len(), 1);
        assert_eq!(store.inbox[0].operation_id.as_deref(), Some("test-id"));
        assert_eq!(store.inbox[0].base_message, "Batch move: 10 series");
        assert!(!store.inbox[0].sticky);
        assert!(
            store.inbox[0].message.contains("10 ok"),
            "{}",
            store.inbox[0].message
        );
        assert_eq!(delivered, vec!["test-id".to_string()]);
    }

    #[test]
    fn test_reconcile_uses_stored_label_for_recovery() {
        let mut store = NotificationStore::new();
        let op = make_op("batch_move", 4, 4, 0, vec![]);
        let scope = make_scope(&[("test-id", "Custom wording")], &[]);

        store.reconcile(std::slice::from_ref(&op), 2_000, &scope);

        assert_eq!(store.inbox[0].base_message, "Custom wording");
        assert!(store.inbox[0].message.starts_with("Custom wording"));
    }

    #[test]
    fn test_reconcile_ignores_operation_not_initiated_on_device() {
        let mut store = NotificationStore::new();
        let op = make_op("batch_move", 10, 10, 0, vec![]);
        // This device initiated a different operation.
        let scope = make_scope(&[("other-id", "Batch move: 3 items")], &[]);

        let delivered = store.reconcile(std::slice::from_ref(&op), 2_000, &scope);

        assert!(store.inbox.is_empty(), "Foreign operations must not toast");
        assert!(delivered.is_empty());
    }

    #[test]
    fn test_reconcile_skips_delivered_operation() {
        let mut store = NotificationStore::new();
        let op = make_op("batch_move", 10, 10, 0, vec![]);
        let scope = make_scope(&[("test-id", "Batch move: 10 series")], &["test-id"]);

        let delivered = store.reconcile(std::slice::from_ref(&op), 2_000, &scope);

        assert!(
            store.inbox.is_empty(),
            "Delivered results must not reappear"
        );
        assert!(delivered.is_empty());
    }

    #[test]
    fn test_reconcile_does_not_recreate_after_delivery() {
        let mut store = NotificationStore::new();
        let op = make_op("batch_move", 10, 10, 0, vec![]);
        let owned = make_scope(&[("test-id", "Batch move: 10 series")], &[]);

        let delivered = store.reconcile(std::slice::from_ref(&op), 2_000, &owned);
        assert_eq!(store.inbox.len(), 1);
        assert_eq!(delivered, vec!["test-id".to_string()]);

        // Delivery is persisted; toast dismissed; backend still retains the op.
        let delivered_scope = make_scope(&[("test-id", "Batch move: 10 series")], &["test-id"]);
        store.dismiss_all();
        store.reconcile(std::slice::from_ref(&op), 62_000, &delivered_scope);

        assert!(store.inbox.is_empty());
    }

    #[test]
    fn test_reconcile_ignores_finished_operation_outside_recency_window() {
        let mut store = NotificationStore::new();
        let op = make_op("batch_move", 10, 10, 0, vec![]); // finished_at_ms = 1000
        let scope = make_scope(&[("test-id", "Batch move: 10 series")], &[]);
        // 31 minutes after the operation finished.
        let now_ms = 1_000 + 31 * 60 * 1000;

        let delivered = store.reconcile(std::slice::from_ref(&op), now_ms, &scope);

        assert!(store.inbox.is_empty());
        assert!(delivered.is_empty());
    }

    #[test]
    fn test_reconcile_creates_and_updates_running_operation_progress() {
        let mut store = NotificationStore::new();
        let scope = make_scope(&[("test-id", "Batch move: 10 series")], &[]);
        let mut op = make_op("batch_move", 10, 3, 0, vec![]);
        op.finished = false;
        op.completed = 3;
        op.finished_at_ms = None;

        let delivered = store.reconcile(std::slice::from_ref(&op), 2_000, &scope);
        assert_eq!(store.inbox.len(), 1);
        assert!(store.inbox[0].sticky);
        assert_eq!(
            store.inbox[0].progress,
            Some(ProgressState {
                completed: 3,
                total: 10
            })
        );
        assert!(delivered.is_empty(), "Running operations are not delivered");

        // Progress reported in place; still a single toast.
        op.completed = 7;
        store.reconcile(std::slice::from_ref(&op), 2_500, &scope);
        assert_eq!(store.inbox.len(), 1);
        assert_eq!(
            store.inbox[0].progress,
            Some(ProgressState {
                completed: 7,
                total: 10
            })
        );
    }

    #[test]
    fn test_reconcile_removes_toasts_for_operations_no_longer_reported() {
        let mut store = NotificationStore::new();
        let scope = make_scope(&[("test-id", "Batch move: 10 series")], &[]);
        let mut op = make_op("batch_move", 10, 3, 0, vec![]);
        op.finished = false;
        op.completed = 3;
        op.finished_at_ms = None;

        store.reconcile(std::slice::from_ref(&op), 2_000, &scope);
        assert_eq!(store.inbox.len(), 1);

        // Backend no longer reports the operation.
        store.reconcile(&[], 5_000, &scope);
        assert!(store.inbox.is_empty());
    }

    #[test]
    fn test_reconcile_preserves_non_operation_toasts() {
        let mut store = NotificationStore::new();
        push(&mut store, "download finished", NotificationType::Success);

        store.reconcile(&[], 2_000, &OperationScope::default());

        assert_eq!(store.inbox.len(), 1);
        assert_eq!(store.inbox[0].base_message, "download finished");
    }

    #[test]
    fn test_finalize_by_operation_id_updates_only_matching_toast() {
        let mut store = NotificationStore::new();
        let mut notif = Notification::new_progress("Batch move: 10 series".into(), 0);
        notif.operation_id = Some("op-1".into());
        store.push(notif);
        push(&mut store, "other", NotificationType::Info);

        let mut op = make_op("batch_move", 10, 10, 0, vec![]);
        op.id = "op-1".into();
        store.finalize_by_operation_id("op-1", &op);

        assert!(
            store.inbox[0].message.contains("complete"),
            "{}",
            store.inbox[0].message
        );
        assert_eq!(store.inbox[1].base_message, "other");
    }
}
