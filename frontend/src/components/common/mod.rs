// Layout & shell
pub mod app_layout;
pub mod header;
pub mod layout;
pub mod sidebar;
pub mod view_header;
pub mod view_shell;

// Modals
pub mod confirmation_modal;
pub mod modal_wrapper;
pub mod standard_modal;

// Pages & landing
pub mod error_page;
pub mod landing;
pub mod placeholder;

// Form & input
pub mod form_fields;
pub mod json_schema_form;
pub mod settings_builder;
pub mod settings_field;
pub mod settings_page;
pub mod sortable_header;
pub mod table_builder;

// UI primitives
pub mod empty_state;
pub mod icons;
pub mod loading_spinner;
pub mod notification_demo;
pub mod search_input;
pub mod skeleton;
pub mod status_badge;
pub mod toast;

// Data & utilities
pub mod structs;

// Plugin & calendar
pub mod calendar_link_modal;
pub mod pagination;
pub mod plugin_form;
pub mod plugin_settings;
pub mod revealable_code;

pub mod formatted_timestamp;

// Re-exports
pub use app_layout::*;
pub use confirmation_modal::*;
pub use empty_state::*;
pub use error_page::*;
pub use form_fields::*;
pub use header::*;
pub use icons::*;
pub use json_schema_form::*;
pub use landing::*;
pub use layout::*;
pub use loading_spinner::*;
// Re-export nested_checkboxes module so existing imports
// (common::nested_checkboxes::{...}) continue to work.
pub use form_fields::nested_checkboxes;
pub use formatted_timestamp::*;
pub use placeholder::*;
pub use revealable_code::*;
pub use search_input::*;
pub use settings_builder::*;
pub use settings_field::*;
pub use settings_page::*;
pub use sidebar::*;
pub use skeleton::*;
pub use sortable_header::*;
pub use status_badge::*;
pub use structs::*;
pub use table_builder::*;
pub use toast::*;
pub use view_header::*;
pub use view_shell::*;
