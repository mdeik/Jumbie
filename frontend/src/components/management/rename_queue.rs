use crate::api::{
    fetch_rename_queue, reorganize_all_series_async, reorganize_all_status, reorganize_series,
};
use crate::components::common::SettingsBuilder;
use crate::components::common::form_fields::CheckboxInput;
use crate::components::common::icons::{AlertTriangleIcon, FolderIcon};
use crate::components::common::standard_modal::StandardModal;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use crate::components::common::toast::{NotificationType, show_toast, use_notification};
use crate::hooks::use_config::{ConfigContext, use_config};
use crate::hooks::use_status_polling::RefreshIndicators;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashSet;

#[component]
pub fn SystemRenameQueue() -> impl IntoView {
    let ConfigContext { config, .. } = use_config();
    let platform_os = crate::hooks::use_platform_os();
    let (queue, set_queue) = signal(Option::<jumbie_shared::types::RenameQueueResponse>::None);
    let (is_loading, set_is_loading) = signal(true);

    let (detail, set_detail) = signal(Option::<jumbie_shared::types::RenameQueueItemDetail>::None);
    let (detail_loading, set_detail_loading) = signal(false);
    let show_detail = Signal::derive(move || detail.get().is_some());

    let (auto_apply, set_auto_apply) = signal(false);
    let processing_local: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let (processing_all, set_processing_all) = signal(false);

    // Single check for whether a series is currently being renamed, shared by
    // the table and card renderers.
    let is_processing =
        move |series_id: &str| -> bool { processing_local.get().contains(series_id) };

    spawn_local(async move {
        if let Ok(c) = crate::api::fetch_config().await {
            set_auto_apply.try_update(|a| *a = c.organization.auto_apply_renames);
        }
    });

    let refresh_indicators = use_context::<RefreshIndicators>();
    let refresh_indicators_after = move || {
        if let Some(r) = refresh_indicators {
            r.0.run(());
        }
    };

    let load_queue = move || {
        set_is_loading.set(true);
        spawn_local(async move {
            match fetch_rename_queue().await {
                Ok(q) => {
                    set_queue.try_update(|queue| *queue = Some(q));
                    // Sidebar indicators are refreshed via RefreshIndicators
                    // rather than manually poking individual context signals.
                    // This ensures both RenameQueuePopulated AND RenameQueueFailed
                    // stay in sync through the single fetch_system_status() path.
                    if let Some(r) = refresh_indicators {
                        r.0.run(());
                    }
                }
                Err(e) => show_toast(
                    format!("Failed to load rename queue: {}", e),
                    NotificationType::Error,
                ),
            }
            set_is_loading.try_update(|l| *l = false);
        });
    };

    // Load on every mount — the fetch is cheap enough that rapid mount/unmount
    // cycles (navigating between management sections) aren't a concern.
    load_queue();

    // Reactive re-fetch on manual refresh: the Effect fires once on mount
    // (r.0 = 0), which we skip since load_queue already ran above; later trigger
    // increments always re-fetch.
    if let Some(r) = use_context::<crate::hooks::RenameQueueRefresh>() {
        Effect::new(move |_| {
            if r.0.get() > 0 {
                load_queue();
            }
        });
    }

    let open_detail = move |series_id: String| {
        set_detail_loading.set(true);
        // Clear previous so modal title updates immediately
        set_detail.set(None);
        spawn_local(async move {
            match crate::api::fetch_rename_queue_detail(&series_id).await {
                Ok(d) => {
                    set_detail.try_update(|detail| *detail = Some(d));
                }
                Err(e) => show_toast(
                    format!("Failed to load details: {}", e),
                    NotificationType::Error,
                ),
            }
            set_detail_loading.try_update(|l| *l = false);
        });
    };

    let refresh_rename_queue = use_context::<crate::hooks::RenameQueueRefresh>();

    let apply_rename = move |series_id: String, target_absolute: bool| {
        let refresh = refresh_indicators_after;
        processing_local.update(|set| {
            set.insert(series_id.clone());
        });
        let sid = series_id.clone();
        crate::utils::spawn_api_toast(
            reorganize_series(series_id.clone(), target_absolute),
            Some("Started renaming..."),
            move |_| {
                processing_local.update(|set| {
                    set.remove(&sid);
                });
                crate::components::common::toast::show_success("Renaming completed successfully");
                if let Some(r) = refresh_rename_queue {
                    r.1.run(());
                } else {
                    load_queue();
                }
                refresh();
            },
        );
    };

    let apply_all = move || {
        let refresh = refresh_indicators_after;
        let refresh_queue = refresh_rename_queue.clone();
        let load = load_queue.clone();
        set_processing_all.set(true);
        spawn_local(async move {
            match reorganize_all_series_async().await {
                Ok(result) => {
                    if let Some(task_id) = &result.task_id {
                        let ctx = use_notification();
                        ctx.show_operation_progress(
                            "Reorganize all series".to_string(),
                            task_id.clone(),
                        );
                        let max_polls: u32 = 600; // 5 minutes
                        let mut polls: u32 = 0;
                        loop {
                            if polls >= max_polls {
                                ctx.dismiss_all();
                                show_toast("Reorganization timed out", NotificationType::Error);
                                break;
                            }
                            polls += 1;
                            gloo_timers::future::TimeoutFuture::new(500).await;
                            if let Ok(progress) = reorganize_all_status(task_id).await {
                                // SSoT: The backend's `BatchMoveProgress::mark_finished()`
                                // guarantees completed == total when finished is true, so
                                // we pass the raw values unconditionally.
                                if progress.finished {
                                    // SSoT: transform the progress toast into a result toast
                                    let ctx = use_notification();
                                    let op = crate::components::common::structs::ActiveOperation {
                                        id: task_id.clone(),
                                        operation_type: "reorganize".to_string(),
                                        total: progress.total,
                                        completed: progress.completed,
                                        finished: true,
                                        finished_at_ms: None,
                                        success_count: progress.success_count,
                                        failed: progress.failed,
                                        errors: progress.errors.clone(),
                                    };
                                    ctx.finalize_operation(task_id, &op);
                                    // SSoT: the transformed progress toast is the single
                                    // result notification; a second toast would duplicate it.
                                    break;
                                }
                            }
                        }
                    }
                    set_processing_all.set(false);
                    if let Some(r) = refresh_queue {
                        r.1.run(());
                    } else {
                        load();
                    }
                    refresh();
                }
                Err(e) => {
                    set_processing_all.set(false);
                    show_toast(
                        format!("Failed to start reorganization: {}", e),
                        NotificationType::Error,
                    );
                }
            }
        });
    };

    let toggle_auto_apply = move |checked: bool| {
        let refresh = refresh_indicators_after;
        spawn_local(async move {
            if let Ok(mut c) = crate::api::fetch_config().await {
                c.organization.auto_apply_renames = checked;
                if crate::api::save_config(c).await.is_ok() {
                    set_auto_apply.try_update(|a| *a = checked);
                    if checked {
                        show_toast("Auto-Apply Renames enabled.", NotificationType::Success);
                    } else {
                        show_toast("Auto-Apply Renames disabled.", NotificationType::Success);
                        if let Some(r) = refresh_rename_queue {
                            r.1.run(());
                        } else {
                            load_queue();
                        }
                    }
                    refresh();
                }
            }
        });
    };

    let (sort_col, sort_asc, on_sort) = crate::hooks::use_persistent_table_state(
        "rename_queue".to_string(),
        "series".to_string(),
        true,
    );

    let columns: Vec<ManagedColumn<jumbie_shared::types::RenameQueueItem>> = vec![
        ManagedColumn {
            id: "series".into(),
            label: "Series".into(),
            sortable: true,
            class: "weight-medium".into(),
            cell_render: Callback::new(move |item: jumbie_shared::types::RenameQueueItem| {
                let is_processing = item.processing || is_processing(&item.series_id);
                let title = item.series_title.clone();
                view! {
                    <span class={if is_processing { "opacity-50" } else { "" }}>
                        {if is_processing {
                            view! { <span class="inline-loader me-1"></span> }.into_any()
                        } else {
                            view! {}.into_any()
                        }}
                        {title}
                    </span>
                }
                .into_any()
            }),
        },
        ManagedColumn {
            id: "affected".into(),
            label: "Affected".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |item: jumbie_shared::types::RenameQueueItem| {
                view! {
                    <div class="sys-health-row">
                        <span>{item.affected_episodes}</span>
                        {if item.collision_count > 0 {
                            let count = item.collision_count;
                            let text = if count == 1 { "1 file collision".to_string() } else { format!("{} file collisions", count) };
                            view! { <span class="icon sys-warning-icon-lg" title=text><AlertTriangleIcon /></span> }.into_any()
                        } else { view! {}.into_any() }}
                    </div>
                }.into_any()
            }),
        },
        ManagedColumn {
            id: "causes".into(),
            label: "Causes".into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |item: jumbie_shared::types::RenameQueueItem| {
                view! {
                    <div class="sys-badges-container">
                        <For
                            each=move || item.causes.clone()
                            key=|c| c.clone()
                            children=move |cause| view! { <span class="badge sys-cause-badge">{cause}</span> }
                        />
                    </div>
                }.into_any()
            }),
        },
        ManagedColumn {
            id: "actions".into(),
            label: "Actions".into(),
            sortable: false,
            class: "col-actions".into(),
            cell_render: Callback::new(move |item: jumbie_shared::types::RenameQueueItem| {
                let sid = item.series_id.clone();
                let abs = item.absolute_numbering;
                let is_processing = item.processing || is_processing(&item.series_id);
                view! {
                    <div class="action-cell">
                        <button class="btn btn-sm btn-primary"
                            disabled={is_processing}
                            on:click=move |e| { e.stop_propagation(); if !is_processing { apply_rename(sid.clone(), abs); } }
                        >
                            {if is_processing {
                                view! { <><span class="inline-loader me-1"></span> "Moving…"</> }.into_any()
                            } else {
                                view! { "Apply" }.into_any()
                            }}
                        </button>
                    </div>
                }
                .into_any()
            }),
        },
    ];

    let compare = Callback::new(
        move |(a, b, col): (
            jumbie_shared::types::RenameQueueItem,
            jumbie_shared::types::RenameQueueItem,
            String,
        )| {
            match col.as_str() {
                "series" => a
                    .series_title
                    .to_lowercase()
                    .cmp(&b.series_title.to_lowercase()),
                "affected" => a.affected_episodes.cmp(&b.affected_episodes),
                "causes" => a.causes.len().cmp(&b.causes.len()),
                _ => std::cmp::Ordering::Equal,
            }
        },
    );

    SettingsBuilder::new("rename-queue")
        .raw_section(view! {
            {TableBuilder::new(sort_col, sort_asc)
                .container_class(Signal::derive(move || "table-container unselectable".to_string()))
                .table_variant(TableVariant::Hover)
                .loading(Signal::derive(move || is_loading.get() && queue.get().is_none()))
                .empty_message("No pending renames found. All files are correctly named.")
                .on_sort(on_sort)
                .header_view(view! {
                    <div class="table-header flex justify-between gap-sm mb-sm w-full gap-0">

                        <div class="flex justify-end items-center gap-md" id="auto-apply-renames-container">
                            <CheckboxInput
                                checked=auto_apply
                                set_checked=Callback::new(toggle_auto_apply)
                                label="Auto-Apply Renames"
                            />
                        </div>
                        <div class="table-actions flex gap-sm">
                            {move || if let Some(q) = queue.get() {
                                if !q.items.is_empty() {
                                    view! {
                                        <button id="apply-all-renames-btn" class="btn btn-primary" disabled={processing_all.get()} on:click=move |_| apply_all()>
                                            {if processing_all.get() {
                                                view! { <><span class="inline-loader me-1"></span> "Moving All…"</> }.into_any()
                                            } else {
                                                view! { "Apply All" }.into_any()
                                            }}
                                        </button>
                                    }.into_any()
                                } else { view! {}.into_any() }
                            } else { view! {}.into_any() }}
                        </div>
                    </div>
                }.into_any())
                .build_managed(
                    Signal::derive(move || queue.get().map(|q| q.items.clone()).unwrap_or_default()),
                    columns,
                    compare,
                    Some(Callback::new(move |item: jumbie_shared::types::RenameQueueItem| open_detail(item.series_id))),
                    None,
                    Some(Callback::new(move |item: jumbie_shared::types::RenameQueueItem| {
                        let sid = item.series_id.clone();
                        let sid_rename = item.series_id.clone();
                        let abs = item.absolute_numbering;
                        let is_processing = item.processing || is_processing(&item.series_id);
                        view! {
                            <div class={format!("card p-md bg-secondary border border-radius flex flex-col gap-sm cursor-pointer{}", if is_processing { " opacity-50" } else { "" })} on:click=move |_| if !is_processing { open_detail(sid.clone()); }>
                                <div class="flex justify-between items-center min-w-0">
                                    <strong class="text-base truncate">
                                        {if is_processing {
                                            view! { <span class="inline-loader me-1"></span> }.into_any()
                                        } else {
                                            view! {}.into_any()
                                        }}
                                        {item.series_title}
                                    </strong>
                                    <span class="badge sys-cause-badge">{format!("{} issues", item.causes.len())}</span>
                                </div>
                                <div class="flex justify-between text-sm text-muted">
                                    <span>{format!("{} episodes affected", item.affected_episodes)}</span>
                                    {if item.collision_count > 0 {
                                        view! { <span class="text-danger flex items-center gap-xs"><span class="icon"><AlertTriangleIcon /></span> "Collisions"</span> }.into_any()
                                    } else { view! { <></> }.into_any() }}
                                </div>
                                <button class="btn btn-sm btn-primary mt-xs" disabled={is_processing} on:click=move |e| { e.stop_propagation(); if !is_processing { apply_rename(sid_rename.clone(), abs); } }>
                                    {if is_processing {
                                        view! { <><span class="inline-loader me-1"></span> "Moving…"</> }.into_any()
                                    } else {
                                        view! { "Apply Rename" }.into_any()
                                    }}
                                </button>
                            </div>
                        }.into_any()
                    })),
                )}
        })
        .raw_section(view! {

        <StandardModal
            show=show_detail
            on_close=move |_| set_detail.set(None)
            title=Signal::derive(move || {
                detail.get()
                    .map(|d| format!("{} — Pending Renames", d.series_title))
                    .unwrap_or_else(|| "Pending Renames".to_string())
            })
            size="modal-xxl"
        >
            {move || {
                if detail_loading.get() && detail.get().is_none() {
                    return view! { <div class="empty-state-body"><crate::components::common::loading_spinner::LoadingSpinner /></div> }.into_any();
                }
                if let Some(d) = detail.get() {
                    if d.renames.is_empty() && d.folder_creations.is_empty() && d.folder_deletions.is_empty() {
                        return view! {
                            <div class="empty-state-body">"No files to rename."</div>
                        }.into_any();
                    }

                    let has_folder_changes = !d.folder_creations.is_empty() || !d.folder_deletions.is_empty();
                    // Resolve template variables like `${series}` in paths so the UI
                    // shows readable names. Clone once up front so each closure can
                    // capture its own copy.
                    let st = d.series_title.clone();
                    let st_creations = st.clone();
                    let st_deletions = st.clone();
                    // Shared template resolution: the org illegal-char policy and
                    // SERVER OS come from the same engine the backend uses. Batch paths
                    // are already concrete, so this is a defensive no-op that can't
                    // drift from backend behavior.
                    let org = config.get().map(|c| c.organization).unwrap_or_default();
                    let os = platform_os.get();
                    // Pre-compute resolved paths once per row so columns don't
                    // redundantly call resolve_path_template. The LCS diff
                    // (diff_filename) still runs per-column since AnyView can't be
                    // stored in a Send+Sync struct.
                    #[derive(Clone)]
                    struct PrecomputedRename {
                        original: String,
                        expected: String,
                        has_collision: bool,
                        orig_title: String,
                        exp_title: String,
                    }
                    let precomputed: Vec<PrecomputedRename> = d.renames.iter().map(|r| {
                        let resolved_original = crate::utils::resolve_path_template(&r.original, &st, &org, os);
                        let resolved_expected = crate::utils::resolve_path_template(&r.expected, &st, &org, os);
                        PrecomputedRename {
                            original: r.original.clone(),
                            expected: r.expected.clone(),
                            has_collision: r.has_collision,
                            orig_title: resolved_original,
                            exp_title: resolved_expected,
                        }
                    }).collect();

                    view! {
                        <div class="flex flex-col gap-md">

                            {if has_folder_changes { view! {
                                <div>
                                    <p class="text-sm weight-medium text-muted mb-xs sys-section-header">"Folder Changes"</p>
                                    <div class="sys-changes-list">
                                                        // One For handles creations and deletions; only the
                                                        // CSS class and icon differ.
                                                        {render_folder_changes(&d.folder_creations, &st_creations, &org, os, "add", "+")}
                                                        {render_folder_changes(&d.folder_deletions, &st_deletions, &org, os, "remove", "-")}
                                    </div>
                                </div>
                            }.into_any() } else { view! { <></> }.into_any() }}

                            {if !precomputed.is_empty() {
                                let has_any_collision = precomputed.iter().any(|r| r.has_collision);
                                let pc = precomputed.clone();

                                let mut columns: Vec<ManagedColumn<PrecomputedRename>> = vec![
                                    ManagedColumn {
                                        id: "original".into(), label: "Original".into(), sortable: true, class: "text-sm".into(),
                                        cell_render: Callback::new(move |r: PrecomputedRename| {
                                            let orig_name = std::path::Path::new(&r.orig_title)
                                                .file_name().and_then(|n| n.to_str()).unwrap_or(&r.orig_title).to_string();
                                            let exp_name = std::path::Path::new(&r.exp_title)
                                                .file_name().and_then(|n| n.to_str()).unwrap_or(&r.exp_title).to_string();
                                            let (orig_spans, _) = diff_filename(&orig_name, &exp_name);
                                            view! { <span title=r.orig_title>{orig_spans}</span> }.into_any()
                                        })
                                    },
                                    ManagedColumn {
                                        id: "expected".into(), label: "Expected".into(), sortable: true, class: "text-sm".into(),
                                        cell_render: Callback::new(move |r: PrecomputedRename| {
                                            let orig_name = std::path::Path::new(&r.orig_title)
                                                .file_name().and_then(|n| n.to_str()).unwrap_or(&r.orig_title).to_string();
                                            let exp_name = std::path::Path::new(&r.exp_title)
                                                .file_name().and_then(|n| n.to_str()).unwrap_or(&r.exp_title).to_string();
                                            let (_, exp_spans) = diff_filename(&orig_name, &exp_name);
                                            view! { <span title=r.exp_title>{exp_spans}</span> }.into_any()
                                        })
                                    },
                                ];

                                // Only show the collision column when at least one file has a collision
                                if has_any_collision {
                                    columns.insert(1, ManagedColumn {
                                        id: "collision".into(), label: "Collision".into(), sortable: false, class: "text-center align-middle w-15".into(),
                                        cell_render: Callback::new(move |r: PrecomputedRename| {
                                            if r.has_collision {
                                                let collision_mode = detail.with(|d| d.as_ref().map(|x| x.collision_mode.clone()).unwrap_or_else(|| "rename".to_string()));
                                                view! {
                                                    <span
                                                        class=format!("badge badge-{}", collision_mode.to_lowercase())
                                                        title=format!("Collision handling: {}", collision_mode)
                                                    >
                                                        {collision_mode.to_uppercase()}
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! {}.into_any()
                                            }
                                        })
                                    });
                                }

                                let (sc, _) = signal("original".to_string());
                                let (sa, _) = signal(true);

                                view! {
                                    <div>
                                        <p class="text-sm weight-medium text-muted mb-xs sys-section-header">"File Renames"</p>
                                        {TableBuilder::new(sc.into(), sa.into())
                                            // `rename-files-table` keeps filenames on one line so the
                                            // full name is always visible; without `table-fixed` the
                                            // table grows past the modal and the wrapper scrolls.
                                            .table_class("w-full rename-files-table")
                                            .loading(Signal::derive(move || false))
                                            .empty_message("No files to rename.")
                                            .build_managed(
                                                Signal::derive(move || pc.clone()),
                                                columns,
                                                Callback::new(|(a, b, col): (PrecomputedRename, PrecomputedRename, String)| {
                                                    match col.as_str() {
                                                        "original" => jumbie_shared::formatting::natural_cmp(&a.original, &b.original),
                                                        "expected" => jumbie_shared::formatting::natural_cmp(&a.expected, &b.expected),
                                                        _ => std::cmp::Ordering::Equal,
                                                    }
                                                }),
                                                None::<Callback<PrecomputedRename>>,
                                                None::<Callback<PrecomputedRename, bool>>,
                                                None::<Callback<PrecomputedRename, AnyView>>,
                                            )}
                                    </div>
                                }.into_any()
                            } else { view! { <></> }.into_any() }}

                        </div>
                    }.into_any()
                } else {
                    view! { <></> }.into_any()
                }
            }}
        </StandardModal>
    })
    .build()
}

/// Returns `(orig_view, exp_view)` where each is a list of `<span>` elements:
/// - In `orig_view`: unchanged chars are plain, removed chars are red.
/// - In `exp_view`:  unchanged chars are plain, added chars are green.
fn diff_filename(orig: &str, exp: &str) -> (Vec<AnyView>, Vec<AnyView>) {
    let a: Vec<char> = orig.chars().collect();
    let b: Vec<char> = exp.chars().collect();
    let n = a.len();
    let m = b.len();

    // Build LCS table
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to produce diff ops: 'E' (equal), 'D' (delete from orig), 'I' (insert in exp)
    let mut ops: Vec<(char, char)> = Vec::new(); // (op, char)
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(('E', a[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push(('I', b[j - 1]));
            j -= 1;
        } else {
            ops.push(('D', a[i - 1]));
            i -= 1;
        }
    }
    ops.reverse();

    // Build orig spans: equal chars plain, deleted chars in red
    let mut orig_views: Vec<AnyView> = Vec::new();
    let mut buf = String::new();
    let mut cur_removed = false;
    for (op, ch) in &ops {
        let is_removed = *op == 'D';
        if is_removed != cur_removed {
            if !buf.is_empty() {
                let text = buf.clone();
                if cur_removed {
                    orig_views.push(view! { <span class="text-danger">{text}</span> }.into_any());
                } else {
                    orig_views.push(view! { <span>{text}</span> }.into_any());
                }
                buf.clear();
            }
            cur_removed = is_removed;
        }
        if *op != 'I' {
            buf.push(*ch);
        }
    }
    if !buf.is_empty() {
        let text = buf.clone();
        if cur_removed {
            orig_views.push(view! { <span class="text-danger">{text}</span> }.into_any());
        } else {
            orig_views.push(view! { <span>{text}</span> }.into_any());
        }
    }

    // Build exp spans: equal chars plain, inserted chars in green
    let mut exp_views: Vec<AnyView> = Vec::new();
    let mut buf = String::new();
    let mut cur_added = false;
    for (op, ch) in &ops {
        let is_added = *op == 'I';
        if is_added != cur_added {
            if !buf.is_empty() {
                let text = buf.clone();
                if cur_added {
                    exp_views.push(view! { <span class="text-primary">{text}</span> }.into_any());
                } else {
                    exp_views.push(view! { <span>{text}</span> }.into_any());
                }
                buf.clear();
            }
            cur_added = is_added;
        }
        if *op != 'D' {
            buf.push(*ch);
        }
    }
    if !buf.is_empty() {
        let text = buf;
        if cur_added {
            exp_views.push(view! { <span class="text-primary">{text}</span> }.into_any());
        } else {
            exp_views.push(view! { <span>{text}</span> }.into_any());
        }
    }

    (orig_views, exp_views)
}

/// Render a single folder change row (creation or deletion); extracts the folder
/// name from the resolved template path.
fn render_folder_change(
    path: String,
    series_title: String,
    org: jumbie_shared::config::organization::OrganizationConfig,
    os: jumbie_shared::patterns::PlatformOs,
    variant: String,
    icon: String,
) -> impl IntoView {
    let resolved = crate::utils::resolve_path_template(&path, &series_title, &org, os);
    let folder_name = std::path::Path::new(&resolved)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&resolved)
        .to_string();
    let cls = format!("sys-folder-change-{}", variant);
    let icon_cls = format!("sys-folder-{}-icon", variant);
    let text_cls = format!("sys-folder-{}-text truncate", variant);
    let title_attr = resolved.clone();
    view! {
        <div title=resolved class=cls>
            <span class=icon_cls>{icon} <FolderIcon/></span>
            <span class=text_cls title=title_attr>{folder_name}</span>
        </div>
    }
}

/// Render a list of folder change rows from a slice.
fn render_folder_changes(
    paths: &[String],
    series_title: &str,
    org: &jumbie_shared::config::organization::OrganizationConfig,
    os: jumbie_shared::patterns::PlatformOs,
    variant: &str,
    icon: &str,
) -> impl IntoView {
    let paths = paths.to_vec();
    let st = series_title.to_string();
    let org = org.clone();
    let variant = variant.to_string();
    let icon = icon.to_string();
    view! {
        <For
            each=move || paths.clone()
            key=|p| p.clone()
            children=move |path| render_folder_change(path, st.clone(), org.clone(), os, variant.clone(), icon.clone())
        />
    }
}
