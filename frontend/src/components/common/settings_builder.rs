use crate::components::common::{SettingsPage, Stack};
use leptos::prelude::*;

/// A builder for constructing settings pages with a consistent layout and structure.
pub struct SettingsBuilder {
    id: String,
    header: Option<String>,
    description: Option<String>,
    sections: Vec<SettingsSection>,
}

pub enum SettingsSection {
    Standard {
        title: String,
        description: Option<String>,
        fields: Vec<AnyView>,
    },
    Raw(AnyView),
}

impl SettingsBuilder {
    /// Start a new settings page with the given ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            header: None,
            description: None,
            sections: Vec::new(),
        }
    }

    /// Set an optional header for the entire page.
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }

    /// Set an optional description for the entire page.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a new section to the settings page.
    pub fn section<F>(mut self, title: impl Into<String>, configure: F) -> Self
    where
        F: FnOnce(SettingsSectionBuilder) -> SettingsSectionBuilder,
    {
        let builder = SettingsSectionBuilder {
            title: title.into(),
            description: None,
            fields: Vec::new(),
        };
        let section = configure(builder);
        self.sections.push(SettingsSection::Standard {
            title: section.title,
            description: section.description,
            fields: section.fields,
        });
        self
    }

    /// Add a raw view as a section (e.g. for complex custom components).
    pub fn raw_section(mut self, view: impl IntoView) -> Self {
        self.sections
            .push(SettingsSection::Raw(view.into_any().into_any()));
        self
    }

    /// Build the settings page view.
    pub fn build(self) -> impl IntoView {
        let (id, header) = (self.id, self.header.unwrap_or_default());
        let page_description = self.description;

        view! {
            <SettingsPage id=id header=header>
                <div class="settings-stack">
                    {if let Some(desc) = page_description {
                        view! {
                            <div class="settings-page-description">
                                {desc}
                            </div>
                        }.into_any()
                    } else {
                        view! {}.into_any()
                    }}

                    {self.sections.into_iter().map(|section| {
                        match section {
                            SettingsSection::Standard { title, description, fields } => {
                                view! {
                                    <div class="settings-section">
                                    <h3>{title}</h3>
                                        <Stack>

                                            {if let Some(desc) = description {
                                                view! { <p class="text-muted settings-description">{desc}</p> }.into_any()
                                            } else {
                                                view! {}.into_any()
                                            }}

                                            <div class="settings-fields-grid">
                                                {fields.into_iter().collect_view()}
                                            </div>
                                        </Stack>
                                    </div>
                                }.into_any()
                            },
                            SettingsSection::Raw(view) => view,
                        }
                    }).collect_view()}
                </div>
            </SettingsPage>
        }
    }
}

/// A builder for individual settings sections.
pub struct SettingsSectionBuilder {
    title: String,
    description: Option<String>,
    fields: Vec<AnyView>,
}

impl SettingsSectionBuilder {
    /// Set an optional description for this section.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a field to this section.
    pub fn field(mut self, view: impl IntoView) -> Self {
        self.fields.push(view.into_any().into_any());
        self
    }

    /// Add a custom view to this section (alias for field).
    pub fn custom_view(mut self, view: impl IntoView) -> Self {
        self.fields.push(view.into_any().into_any());
        self
    }
}
