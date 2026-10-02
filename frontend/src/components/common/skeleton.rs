use leptos::prelude::*;

/// A single shimmering skeleton line. `width` is a CSS width string (e.g. "60%", "10rem").
#[component]
pub fn SkeletonLine(
    #[prop(default = "100%".to_string(), into)] width: String,
    #[prop(default = "skeleton-line".to_string(), into)] class: String,
) -> impl IntoView {
    view! {
        <div class=format!("skeleton {}", class) style=format!("width: {}", width)></div>
    }
}

/// A shimmering block (for input/select placeholders).
#[component]
pub fn SkeletonBlock(
    #[prop(default = "100%".to_string(), into)] width: String,
    #[prop(default = "skeleton-block".to_string(), into)] class: String,
) -> impl IntoView {
    view! {
        <div class=format!("skeleton {}", class) style=format!("width: {}", width)></div>
    }
}

/// A skeleton that mirrors the shape of the Edit Series General tab.
#[component]
pub fn EditSeriesSkeleton() -> impl IntoView {
    view! {
        <div class="skeleton-edit-series">
            // Row 1: Series Title + Path
            <div class="skeleton-form-row">
                <div class="skeleton-form-group">
                    <SkeletonLine width="5rem" class="skeleton-line-sm"/>
                    <SkeletonBlock/>
                </div>
                <div class="skeleton-form-group">
                    <SkeletonLine width="3rem" class="skeleton-line-sm"/>
                    <SkeletonBlock/>
                </div>
            </div>
            // Row 2: Quality Profile + Release Profile + Monitor Mode
            <div class="skeleton-form-row">
                <div class="skeleton-form-group">
                    <SkeletonLine width="8rem" class="skeleton-line-sm"/>
                    <SkeletonBlock/>
                </div>
                <div class="skeleton-form-group">
                    <SkeletonLine width="9rem" class="skeleton-line-sm"/>
                    <SkeletonBlock/>
                </div>
                <div class="skeleton-form-group">
                    <SkeletonLine width="7rem" class="skeleton-line-sm"/>
                    <SkeletonBlock/>
                </div>
            </div>
            // Row 3: Metadata ID + Sync button
            <div class="skeleton-form-row">
                <div class="skeleton-form-group">
                    <SkeletonLine width="6rem" class="skeleton-line-sm"/>
                    <div class="flex gap-sm">
                        <SkeletonBlock class="skeleton-block flex-1"/>
                        <SkeletonBlock width="5rem" class="skeleton-block"/>
                    </div>
                </div>
            </div>
            // Danger zone placeholder
            <div class="skeleton-card" id="edit-series-action-row-placeholder">
                <SkeletonLine width="6rem" class="skeleton-line-sm"/>
                <SkeletonLine width="80%" class="skeleton-line-sm"/>
            </div>
        </div>
    }
}

/// Skeleton for 3 series cards (mobile library loading state).
#[component]
pub fn SeriesCardSkeletons(#[prop(default = 4)] count: usize) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-md mt-md">
            {(0..count).map(|_| view! {
                <div class="skeleton-card">
                    <div class="flex justify-between">
                        <SkeletonLine width="55%" class="skeleton-line-lg"/>
                        <SkeletonLine width="4rem" class="skeleton-line-sm"/>
                    </div>
                    <SkeletonLine width="70%" class="skeleton-line-sm"/>
                </div>
            }).collect_view()}
        </div>
    }
}
