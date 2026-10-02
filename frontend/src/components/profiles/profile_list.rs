use crate::components::common::icons::PlusIcon;
use crate::components::common::table_builder::{ManagedColumn, TableBuilder, TableVariant};
use leptos::prelude::*;

fn default_detail_formatter(cells: &[String]) -> String {
    cells.get(1..).unwrap_or_default().join(" • ")
}

#[component]
pub fn ProfileList(
    title: String,
    add_label: String,
    columns: Vec<&'static str>,
    items: Signal<Vec<(String, Vec<String>)>>,
    on_add: Callback<(), ()>,
    on_remove: Callback<String, ()>,
    on_select: Callback<String, ()>,
    persistence_id: String,
    #[prop(default = default_detail_formatter)] detail_formatter: fn(&[String]) -> String,
) -> impl IntoView {
    let (sort_col, sort_asc, on_sort) =
        crate::hooks::use_persistent_table_state(persistence_id, "0".to_string(), true);
    // Items are (id, Vec<cell_data>) mapped into TableBuilder's ManagedColumn format.
    let table_columns: Vec<ManagedColumn<(String, Vec<String>)>> = columns
        .into_iter()
        .enumerate()
        .map(|(i, col_name)| ManagedColumn {
            id: i.to_string(),
            label: col_name.into(),
            sortable: true,
            class: "".into(),
            cell_render: Callback::new(move |item: (String, Vec<String>)| {
                let cell_text = item.1.get(i).cloned().unwrap_or_default();
                view! { <span>{cell_text}</span> }.into_any()
            }),
        })
        .collect::<Vec<_>>();

    let mut final_columns = table_columns;
    final_columns.push(ManagedColumn {
        id: "actions".into(),
        label: "Actions".into(),
        sortable: false,
        class: "col-actions".into(),
        cell_render: Callback::new(move |item: (String, Vec<String>)| {
            let id = item.0.clone();
            view! {
                <div class="action-cell">
                    <button class="btn-danger btn-sm" on:click=move |e| {
                        e.stop_propagation();
                        on_remove.run(id.clone());
                    }>"×"</button>
                </div>
            }
            .into_any()
        }),
    });

    type CompareProfile = ((String, Vec<String>), (String, Vec<String>), String);
    let compare = Callback::new(|(a, b, col): CompareProfile| {
        if let Ok(idx) = col.parse::<usize>() {
            let val_a = a.1.get(idx).map(|s| s.to_lowercase()).unwrap_or_default();
            let val_b = b.1.get(idx).map(|s| s.to_lowercase()).unwrap_or_default();
            val_a.cmp(&val_b)
        } else {
            std::cmp::Ordering::Equal
        }
    });

    view! {
        <div class="mb-xl">
            <div class="table-container">
                <div class="table-header">
                    <div class="table-title">
                        <h3 class="m-0">{title}</h3>
                    </div>
                    <div class="table-actions flex gap-sm">
                        <button class="btn btn-primary" on:click=move |_| on_add.run(())>
                            <span class="icon"><PlusIcon /></span>
                            {add_label}
                        </button>
                    </div>
                </div>

                {
                    TableBuilder::new(sort_col, sort_asc)
                        .table_variant(TableVariant::Hover)
                        .table_class("unselectable")
                        .loading(Signal::derive(move || false))
                        .empty_message("No profiles configured.")
                        .on_sort(on_sort)
                        .build_managed(
                            items,
                            final_columns,
                            compare,
                            Some(Callback::new(move |item: (String, Vec<String>)| {
                                 on_select.run(item.0);
                            })),
                            None,
                            Some(Callback::new(move |item: (String, Vec<String>)| {
                                let id = item.0.clone();
                                let on_rem = on_remove;
                                let on_sel = on_select;
                                let name = item.1.first().cloned().unwrap_or_default();
                                let details = detail_formatter(&item.1);

                                let id_for_sel = id.clone();
                                let id_for_rem = id.clone();

                                view! {
                                    <div class="card p-md flex justify-between items-center" on:click=move |_| on_sel.run(id_for_sel.clone())>
                                        <div class="flex flex-col gap-xs min-w-0">
                                            <span class="font-bold truncate">{name}</span>
                                            <span class="text-sm text-muted truncate">{details}</span>
                                        </div>
                                        <button class="btn-danger btn-sm" on:click=move |e| {
                                            e.stop_propagation();
                                            on_rem.run(id_for_rem.clone());
                                        }>"×"</button>
                                    </div>
                                }.into_any()
                            }))
                        )
                }
            </div>
        </div>
    }
}
