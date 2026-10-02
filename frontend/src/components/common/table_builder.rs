use crate::components::common::icons::{ChevronDownIcon, ChevronUpIcon, EditIcon};
use crate::hooks::use_media_query::use_is_mobile;
use leptos::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// Simple unique ID counter for the mobile sort `<select>` element so that
/// multiple tables on the same page each get a distinct `id` attribute.
static MOBILE_SORT_ID: AtomicU64 = AtomicU64::new(0);

/// Preset table style variants for convenience.
pub enum TableVariant {
    /// Bare table: `w-full` only.
    Default,
    /// Styled table: `table hover w-full text-left`.
    Hover,
}

/// Column definition used by the managed (data-driven) path.
///
/// Created via [`TableBuilder::column_with_render`] or [`TableBuilder::column_render`].
pub struct ManagedColumn<T: Clone + 'static> {
    pub id: String,
    pub label: String,
    pub sortable: bool,
    pub class: String,
    /// Renders the `<td>` content for a single row item.
    pub cell_render: Callback<T, AnyView>,
}

/// Internal column used for the header/sort builder (manual mode).
struct TableColumn {
    id: String,
    label: String,
    sortable: bool,
    class: String,
}

/// Unified builder for data tables — supports both manual and managed modes.
///
/// ## Manual mode
/// Caller owns sort signals and passes raw `tbody_content` to [`TableBuilder::build`].
///
/// ## Managed mode
/// Call [`TableBuilder::build_managed`] — the builder owns sort signals, sorts
/// the data reactively, and iterates rows through `cell_render` callbacks.
/// Requires [`TableBuilder::column_with_render`] instead of [`TableBuilder::column`].
pub struct TableBuilder {
    container_class: Option<Signal<String>>,
    table_class: Option<String>,
    tbody_id: Option<String>,
    columns: Vec<TableColumn>,
    header_view: Option<AnyView>,
    thead_prefix: Option<AnyView>,
    thead_suffix: Option<AnyView>,
    footer_view: Option<AnyView>,
    sort_column: Signal<String>,
    sort_ascending: Signal<bool>,
    on_sort: Option<Callback<String>>,
    on_sort_persist: Option<Callback<(String, bool)>>,
    edit_mode: Option<ReadSignal<bool>>,
    selection_mode: Option<(Signal<bool>, Callback<bool>)>,
    select_all_disabled: Option<Signal<bool>>,
    loading: Option<Signal<bool>>,
    empty_message: Option<String>,
    mobile_view: Option<AnyView>,
    mobile_class: Option<String>,
    preserve_order: bool,
}

struct HeaderRenderParams {
    col_id: String,
    label: String,
    sortable: bool,
    class: String,
    sort_column: Signal<String>,
    sort_ascending: Signal<bool>,
    on_sort: Option<Callback<String>>,
    on_sort_persist: Option<Callback<(String, bool)>>,
}

impl TableBuilder {
    /// Create a new `TableBuilder` with reactive sort state.
    pub fn new(sort_column: Signal<String>, sort_ascending: Signal<bool>) -> Self {
        Self {
            container_class: None,
            table_class: None,
            tbody_id: None,
            columns: Vec::new(),
            header_view: None,
            thead_prefix: None,
            thead_suffix: None,
            footer_view: None,
            sort_column,
            sort_ascending,
            on_sort: None,
            on_sort_persist: None,
            edit_mode: None,
            selection_mode: None,
            select_all_disabled: None,
            loading: None,
            empty_message: None,
            mobile_view: None,
            mobile_class: None,
            preserve_order: false,
        }
    }

    /// Set the container's CSS class (reactive).
    pub fn container_class(mut self, class: Signal<String>) -> Self {
        self.container_class = Some(class);
        self
    }

    /// Set the table element's CSS class directly.
    pub fn table_class(mut self, class: impl Into<String>) -> Self {
        self.table_class = Some(class.into());
        self
    }

    /// Convenience method to set a table style preset instead of a raw class string.
    pub fn table_variant(mut self, variant: TableVariant) -> Self {
        self.table_class = Some(match variant {
            TableVariant::Default => "w-full".to_string(),
            TableVariant::Hover => "table hover w-full text-left".to_string(),
        });
        self
    }

    /// Set the `id` attribute for the `<tbody>` element.
    pub fn tbody_id(mut self, id: impl Into<String>) -> Self {
        self.tbody_id = Some(id.into());
        self
    }

    /// Add a view rendered above the table (e.g., search bar and action buttons).
    pub fn header_view(mut self, view: AnyView) -> Self {
        self.header_view = Some(view);
        self
    }

    /// Add a view rendered inside the `<tr>` of `<thead>` before the columns.
    pub fn thead_prefix(mut self, view: AnyView) -> Self {
        self.thead_prefix = Some(view);
        self
    }

    /// Add a view rendered inside the `<tr>` of `<thead>` after the columns.
    pub fn thead_suffix(mut self, view: AnyView) -> Self {
        self.thead_suffix = Some(view);
        self
    }

    /// Add a view rendered below the table.
    /// When `edit_mode` is set, the footer is only shown while edit mode is active.
    /// When `edit_mode` is not set, the footer is always rendered.
    pub fn footer_view(mut self, view: AnyView) -> Self {
        self.footer_view = Some(view);
        self
    }

    /// Add a header-only column (manual mode). `sortable = false` means the header
    /// won't trigger `on_sort`.
    pub fn column(self, id: impl Into<String>, label: impl Into<String>, sortable: bool) -> Self {
        self.column_with_class(id, label, sortable, "")
    }

    /// Add a header-only column with a custom CSS class applied to its `<th>` cell.
    pub fn column_with_class(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        sortable: bool,
        class: impl Into<String>,
    ) -> Self {
        self.columns.push(TableColumn {
            id: id.into(),
            label: label.into(),
            sortable,
            class: class.into(),
        });
        self
    }

    /// Callback executed when a sortable column header is clicked.
    pub fn on_sort(mut self, callback: Callback<String>) -> Self {
        self.on_sort = Some(callback);
        self
    }

    /// Optional persist callback fired after every sort click with `(new_col, new_asc)`.
    /// Use this to save sort state to config without duplicating `spawn_local` boilerplate.
    pub fn on_sort_persist(mut self, persist: Callback<(String, bool)>) -> Self {
        self.on_sort_persist = Some(persist);
        self
    }

    /// Enable edit mode integration.
    /// - The `mb-footer` class is automatically added to the container while active.
    /// - The `selection_mode` checkbox column and `footer_view` are gated on this signal.
    pub fn edit_mode(mut self, is_edit_mode: ReadSignal<bool>) -> Self {
        self.edit_mode = Some(is_edit_mode);
        self
    }

    /// Enable a built-in "Select All" checkbox in the `<thead>`.
    /// When `edit_mode` is set, the checkbox is only shown while edit mode is active.
    /// `(are_all_selected, on_select_all)` — the signal drives the checkbox `checked` state,
    /// the callback receives the new checked bool when the user clicks.
    pub fn selection_mode(
        mut self,
        are_all_selected: Signal<bool>,
        on_select_all: Callback<bool>,
    ) -> Self {
        self.selection_mode = Some((are_all_selected, on_select_all));
        self
    }

    /// Disable the built-in "Select All" checkbox while this signal is `true`
    /// (e.g. when the filtered list is empty).
    pub fn select_all_disabled(mut self, disabled: Signal<bool>) -> Self {
        self.select_all_disabled = Some(disabled);
        self
    }

    /// Show a loading row when this signal is `true` and the tbody would otherwise be empty.
    pub fn loading(mut self, is_loading: Signal<bool>) -> Self {
        self.loading = Some(is_loading);
        self
    }

    /// Show an empty-state row with this message when loading is done and there are no rows.
    pub fn empty_message(mut self, msg: impl Into<String>) -> Self {
        self.empty_message = Some(msg.into());
        self
    }

    /// When set, the table skips client-side sorting and preserves the order
    /// returned by the backend. Use this for server-paginated tables where
    /// the backend is the single source of truth for ordering.
    ///
    /// The `compare` callback passed to `build_managed` is ignored when this is set.
    pub fn preserve_order(mut self) -> Self {
        self.preserve_order = true;
        self
    }

    /// Add a view rendered only on mobile (below the table if not hidden, or replacing it).
    pub fn mobile_view(mut self, view: AnyView) -> Self {
        self.mobile_view = Some(view);
        self
    }

    /// Override the CSS class of the mobile-only section wrapper.
    /// Defaults to `"mobile-only mt-md"`.
    pub fn mobile_class(mut self, class: impl Into<String>) -> Self {
        self.mobile_class = Some(class.into());
        self
    }

    /// Standardised Edit / Cancel toggle button.
    pub fn render_edit_button(is_edit_mode: ReadSignal<bool>, on_toggle: Callback<()>) -> AnyView {
        view! {
            <button
                class=move || if is_edit_mode.get() { "btn btn-secondary active" } else { "btn btn-primary" }
                on:click=move |_| on_toggle.run(())
            >
                <span class="icon"><EditIcon /></span>
                {move || if is_edit_mode.get() { "Cancel Edit" } else { "Edit" }}
            </button>
        }.into_any()
    }

    /// Row checkbox cell — **toggle** model (matches `Manage Series Files`).
    ///
    /// - `edit_mode_gate`: if `Some`, the cell is hidden when edit mode is off.
    ///   Pass `None` to always show the checkbox.
    /// - `is_selected`: drives the `checked` prop reactively.
    /// - `on_toggle`: called (with no args) when the checkbox changes or the row is clicked.
    ///   Hook the same callback to both the `<tr on:click>` and this cell.
    pub fn render_selection_cell(
        edit_mode_gate: Option<ReadSignal<bool>>,
        is_selected: Signal<bool>,
        on_toggle: Callback<()>,
    ) -> AnyView {
        view! {
            {move || {
                let show = edit_mode_gate.map(|m| m.get()).unwrap_or(true);
                if show {
                    Some(view! {
                        <td class="col-checkbox text-center align-middle">
                            <input
                                type="checkbox"
                                class="table-checkbox mx-auto"
                                prop:checked=move || is_selected.get()
                                on:click=move |ev| ev.stop_propagation()
                                on:change=move |_| on_toggle.run(())
                            />
                        </td>
                    })
                } else {
                    None
                }
            }}
        }
        .into_any()
    }

    fn render_header(params: HeaderRenderParams) -> AnyView {
        let HeaderRenderParams {
            col_id,
            label,
            sortable,
            class,
            sort_column,
            sort_ascending,
            on_sort,
            on_sort_persist,
        } = params;

        let col_id_str = col_id.clone();
        let th_class = if class.is_empty() {
            if sortable {
                "cursor-pointer".to_string()
            } else {
                "".to_string()
            }
        } else if sortable {
            format!("{} cursor-pointer", class)
        } else {
            class.clone()
        };

        if sortable {
            view! {
                <th class=th_class on:click=move |_| {
                    let new_asc = if sort_column.get() == col_id_str {
                        !sort_ascending.get()
                    } else {
                        true
                    };
                    let new_col = col_id_str.clone();

                    if let Some(cb) = on_sort { cb.run(new_col.clone()); }

                    if let Some(cb) = on_sort_persist { cb.run((new_col, new_asc)); }
                }>
                    <div class="flex items-center gap-xs">
                        <span>{label}</span>
                        {
                            let col = col_id.clone();
                            move || {
                                if sort_column.get() == col {
                                    if sort_ascending.get() {
                                        view! {
                                            <span class="icon text-xs transition-transform">
                                                <ChevronUpIcon />
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <span class="icon text-xs transition-transform">
                                                <ChevronDownIcon />
                                            </span>
                                        }.into_any()
                                    }
                                } else {
                                    view! {
                                        <span class="icon text-xs transition-transform" style="opacity: 0; pointer-events: none;">
                                            <ChevronDownIcon />
                                        </span>
                                    }.into_any()
                                }
                            }
                        }
                    </div>
                </th>
            }.into_any()
        } else {
            view! {
                <th class=th_class>
                    <div class="flex items-center gap-xs">
                        <span>{label}</span>
                    </div>
                </th>
            }
            .into_any()
        }
    }

    fn build_shell(self, tbody_final: AnyView) -> AnyView {
        let table_class = self.table_class.unwrap_or_else(|| "w-full".to_string());

        let sort_column = self.sort_column;
        let sort_ascending = self.sort_ascending;
        let on_sort = self.on_sort;
        let on_sort_persist = self.on_sort_persist;
        let selection_mode = self.selection_mode;
        let select_all_disabled = self.select_all_disabled;
        let edit_mode = self.edit_mode;

        let container_ref = NodeRef::<leptos::html::Div>::new();
        if let Some(loading) = self.loading {
            let last_height = StoredValue::new(0.0f64);
            Effect::new(move || {
                let is_loading = loading.get();
                if let Some(el) = container_ref.get() {
                    use wasm_bindgen::JsCast;
                    let html_el = el.unchecked_ref::<web_sys::HtmlElement>();
                    if is_loading {
                        let prev_height = last_height.get_value();
                        if prev_height > 0.0 {
                            let _ = html_el
                                .style()
                                .set_property("min-height", &format!("{}px", prev_height));
                        }
                    } else {
                        let _ = html_el.style().remove_property("min-height");
                        let el_clone = el.clone();
                        leptos::task::spawn_local(async move {
                            let rect = el_clone
                                .unchecked_ref::<web_sys::Element>()
                                .get_bounding_client_rect();
                            let height = rect.height();
                            if height > 0.0 {
                                last_height.set_value(height);
                            }
                        });
                    }
                }
            });
        }

        let selection_col_count = if selection_mode.is_some() { 1 } else { 0 };
        let total_cols = selection_col_count + self.columns.len();

        let thead_content = view! {
            <tr>
                {
                    if let Some((are_all_selected, on_select_all)) = selection_mode {
                        let is_editing = edit_mode;
                        view! {
                            {move || if is_editing.map(|m| m.get()).unwrap_or(true) {
                                view! {
                                    <th class="col-checkbox text-center">
                                        <input type="checkbox"
                                            class="table-checkbox mx-auto"
                                            prop:checked=are_all_selected
                                            prop:disabled=move || select_all_disabled.map(|s| s.get()).unwrap_or(false)
                                            on:change=move |ev| {
                                                on_select_all.run(event_target_checked(&ev));
                                            }
                                        />
                                    </th>
                                }.into_any()
                            } else {
                                view! {}.into_any()
                            }}
                        }.into_any()
                    } else { view! {}.into_any() }
                }
                {self.thead_prefix}
                {
                    self.columns.into_iter().map(|col| {
                        Self::render_header(HeaderRenderParams {
                            col_id: col.id,
                            label: col.label,
                            sortable: col.sortable,
                            class: col.class,
                            sort_column,
                            sort_ascending,
                            on_sort,
                            on_sort_persist,
                        })
                    }).collect_view()
                }
                {self.thead_suffix}
            </tr>
        };

        let loading_sig = self.loading;
        let empty_message = self.empty_message;
        let empty_message_mobile = empty_message.clone();

        let sentinel_view = view! {
            {move || {
                let is_loading = loading_sig.map(|s| s.get()).unwrap_or(false);
                if is_loading {
                    return view! {
                        <tr>
                            <td colspan=total_cols class="empty-state-body">
                                <crate::components::common::loading_spinner::LoadingSpinner />
                            </td>
                        </tr>
                    }.into_any();
                } else if let Some(msg) = empty_message.clone() {
                    return view! {
                        <tr class="table-empty-sentinel">
                            <td colspan=total_cols class="empty-state-body sys-placeholder-box">
                                <crate::components::common::empty_state::EmptyState message=msg />
                            </td>
                        </tr>
                    }.into_any();
                }
                view! {}.into_any()
            }}
        };
        let tbody_final = std::cell::RefCell::new(Some(tbody_final));

        let table_tbody = view! {
            {move || tbody_final.borrow_mut().take()}
        };

        let mobile_sentinel = view! {
            {move || {
                let is_loading = loading_sig.map(|s| s.get()).unwrap_or(false);
                if is_loading {
                    return view! {
                        <div class="empty-state-body">
                            <crate::components::common::loading_spinner::LoadingSpinner />
                        </div>
                    }.into_any();
                } else if let Some(msg) = empty_message_mobile.clone() {
                    return view! {
                        <div class="table-empty-sentinel empty-state-body sys-placeholder-box">
                            <crate::components::common::empty_state::EmptyState message=msg />
                        </div>
                    }.into_any();
                }
                view! {}.into_any()
            }}
        };

        let table_view = view! {
            <div class="table-responsive">
                <table class=table_class>
                    <thead>
                        {thead_content}
                    </thead>
                    <tbody id=self.tbody_id.unwrap_or_default()>
                        {sentinel_view}
                        {table_tbody}
                    </tbody>
                </table>
            </div>
        };

        let has_mobile_view = self.mobile_view.is_some();
        let mobile_class = self
            .mobile_class
            .clone()
            .unwrap_or_else(|| "mobile-only mt-md".to_string());
        let mobile_section = if let Some(mv) = self.mobile_view {
            view! {
                <div class=mobile_class>
                    {mv}
                    {mobile_sentinel}
                </div>
            }
            .into_any()
        } else {
            view! {}.into_any()
        };

        let footer_view = self.footer_view;
        let footer_el = if let Some(footer) = footer_view {
            let em = edit_mode;
            view! {
                <div class=move || {
                    let show = em.map(|m| m.get()).unwrap_or(true);
                    if show { "" } else { "hidden" }
                }>
                    {footer}
                </div>
            }
            .into_any()
        } else {
            view! {}.into_any()
        };

        match self.container_class {
            Some(container_class) => view! {
                <div class=move || {
                    let mut base = container_class.get();
                    if let Some(em) = edit_mode
                        && em.get() && !base.contains("mb-footer") {
                            base.push_str(" mb-footer");
                        }
                    base
                } node_ref=container_ref>
                    {self.header_view}
                    <div class=move || if has_mobile_view { "desktop-only" } else { "" }>
                        {table_view}
                    </div>
                    {mobile_section}
                    {footer_el}
                </div>
            }
            .into_any(),
            None => view! {
                {self.header_view}
                <div class=move || if has_mobile_view { "desktop-only" } else { "" }>
                    {table_view}
                </div>
                {mobile_section}
                {footer_el}
            }
            .into_any(),
        }
    }

    /// Build and render the complete table, wrapping the provided `tbody_content`.
    ///
    /// Use this when you need full control over row rendering (selection cells,
    /// complex per-row state, etc.).
    pub fn build(self, tbody_content: impl IntoView) -> AnyView {
        self.build_shell(tbody_content.into_any())
    }

    /// Build and render the table in **managed mode**.
    ///
    /// The builder owns sort signals (passed to [`TableBuilder::new`]) and takes a
    /// set of typed column descriptors that each include a `cell_render` callback.
    /// Data is sorted reactively; rows are rendered automatically.
    ///
    /// Optionally provide a `card_view` callback to render a mobile card layout.
    ///
    /// # Arguments
    /// - `data` — reactive signal with the full, unsorted item list
    /// - `managed_columns` — ordered list of columns including their `cell_render`
    /// - `compare` — sort comparator `(a, b, column_id) → Ordering`
    /// - `on_row_click` — optional row click handler
    /// - `card_view` — optional mobile card renderer for each item
    pub fn build_managed<T: Clone + Send + Sync + 'static>(
        self,
        data: Signal<Vec<T>>,
        managed_columns: Vec<ManagedColumn<T>>,
        compare: Callback<(T, T, String), std::cmp::Ordering>,
        on_row_click: Option<Callback<T>>,
        is_selected: Option<Callback<T, bool>>,
        card_view: Option<Callback<T, AnyView>>,
    ) -> AnyView {
        let sort_column = self.sort_column;
        let sort_ascending = self.sort_ascending;

        let sorted_data = if self.preserve_order {
            // Server-side sorting — just clone the data as-is
            Signal::derive(move || data.with(|d| d.clone()))
        } else {
            Signal::derive(move || {
                let mut items = data.with(|d| d.clone());
                let col = sort_column.get();
                let asc = sort_ascending.get();
                items.sort_by(|a: &T, b: &T| {
                    let ord = compare.run((a.clone(), b.clone(), col.clone()));
                    if asc { ord } else { ord.reverse() }
                });
                items
            })
        };

        let cell_renders: Vec<_> = managed_columns.iter().map(|c| c.cell_render).collect();
        let col_classes: Vec<_> = managed_columns.iter().map(|c| c.class.clone()).collect();
        let selection_mode = self.selection_mode;
        let edit_mode = self.edit_mode;
        // Capture sort callbacks before self is moved into column_batch.
        let on_sort_persist = self.on_sort_persist;
        let on_sort = self.on_sort;

        let mut builder = self.column_batch(
            managed_columns
                .iter()
                .map(|c| TableColumn {
                    id: c.id.clone(),
                    label: c.label.clone(),
                    sortable: c.sortable,
                    class: c.class.clone(),
                })
                .collect(),
        );

        // Mobile card view (if provided)
        // On desktop, the card list is hidden via CSS but Leptos still evaluates
        // the reactive closures inside.  We add a `use_is_mobile()` guard so
        // that the card rendering only runs on mobile — eliminating ~50 card
        // `cv.run()` calls on every data change for desktop users.
        if let Some(cv) = card_view {
            let is_mobile = use_is_mobile();
            let sorted_data = sorted_data.clone();

            let sortable_cols: Vec<(String, String)> = managed_columns
                .iter()
                .filter(|c| c.sortable)
                .map(|c| (c.id.clone(), c.label.clone()))
                .collect();
            let has_multiple = sortable_cols.len() > 1;

            let sort_col = sort_column;
            let sort_asc = sort_ascending;

            let sort_toolbar = if !sortable_cols.is_empty() {
                // Stable unique ID for the sort select (generated once, not per re-render).
                let select_id = format!(
                    "mobile-sort-select-{}",
                    MOBILE_SORT_ID.fetch_add(1, Ordering::Relaxed)
                );
                // Use a tuple of signals so the closures can read them reactively.
                let sort_col_for_move = sort_col.clone();
                view! {
                    <div class="mobile-sort-bar flex items-center gap-sm p-sm">
                        <span class="text-sm font-medium whitespace-nowrap">"Sort:"</span>
                        {move || {
                            if has_multiple {
                                let current = sort_col_for_move.get();
                                view! {
                                    <select id=select_id.clone() class="strict-select flex-1"
                                        prop:value=current.clone()
                                        on:change=move |ev| {
                                            let new_col = event_target_value(&ev);
                                            if new_col == current { return; }
                                            if let Some(cb) = on_sort_persist {
                                                // Preserve current direction when switching columns.
                                                cb.run((new_col, sort_asc.get_untracked()));
                                            } else if let Some(cb) = on_sort {
                                                cb.run(new_col);
                                            }
                                        }
                                    >
                                        {sortable_cols.iter().map(|(id, label)| {
                                            let id = id.clone();
                                            let label = label.clone();
                                            view! {
                                                <option value=id.clone()>{label}</option>
                                            }
                                        }).collect_view()}
                                    </select>
                                }.into_any()
                            } else {
                                // Single sortable column — just show the label, no dropdown.
                                let label = sortable_cols.first().map(|(_, l)| l.clone()).unwrap_or_default();
                                view! {
                                    <span class="text-sm text-muted flex-1">{label}</span>
                                }.into_any()
                            }
                        }}
                        // Asc / Desc toggle
                        <button
                            class="btn btn-sm btn-ghost btn-icon"
                            title=move || if sort_asc.get() { "Ascending" } else { "Descending" }
                            on:click=move |_| {
                                let current_col = sort_col.get_untracked();
                                if current_col.is_empty() { return; }
                                if let Some(cb) = on_sort_persist {
                                    cb.run((current_col, !sort_asc.get_untracked()));
                                } else if let Some(cb) = on_sort {
                                    cb.run(current_col);
                                }
                            }
                        >
                            {move || if sort_asc.get() {
                                view! { <span class="icon"><ChevronUpIcon /></span> }.into_any()
                            } else {
                                view! { <span class="icon"><ChevronDownIcon /></span> }.into_any()
                            }}
                        </button>
                    </div>
                }.into_any()
            } else {
                view! {}.into_any()
            };

            builder = builder.mobile_view(
                view! {
                    <div class="flex flex-col gap-md">
                        {sort_toolbar}
                        <div class="card-list flex flex-col gap-md">
                            {move || {
                                if !is_mobile.get() {
                                    return view! {}.into_any();
                                }
                                let items = sorted_data.get();
                                items.into_iter().map(|item| cv.run(item)).collect_view().into_any()
                            }}
                        </div>
                    </div>
                }
                .into_any(),
            );
        }

        let desktop_rows = view! {
            {move || {
                let rows = sorted_data.with(|d| d.clone());
                let renders = cell_renders.clone();
                rows.into_iter().map(|item: T| {
                    let item_for_click = item.clone();
                    let item_for_cells = item.clone();
                    let item_for_selection = item.clone();
                    view! {
                        <tr on:click=move |_| {
                            if let Some(cb) = on_row_click {
                                cb.run(item_for_click.clone());
                            }
                        }>
                            {
                                if selection_mode.is_some() {
                                    let is_editing = edit_mode;
                                    let is_sel = is_selected;
                                    let item_for_sel = item_for_selection.clone();
                                    let on_row_click_cb = on_row_click;
                                    let item_for_toggle = item_for_selection.clone();

                                    Self::render_selection_cell(
                                        is_editing,
                                        Signal::derive(move || is_sel.map(|cb| cb.run(item_for_sel.clone())).unwrap_or(false)),
                                        Callback::new(move |()| {
                                            if let Some(cb) = on_row_click_cb {
                                                cb.run(item_for_toggle.clone());
                                            }
                                        })
                                    )
                                } else {
                                    view! {}.into_any()
                                }
                            }
                            {
                                renders.iter().zip(col_classes.iter()).map(|(render, class)| {
                                    let class = class.clone();
                                    view! { <td class=class>{render.run(item_for_cells.clone())}</td> }
                                }).collect_view()
                            }
                        </tr>
                    }
                }).collect_view()
            }}
        }.into_any();

        view! {
            <div class="managed-table-wrapper">
                {builder.build_shell(desktop_rows)}
            </div>
        }
        .into_any()
    }

    /// Internal: bulk-replace built column list (used by `build_managed`).
    fn column_batch(mut self, cols: Vec<TableColumn>) -> Self {
        self.columns = cols;
        self
    }
}
