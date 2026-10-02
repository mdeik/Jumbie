use crate::components::common::form_fields::action_field::ActionField;
use crate::components::common::form_fields::checkbox_input::{CheckboxField, CheckboxInput};
use crate::components::common::form_fields::form_group::FormGroup;
use crate::components::common::form_fields::nested_checkboxes::{
    NestedCheckboxGroup, NestedCheckboxItem,
};
use crate::components::common::form_fields::number_input::NumberInput;
use crate::components::common::form_fields::select::{Select, SelectOption};
use crate::components::common::form_fields::text_input::TextInput;
use crate::components::common::form_fields::textarea::Textarea;
use crate::components::common::form_fields::tooltip::TooltipBuilder;
use crate::components::common::form_fields::tri_state_checkbox::TriStateCheckbox;
use leptos::prelude::*;

/// Renders every form field component with various states (default, disabled,
/// error, forced) so e2e Playwright tests can verify behaviour in isolation.
///
/// Test-relevant elements carry `data-testid="{component-name}-{variant}"`
/// (e.g. `checkbox-default`, `nested-parent`); the Playwright spec selects on
/// these attributes only.
#[component]
pub fn FormFieldsTestPage() -> impl IntoView {
    let text_val: RwSignal<String> = RwSignal::new(String::new());
    let number_val: RwSignal<String> = RwSignal::new("42".to_string());
    let checkbox_val: RwSignal<bool> = RwSignal::new(false);
    let child1_val: RwSignal<bool> = RwSignal::new(false);
    let child2_val: RwSignal<bool> = RwSignal::new(false);
    let nested_parent: RwSignal<bool> = RwSignal::new(false);
    let tri_val: RwSignal<Option<bool>> = RwSignal::new(None);
    let textarea_val: RwSignal<String> = RwSignal::new(String::new());
    let select_val: RwSignal<String> = RwSignal::new("opt1".to_string());

    let select_options = Signal::derive(move || {
        vec![
            SelectOption::from(("opt1".to_string(), "Option One".to_string())),
            SelectOption::from(("opt2".to_string(), "Option Two".to_string())),
            SelectOption::from(("opt3".to_string(), "Option Three".to_string())),
        ]
    });

    let tooltip = TooltipBuilder::new()
        .with_title("Variable Reference")
        .build();

    view! {
        <div class="p-lg max-w-2xl mx-auto" data-testid="form-fields-test-page">
            <h1 class="mb-lg">"Form Fields Test Page"</h1>
            <p class="text-muted mb-lg">
                "This page renders every form field component in isolation. "
                "It is intended for e2e testing and local visual inspection."
            </p>

            <section class="mb-xl" data-testid="section-textinput">
                <h2 class="mb-sm">"TextInput"</h2>
                <TextInput
                    label=String::from("Default text input")
                    value=Signal::derive(move || text_val.get())
                    set_value=Callback::new(move |v| text_val.set(v))
                    placeholder=String::from("Type something...")
                />
                <TextInput
                    label=String::from("Disabled text input")
                    value=Signal::derive(move || "Disabled value".to_string())
                    set_value=Callback::new(|_| {})
                    disabled=Signal::derive(move || true)
                />
                <TextInput
                    label=String::from("Text input with error")
                    value=Signal::derive(move || "Bad value".to_string())
                    set_value=Callback::new(|_| {})
                    error=Signal::derive(move || Some("This field has an error".to_string()))
                />
                <TextInput
                    label=String::from("Password input")
                    value=Signal::derive(move || "secret123".to_string())
                    set_value=Callback::new(|_| {})
                    type_=Signal::derive(move || "password".to_string())
                    show_toggle=true
                />
            </section>

            <section class="mb-xl" data-testid="section-numberinput">
                <h2 class="mb-sm">"NumberInput"</h2>
                <NumberInput
                    label=String::from("Default number input")
                    value=Signal::derive(move || number_val.get())
                    set_value=Callback::new(move |v| number_val.set(v))
                    min=Signal::derive(move || "0".to_string())
                    max=Signal::derive(move || "100".to_string())
                />
                <NumberInput
                    label=String::from("Disabled number input")
                    value=Signal::derive(move || "7".to_string())
                    set_value=Callback::new(|_| {})
                    disabled=Signal::derive(move || true)
                />
            </section>

            <section class="mb-xl" data-testid="section-checkbox">
                <h2 class="mb-sm">"CheckboxInput"</h2>
                <CheckboxInput
                    label=String::from("Default checkbox")
                    checked=Signal::derive(move || checkbox_val.get())
                    set_checked=Callback::new(move |v| checkbox_val.set(v))
                />
                <CheckboxInput
                    label=String::from("Forced checkbox")
                    checked=Signal::derive(move || true)
                    set_checked=Callback::new(|_| {})
                    forced=Signal::derive(move || true)
                />
                <CheckboxInput
                    label=String::from("Disabled checkbox")
                    checked=Signal::derive(move || false)
                    set_checked=Callback::new(|_| {})
                    disabled=Signal::derive(move || true)
                />
                <CheckboxField
                    label=String::from("CheckboxField (settings style)")
                    checked=Signal::derive(move || checkbox_val.get())
                    set_checked=Callback::new(move |v| checkbox_val.set(v))
                    help_text=Some("Settings-style checkbox wrapper".to_string())
                />
            </section>

            <section class="mb-xl" data-testid="section-nested">
                <h2 class="mb-sm">"NestedCheckboxGroup"</h2>
                <NestedCheckboxGroup
                    parent_id="nested-parent"
                    parent_label="Parent checkbox"
                    parent_checked=Signal::derive(move || nested_parent.get())
                    set_parent_checked=nested_parent.write_only()
                    children=vec![
                        NestedCheckboxItem {
                            id: "nested-child1".to_string(),
                            label: "Child 1".to_string(),
                            checked: Signal::derive(move || child1_val.get()),
                            set_checked: child1_val.write_only(),
                        },
                        NestedCheckboxItem {
                            id: "nested-child2".to_string(),
                            label: "Child 2".to_string(),
                            checked: Signal::derive(move || child2_val.get()),
                            set_checked: child2_val.write_only(),
                        },
                    ]
                />
                // Display current signal values to verify no writes occurred when
                // the parent was toggled.
                <div class="mt-sm text-xs text-muted" data-testid="nested-signal-state">
                    "Child1 signal: " {move || child1_val.get().to_string()} ", "
                    "Child2 signal: " {move || child2_val.get().to_string()} ", "
                    "Parent signal: " {move || nested_parent.get().to_string()}
                </div>
            </section>

            <section class="mb-xl" data-testid="section-tristate">
                <h2 class="mb-sm">"TriStateCheckbox"</h2>
                <TriStateCheckbox
                    label="Tri-state example"
                    help_text="Cycles through inherit → On → Off".to_string()
                    value=Signal::derive(move || tri_val.get())
                    set_value=Callback::new(move |v| tri_val.set(v))
                />
            </section>

            <section class="mb-xl" data-testid="section-select">
                <h2 class="mb-sm">"Select"</h2>
                <Select
                    label=String::from("Default select")
                    value=Signal::derive(move || select_val.get())
                    set_value=Callback::new(move |v| select_val.set(v))
                    options=select_options
                    id="test-select"
                />
                <Select
                    label=String::from("Disabled select")
                    value=Signal::derive(move || "opt2".to_string())
                    set_value=Callback::new(|_| {})
                    options=select_options
                    disabled=Signal::derive(move || true)
                />
            </section>

            <section class="mb-xl" data-testid="section-textarea">
                <h2 class="mb-sm">"Textarea"</h2>
                <Textarea
                    label=String::from("Default textarea")
                    value=Signal::derive(move || textarea_val.get())
                    set_value=Callback::new(move |v| textarea_val.set(v))
                    placeholder=String::from("Write something...")
                />
                <Textarea
                    label=String::from("Disabled textarea")
                    value=Signal::derive(move || "Disabled content".to_string())
                    set_value=Callback::new(|_| {})
                    disabled=Signal::derive(move || true)
                />
            </section>

            <section class="mb-xl" data-testid="section-formgroup">
                <h2 class="mb-sm">"FormGroup"</h2>
                <FormGroup
                    label=String::from("Group with help text")
                    help_text=Signal::derive(move || "This is help text".to_string())
                >
                    <p>"Content inside the form group."</p>
                </FormGroup>
                <FormGroup
                    label=String::from("Group with error")
                    error=Signal::derive(move || Some("An error occurred".to_string()))
                >
                    <p>"Group content with an error shown."</p>
                </FormGroup>
            </section>

            <section class="mb-xl" data-testid="section-actionfield">
                <h2 class="mb-sm">"ActionField"</h2>
                <ActionField label=String::from("Action field") help_text=String::from("With an action button")>
                    <button class="btn btn-primary">"Action"</button>
                </ActionField>
            </section>

            <section class="mb-xl" data-testid="section-formatfield">
                <h2 class="mb-sm">"FormatFieldBuilder"</h2>
                <crate::components::common::form_fields::format_field::FormatFieldBuilder
                    id="test-format-field".to_string()
                    label=String::from("Format field")
                    value=Signal::derive(move || text_val.get())
                    on_change=Callback::new(move |v| text_val.set(v))
                    tooltip=TooltipBuilder::new()
                />
            </section>

            <section class="mb-xl" data-testid="section-tooltip">
                <h2 class="mb-sm">"TooltipBuilder"</h2>
                <div class="flex gap-sm">
                    <span>"Hover the question mark: "</span>
                    {tooltip}
                </div>
            </section>
        </div>
    }
}
