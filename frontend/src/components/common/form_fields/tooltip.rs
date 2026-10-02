use jumbie_shared::variables::{
    TemplateContext, get_formatting_instructions, get_formatting_instructions_for_context,
    get_sections_for_context,
};
use leptos::prelude::*;

/// Known contexts for static help tooltips (non-template-variable content).
/// Add new variants here rather than duplicating the builder chain at call sites.
#[derive(Clone, Copy)]
pub enum HelpContext {
    /// Explains what aliases do and the `@instance-id:alias` prefix feature
    /// used in alias textareas.
    SourceAlias,
    /// Explains how regex patterns gate matching, the `@instance-id:pattern`
    /// prefix, and named capture group extraction for pattern textareas.
    SourcePattern,
}

/// A segment of tooltip text that may contain inline [`TooltipText::Var`] spans
/// rendered with the `.tooltip-var` CSS class (e.g. for highlighting variable
/// syntax inline within a description paragraph).
#[derive(Clone)]
pub enum TooltipText {
    Plain(&'static str),
    Var(&'static str),
}

#[derive(Clone)]
enum BodyItem {
    /// Renders as `<ul class="tooltip-list"><li>{parts}</li></ul>`
    List(Vec<TooltipText>),
    /// Renders as `<div class="tooltip-section-title mt-sm">`. Inner parts can carry inline Var spans.
    SectionTitle(Vec<TooltipText>),
    /// Renders as `<div class="tooltip-desc">`. Inner parts can carry inline Var spans.
    Desc(Vec<TooltipText>),
    /// Renders as `<div class="tooltip-example mt-xs">`. Inner parts can carry inline Var spans.
    Example(Vec<TooltipText>),
    /// A heading with a short description beneath, from owned text. Used by
    /// normal (non-help) tooltips whose content is runtime data.
    Section {
        heading: String,
        description: String,
    },
    /// A plain description line from owned text (no heading).
    Text(String),
}

#[derive(Clone, Default)]
pub struct TooltipBuilder {
    title: Option<&'static str>,
    sections: Vec<(&'static str, Vec<(String, &'static str)>)>,
    show_formatting: bool,
    /// Context the formatting instructions are restricted to (e.g. search
    /// templates show padding only). `None` falls back to the full list.
    formatting_context: Option<TemplateContext>,
    tooltip_auto_down: bool,
    body_items: Vec<BodyItem>,
}

fn render_tooltip_text(parts: Vec<TooltipText>) -> Vec<AnyView> {
    parts
        .into_iter()
        .map(|part| match part {
            TooltipText::Plain(s) => view! { {s} }.into_any(),
            TooltipText::Var(s) => view! { <span class="tooltip-var">{s}</span> }.into_any(),
        })
        .collect()
}

impl TooltipBuilder {
    pub fn new() -> Self {
        Self {
            title: Some("Variable Reference"),
            sections: Vec::new(),
            show_formatting: false,
            formatting_context: None,
            tooltip_auto_down: false,
            body_items: Vec::new(),
        }
    }

    pub fn with_title(mut self, title: &'static str) -> Self {
        self.title = Some(title);
        self
    }

    /// Drop the default title so the tooltip shows body content only. Use for
    /// normal tooltips whose anchor already conveys the subject.
    pub fn without_title(mut self) -> Self {
        self.title = None;
        self
    }

    pub fn with_context(mut self, context: TemplateContext) -> Self {
        let shared_sections = get_sections_for_context(&context);
        for section in shared_sections {
            let vars = section
                .variables
                .into_iter()
                .map(|v| (v.key.to_string(), v.description))
                .collect::<Vec<_>>();
            if !vars.is_empty() {
                self.sections.push((section.title, vars));
            }
        }
        self.formatting_context = Some(context);
        self
    }

    pub fn with_formatting(mut self) -> Self {
        self.show_formatting = true;
        self
    }

    /// Add the `.tooltip-auto-down` CSS class to the outer tooltip host span.
    pub fn with_auto_down(mut self) -> Self {
        self.tooltip_auto_down = true;
        self
    }

    /// Append a bare list item rendered as
    /// `<ul class="tooltip-list"><li>{parts}</li></ul>`.
    pub fn with_list_item(mut self, parts: Vec<TooltipText>) -> Self {
        self.body_items.push(BodyItem::List(parts));
        self
    }

    /// Append a section title rendered as
    /// `<div class="tooltip-section-title mt-sm">{parts}</div>`.
    pub fn with_section_title(mut self, parts: Vec<TooltipText>) -> Self {
        self.body_items.push(BodyItem::SectionTitle(parts));
        self
    }

    /// Append a description paragraph rendered as
    /// `<div class="tooltip-desc">{parts}</div>`.
    pub fn with_desc(mut self, parts: Vec<TooltipText>) -> Self {
        self.body_items.push(BodyItem::Desc(parts));
        self
    }

    /// Append an example block rendered as
    /// `<div class="tooltip-example mt-xs">{parts}</div>`.
    pub fn with_example(mut self, parts: Vec<TooltipText>) -> Self {
        self.body_items.push(BodyItem::Example(parts));
        self
    }

    /// Append a section — a heading with a short description beneath — from
    /// owned text. This is the dynamic-text entry point for normal tooltips
    /// (help tooltips use [`with_section_title`](Self::with_section_title) and
    /// [`with_desc`](Self::with_desc) with static text).
    pub fn with_section(
        mut self,
        heading: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.body_items.push(BodyItem::Section {
            heading: heading.into(),
            description: description.into(),
        });
        self
    }

    /// Append a plain description line from owned text (dynamic tooltips).
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.body_items.push(BodyItem::Text(text.into()));
        self
    }

    /// Populate the tooltip from a [`HelpContext`]. Each variant sets the title,
    /// auto-down positioning, and all body items in one call.
    ///
    /// Layout convention: each tooltip opens with a "How it works" section
    /// describing behavior, then moves to concrete how-to details and examples.
    pub fn with_help_context(mut self, ctx: HelpContext) -> Self {
        match ctx {
            HelpContext::SourceAlias => {
                self.title = Some("Aliases");
                self.tooltip_auto_down = true;

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "What aliases do",
                    )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "Alternate titles the series may be released under. They set which \
                         titles are searched for on sources, and releases found by polling \
                         are matched against them when no regex patterns are set.",
                )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "Matching is case-insensitive and treats dots and spaces the same.",
                )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "When aliases are set they replace the series title in searches; \
                         leave empty to use the series title. On a season, aliases \
                         override the series-level aliases for that season's searches.",
                )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "Once regex patterns are set, they take over matching — releases \
                         found by polling must match a pattern even if they match an alias.",
                )]));

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "Restrict to one source",
                    )]));
                self.body_items
                    .push(BodyItem::List(vec![TooltipText::Var("@instance-id:alias")]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Prepend "),
                    TooltipText::Var("@instance-id:"),
                    TooltipText::Plain(" to restrict this alias to a specific source plugin."),
                ]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain(
                        "The instance ID is shown in the plugin modal subtitle \
                         when editing a plugin (e.g., ",
                    ),
                    TooltipText::Var("a3f8c91e4b2d"),
                    TooltipText::Plain(")."),
                ]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "If no plugin has that ID, the prefixed alias is skipped entirely.",
                )]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("@a3f8c91e4b2d:Test Series"),
                    TooltipText::Plain(" → only that plugin instance will search for "),
                    TooltipText::Var("Test Series"),
                ]));
            }
            HelpContext::SourcePattern => {
                self.title = Some("Regex Patterns");
                self.tooltip_auto_down = true;

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "How patterns work",
                    )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "Patterns only gate releases matched automatically (polling and \
                         auto-search); they never change what is searched for and manual \
                         searches are unaffected.",
                )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "They run against the raw release title. Once any pattern is set, a \
                         release must match at least one to be accepted — the series title \
                         and aliases are no longer matched on their own.",
                )]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Patterns are case-sensitive by default. Start one with "),
                    TooltipText::Var("(?i)"),
                    TooltipText::Plain(" to make it case-insensitive."),
                ]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "An empty list falls back to default matching by the series title \
                         and aliases. Season-level patterns override the series patterns \
                         for that season while set.",
                )]));

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "Restrict to one source",
                    )]));
                self.body_items.push(BodyItem::List(vec![TooltipText::Var(
                    "@instance-id:regex-pattern",
                )]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Prepend "),
                    TooltipText::Var("@instance-id:"),
                    TooltipText::Plain(
                        " to restrict this regex to a specific source plugin during polling.",
                    ),
                ]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain(
                        "The instance ID is shown in the plugin modal subtitle \
                         when editing a plugin (e.g., ",
                    ),
                    TooltipText::Var("a3f8c91e4b2d"),
                    TooltipText::Plain(")."),
                ]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "Entries from non-matching sources will skip this pattern.  \
                     Patterns without a prefix apply to all sources.",
                )]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("@a3f8c91e4b2d:1080p"),
                    TooltipText::Plain(" → only that plugin instance must match "),
                    TooltipText::Var("1080p"),
                    TooltipText::Plain(" in the title"),
                ]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("1080p"),
                    TooltipText::Plain(" → applies to all sources (no prefix)"),
                ]));

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "Filtering (no named groups)",
                    )]));
                self.body_items.push(BodyItem::Desc(vec![TooltipText::Plain(
                    "A pattern without named capture groups acts as a filter: \
                         the entry title must match it to be accepted, and the default \
                         parser still determines the episode/season.",
                )]));

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "Extract episode / season (named groups)",
                    )]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Use "),
                    TooltipText::Var("(?P<episode>\\d+)"),
                    TooltipText::Plain(
                        " to extract the episode number from non-standard titles, \
                         overriding the default filename parser.",
                    ),
                ]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Add "),
                    TooltipText::Var("(?P<season>\\d+)"),
                    TooltipText::Plain(
                        " to also extract the season number (standard mode only; \
                         ignored in absolute mode).",
                    ),
                ]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Without a "),
                    TooltipText::Var("season"),
                    TooltipText::Plain(
                        " group, the season is inherited from the context (season-level) \
                         or defaults to season 1 (series-level).",
                    ),
                ]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("Episode\\s*(?P<episode>\\d+)"),
                    TooltipText::Plain(" → extracts episode 5 from "),
                    TooltipText::Var("Episode 5"),
                    TooltipText::Plain(" (season inherited from context)"),
                ]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("S(?P<season>\\d+)EP(?P<episode>\\d+)"),
                    TooltipText::Plain(" → extracts season 3, episode 7 from "),
                    TooltipText::Var("S03EP07"),
                ]));

                self.body_items
                    .push(BodyItem::SectionTitle(vec![TooltipText::Plain(
                        "Season packs (season group only)",
                    )]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain("Use only "),
                    TooltipText::Var("(?P<season>\\d+)"),
                    TooltipText::Plain(
                        " (without an episode group) to detect season packs with \
                         non-standard naming, e.g. ",
                    ),
                    TooltipText::Var("Season 2 Complete"),
                    TooltipText::Plain(" instead of "),
                    TooltipText::Var("S02"),
                    TooltipText::Plain("."),
                ]));
                self.body_items.push(BodyItem::Desc(vec![
                    TooltipText::Plain(
                        "The release is marked as a season pack with the captured \
                         season number. If the title contains ",
                    ),
                    TooltipText::Var("Complete"),
                    TooltipText::Plain(
                        " (case-insensitive), it is also marked as a complete pack.",
                    ),
                ]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("Season\\s*(?P<season>\\d+)"),
                    TooltipText::Plain(" → season pack for season 2 from "),
                    TooltipText::Var("Season 2"),
                ]));
                self.body_items.push(BodyItem::Example(vec![
                    TooltipText::Var("S(?P<season>\\d+)\\s*Complete"),
                    TooltipText::Plain(" → complete pack for season 1 from "),
                    TooltipText::Var("S1 Complete"),
                ]));
            }
        }
        self
    }

    /// Build a help-icon tooltip — the "?" circle used across settings forms.
    pub fn build(self) -> impl IntoView {
        self.build_with("help-icon", view! { "?" })
    }

    /// Build a tooltip anchored to a custom `trigger`. The wrapper element carries
    /// `anchor_class` plus the `tooltip-host` mechanism class the hover script
    /// binds to, so any element can reveal the tooltip content. This is the entry
    /// point for normal (non-help) tooltips.
    pub fn build_with(self, anchor_class: &str, trigger: impl IntoView + 'static) -> impl IntoView {
        let title_view = self
            .title
            .map(|t| view! { <div class="tooltip-title">{t}</div> });
        let sections_view = self
            .sections
            .into_iter()
            .map(|(subtitle, vars)| {
                view! {
                    <div class="tooltip-subtitle">{subtitle}</div>
                    <ul class="tooltip-list">
                        {vars.into_iter().map(|(var, desc)| view! {
                            <li><span class="tooltip-var">{var}</span><span class="tooltip-desc">{desc}</span></li>
                        }).collect_view()}
                    </ul>
                }
            })
            .collect_view();

        let formatting_view = if self.show_formatting {
            let instructions = match self.formatting_context {
                Some(context) => get_formatting_instructions_for_context(&context),
                None => get_formatting_instructions(),
            };
            Some(view! {
                <div class="tooltip-subtitle">"Formatting"</div>
                <ul class="tooltip-list">
                    {instructions.into_iter().map(|f| {
                        view! {
                            <li><span class="tooltip-var">{f.syntax}</span><span class="tooltip-desc">{f.description}</span></li>
                        }
                    }).collect_view()}
                </ul>
            })
        } else {
            None
        };

        let body_views = self
            .body_items
            .into_iter()
            .map(|item| match item {
                BodyItem::List(parts) => {
                    let children = render_tooltip_text(parts);
                    view! {
                        <ul class="tooltip-list">
                            <li>{children}</li>
                        </ul>
                    }
                    .into_any()
                }
                BodyItem::SectionTitle(parts) => {
                    let children = render_tooltip_text(parts);
                    view! {
                        <div class="tooltip-section-title mt-sm">{children}</div>
                    }
                    .into_any()
                }
                BodyItem::Desc(parts) => {
                    let children = render_tooltip_text(parts);
                    view! { <div class="tooltip-desc">{children}</div> }.into_any()
                }
                BodyItem::Example(parts) => {
                    let children = render_tooltip_text(parts);
                    view! { <div class="tooltip-example mt-xs">{children}</div> }.into_any()
                }
                BodyItem::Section {
                    heading,
                    description,
                } => view! {
                    <div class="tooltip-section-title mt-sm">{heading}</div>
                    <div class="tooltip-desc">{description}</div>
                }
                .into_any(),
                BodyItem::Text(text) => view! { <div class="tooltip-desc">{text}</div> }.into_any(),
            })
            .collect_view();

        let mut wrapper_class = format!("{anchor_class} tooltip-host");
        if self.tooltip_auto_down {
            wrapper_class.push_str(" tooltip-auto-down");
        }

        view! {
            <span class={wrapper_class}>
                {trigger}
                <div class="tooltip-content">
                    {title_view}
                    {sections_view}
                    {formatting_view}
                    {body_views}
                </div>
            </span>
        }
    }
}
