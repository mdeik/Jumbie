// Example usage of the toast notification system.

use crate::components::common::toast::{show_error, show_info, show_success, show_warning};
use leptos::prelude::*;

#[component]
pub fn NotificationDemo() -> impl IntoView {
    view! {
        <div class="demo-container p-xl">
            <h2>"Toast Notification Demo"</h2>
            <div class="flex gap-md mt-md">
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        show_success("Operation completed successfully!");
                    }
                >
                    "Show Success"
                </button>

                <button
                    class="btn bg-warning"
                    on:click=move |_| {
                        show_warning("This action requires confirmation");
                    }
                >
                    "Show Warning"
                </button>

                <button
                    class="btn btn-danger"
                    on:click=move |_| {
                        show_error("Failed to save changes");
                    }
                >
                    "Show Error"
                </button>

                <button
                    class="btn btn-secondary"
                    on:click=move |_| {
                        show_info("Background process started");
                    }
                >
                    "Show Info"
                </button>

                <button
                    class="btn"
                    on:click=move |_| {
                        show_success("First notification");
                        show_info("Second notification");
                        show_warning("Third notification");
                    }
                >
                    "Show Multiple"
                </button>

                <button
                    class="btn"
                    on:click=move |_| {
                        show_info("This stays for 10 seconds (priority-based)");
                    }
                >
                    "Long Duration (10s)"
                </button>
            </div>
        </div>
    }
}
