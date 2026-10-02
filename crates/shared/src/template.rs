//! Template rendering: `${variable}` substitution and formatting.
//!
//! SSoT for template expansion, shared by the backend (filename formats, notifier
//! templates) and the frontend (search-format preview, manual search defaults).
//!
//! Parser sync: this parsing logic MUST stay in sync with `validate_template` in
//! [`crate::validation::template`] — any new syntax or behavior change must be
//! mirrored in BOTH places.

use std::collections::HashMap;

use crate::variables::is_valid_variable;

/// Padding options for template rendering.
#[derive(Default)]
pub struct TemplatePadOptions {
    /// Highest season number known for the series. Drives the width of
    /// `${season:auto}`.
    pub max_season: u32,
    /// Highest episode number known for the series (or season) being rendered.
    /// Drives the width of `${episode:auto}` — see [`auto_width`].
    pub max_episode: u32,
    /// Maximum length of the final rendered output. When the fully-expanded
    /// template exceeds this limit, it is cropped intelligently:
    ///   • If the output contains a file extension (last `.`), the extension
    ///     is preserved and the stem is truncated.
    ///   • A `…` suffix is appended to signal truncation.
    ///   • `0` means unlimited (default).
    pub max_length: usize,
}

// Apply Template
//
// Apply a naming template with variable substitution and formatting.
//
// Syntax Reference:
//   $$                → literal `$`
//   ${variable}       → basic substitution (no padding)
//   ${variable:0N}    → zero-pad to a minimum width of N digits
//   ${variable:auto}  → zero-pad to the width the highest season/episode needs
//   ${variable:autoN} → as `:auto`, but never narrower than N digits
//   ${variable:<N}    → crop to N characters
//   ${variable:<N:…}  → crop to N, append suffix "…"
//   ${variable:-default} → use "default" if variable absent or empty
//   ${variable/old/new}  → replace first occurrence
//   ${variable//old/new} → replace ALL occurrences
//   ?{content}        → conditional block; rendered only if ALL vars inside are present
//
// Security: variable values are NEVER re-parsed after substitution. A value
// containing `${...}` or `?{...}` is emitted literally, preventing injection
// through variable data.
//
// Parser sync: this parsing logic MUST stay in sync with `validate_template` in
// `crates/shared/src/validation/template.rs` — any new syntax or behavior change
// must be mirrored in BOTH places.

pub fn apply_template(
    template: &str,
    vars: &HashMap<String, String>,
    options: Option<&TemplatePadOptions>,
) -> String {
    let default_opts = TemplatePadOptions::default();
    let opts = options.unwrap_or(&default_opts);
    let mut result = String::new();
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '$' => {
                if let Some(&next) = chars.peek() {
                    match next {
                        '$' => {
                            // Escaped dollar: $$ → $
                            chars.next();
                            result.push('$');
                        }
                        '{' => {
                            // Variable block: ${...}
                            chars.next(); // consume '{'
                            process_variable(&mut chars, &mut result, vars, opts);
                        }
                        _ => {
                            // Lonely $ with no special following char
                            result.push('$');
                        }
                    }
                } else {
                    result.push('$');
                }
            }
            '{' => {
                result.push('{');
            }
            '?' => {
                if chars.peek() == Some(&'{') {
                    // Conditional block: ?{...}
                    chars.next(); // consume '{'
                    process_conditional(&mut chars, &mut result, vars, opts);
                } else {
                    result.push('?');
                }
            }
            '%' => {
                if chars.peek() == Some(&'{') {
                    // Date format block: %{source:format}
                    // Reads the source name (release / download), consumes ':',
                    // then reads the format string until '}' and formats the date.
                    chars.next(); // consume '{'
                    let mut source = String::new();
                    let mut fmt = String::new();
                    let mut after_colon = false;
                    let mut closed = false;
                    while let Some(dc) = chars.next() {
                        if dc == '}' {
                            closed = true;
                            break;
                        }
                        // ${...} inside %{...} — consume as literal text so its }
                        // isn't mistaken for the %{...} closing brace
                        if dc == '$' && chars.peek() == Some(&'{') {
                            chars.next(); // consume '{'
                            if after_colon {
                                fmt.push('$');
                                fmt.push('{');
                            } else {
                                source.push('$');
                                source.push('{');
                            }
                            for inner in chars.by_ref() {
                                if after_colon {
                                    fmt.push(inner);
                                } else {
                                    source.push(inner);
                                }
                                if inner == '}' {
                                    break;
                                }
                            }
                            continue;
                        }
                        if after_colon {
                            fmt.push(dc);
                        } else if dc == ':' {
                            after_colon = true;
                        } else {
                            source.push(dc);
                        }
                    }
                    if closed {
                        let date_val = source.trim().to_lowercase();
                        let raw_date_key = if date_val == "release" {
                            "__release_date"
                        } else if date_val == "download" {
                            "__created_at"
                        } else {
                            // Unknown source — emit literal
                            result.push_str(&format!("%{{{}:{}}}", source, fmt));
                            continue;
                        };
                        if let Some(date_str) = vars.get(raw_date_key) {
                            if !date_str.is_empty() {
                                // Reconstruct NaiveDateTime from the stored ISO string
                                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
                                    date_str,
                                    "%Y-%m-%d %H:%M:%S",
                                ) {
                                    result.push_str(&dt.format(&fmt).to_string());
                                } else {
                                    result.push_str(date_str);
                                }
                            }
                            // else: empty date → emit nothing
                        } else {
                            result.push_str(&format!("%{{{}:{}}}", source, fmt));
                        }
                    } else {
                        // Unclosed block — emit literal
                        result.push_str(&format!("%{{{}}}", source));
                        if after_colon {
                            result.push(':');
                            result.push_str(&fmt);
                        }
                    }
                } else {
                    result.push('%');
                }
            }
            _ => result.push(c),
        }
    }

    // Post-expansion crop: applies to the TOTAL rendered length (including static
    // text), unlike the per-variable `${var:<N}` crop. Preserves the file extension
    // (last `.` onwards) and operates on char boundaries for multi-byte Unicode.
    if opts.max_length > 0 && result.len() > opts.max_length {
        if let Some(dot) = result.rfind('.') {
            let ext = &result[dot..];
            let ext_len = ext.len();
            let available = opts.max_length.saturating_sub(ext_len);
            let mut end = available.min(result.len().saturating_sub(ext_len));
            if end > 0 {
                while !result.is_char_boundary(end) {
                    end -= 1;
                }
                let mut cropped = String::with_capacity(opts.max_length);
                cropped.push_str(&result[..end]);
                cropped.push_str(ext);
                result = cropped;
            } else {
                let mut end = opts.max_length;
                while !result.is_char_boundary(end) {
                    end -= 1;
                }
                result.truncate(end);
            }
        } else {
            let mut end = opts.max_length.min(result.len());
            while !result.is_char_boundary(end) {
                end -= 1;
            }
            result.truncate(end);
        }
    }

    result
}

/// Parses the content between `${` and `}` and resolves the value.
///
/// Parsing order within the spec (first-match wins):
///   1. `:-default`     — if present, everything before `:-` is the key,
///      everything after (up to next `:`) is the default
///   2. `//` or `/`     — replacement syntax (global vs first)
///   3. `:format`       — format specification
///
/// Replacement and default are mutually exclusive within a single block.
fn process_variable(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    result: &mut String,
    vars: &HashMap<String, String>,
    opts: &TemplatePadOptions,
) {
    // Step 1: extract raw spec.
    let mut var_spec = String::new();
    let mut enclosed = false;
    for vc in chars.by_ref() {
        if vc == '}' {
            enclosed = true;
            break;
        }
        var_spec.push(vc);
    }

    if !enclosed {
        // Unclosed brace — treat as literal
        result.push_str("${");
        result.push_str(&var_spec);
        return;
    }

    // Step 2: decompose spec into components.
    let key: String;
    let mut default: Option<String> = None;
    let mut replace_pattern: Option<String> = None;
    let mut replace_replacement: Option<String> = None;
    let mut replace_global = false;
    let mut format_spec: Option<String> = None;
    let mut format_options: Option<String> = None;
    let mut raw_after_key: Option<String> = None;

    // 2a. Try `:-default` syntax
    if let Some(pos) = var_spec.find(":-") {
        key = var_spec[..pos].to_string();
        let remainder = &var_spec[pos + 2..]; // skip past ":-"
        // Default extends until the first `:` (which starts a format spec)
        if let Some(format_start) = remainder.find(':') {
            default = Some(remainder[..format_start].to_string());
            raw_after_key = Some(remainder[format_start..].to_string());
        } else {
            default = Some(remainder.to_string());
            raw_after_key = None;
        }
    }
    // 2b/2c. Replacement (`//` global or `/` single). Both branches share the same
    // replacement+format parsing; only `replace_global` differs.
    else if let Some(pos) = var_spec.find("//") {
        key = var_spec[..pos].to_string();
        replace_global = true;
        parse_replacement_and_format(
            &var_spec[pos + 2..],
            &mut replace_pattern,
            &mut replace_replacement,
            &mut format_spec,
            &mut format_options,
            &mut raw_after_key,
        );
    } else if let Some(pos) = var_spec.find('/') {
        key = var_spec[..pos].to_string();
        parse_replacement_and_format(
            &var_spec[pos + 1..],
            &mut replace_pattern,
            &mut replace_replacement,
            &mut format_spec,
            &mut format_options,
            &mut raw_after_key,
        );
    }
    // 2d. Plain key with optional format spec
    else {
        let parts: Vec<&str> = var_spec.split(':').collect();
        key = parts[0].to_string();
        if parts.len() > 1 {
            format_spec = Some(parts[1].to_string());
            if parts.len() > 2 {
                format_options = Some(parts[2..].join(":"));
            }
        }
    }

    // If we had `:-default` and there's a remaining format spec, parse it
    if let Some(ref remaining) = raw_after_key {
        let parts: Vec<&str> = remaining.split(':').collect();
        if parts.len() > 1 {
            format_spec = Some(parts[1].to_string());
            if parts.len() > 2 {
                format_options = Some(parts[2..].join(":"));
            }
        }
    }

    // Step 3: validate the variable (unknown → emit literal).
    if !is_valid_variable(&key) && !vars.contains_key(&key) {
        result.push_str("${");
        result.push_str(&var_spec);
        result.push('}');
        return;
    }

    // Step 4: resolve the value.
    let raw_val = vars.get(&key).map(|s| s.as_str()).unwrap_or("");

    let resolved = if raw_val.is_empty() {
        // Variable absent or empty — use default if available
        if let Some(ref d) = default {
            d.clone()
        } else {
            raw_val.to_string() // empty string
        }
    } else {
        raw_val.to_string()
    };

    if resolved.is_empty() {
        return; // nothing to emit
    }

    // Step 5: apply replacement.
    let after_replace = if let (Some(pat), Some(rep)) = (&replace_pattern, &replace_replacement) {
        if replace_global {
            resolved.replace(pat, rep)
        } else {
            resolved.replacen(pat, rep, 1)
        }
    } else {
        resolved
    };

    // Step 6: apply format. With no format spec the value is emitted verbatim:
    // padding is opt-in via `:0N` (fixed) or `:auto` (grows with the season).
    if let Some(ref spec) = format_spec {
        if !after_replace.is_empty() {
            apply_format(
                spec,
                format_options.as_deref(),
                &after_replace,
                &key,
                result,
                opts,
            );
        }
    } else {
        result.push_str(&after_replace);
    }
}

/// Given the replacement part of a variable spec (everything after the key and
/// `//` or `/`), check if it contains a `:` that introduces a format spec.
/// Format specs are recognized by starting with `<` (crop) or a digit (padding).
///
/// Returns `(replacement_part, optional_format_spec)`.
fn split_replacement_and_format(input: &str) -> (&str, Option<&str>) {
    // Find the last `:` that could be a format delimiter
    // Format specs: `<N`, `<N:suffix`, `0N`, `N` (digit(s))
    if let Some(pos) = input.rfind(':') {
        let after = &input[pos + 1..];
        let first = after.chars().next().unwrap_or(' ');
        if first == '<' || first.is_ascii_digit() {
            return (&input[..pos], Some(after));
        }
    }
    (input, None)
}

/// Shared by both `//` (global) and `/` (single) replacement branches in
/// `process_variable`. Extracts the replacement pattern/replacement pair and
/// optional format spec from the part after `//` or `/`.
///
/// SSoT: This is the single place where replacement+format parsing lives.
fn parse_replacement_and_format(
    input: &str,
    replace_pattern: &mut Option<String>,
    replace_replacement: &mut Option<String>,
    format_spec: &mut Option<String>,
    format_options: &mut Option<String>,
    raw_after_key: &mut Option<String>,
) {
    let (repl_part, fmt_part) = split_replacement_and_format(input);
    if let Some(fmt) = fmt_part {
        let fmt_parts: Vec<&str> = fmt.split(':').collect();
        *format_spec = Some(fmt_parts[0].to_string());
        if fmt_parts.len() > 1 {
            *format_options = Some(fmt_parts[1..].join(":"));
        }
    }
    // Find the last `/` to split pattern/replacement
    if let Some(split) = repl_part.rfind('/') {
        *replace_pattern = Some(repl_part[..split].to_string());
        *replace_replacement = Some(repl_part[split + 1..].to_string());
        *raw_after_key = None;
    } else {
        // Malformed: no closing `/` — treat pattern as empty, replacement as whole string
        *replace_pattern = Some(String::new());
        *replace_replacement = Some(repl_part.to_string());
        *raw_after_key = None;
    }
}

/// Parses `?{...}` content. The block is rendered only if EVERY variable
/// referenced at the top level (outside nested `?{...}` blocks) is present
/// and non-empty in the vars map.
///
/// Supports an optional block-level fallback via `:-` at the top level
/// (not inside `${...}` or nested `?{...}`). When some variables are absent
/// the fallback is emitted instead of nothing:
///
///   `?{ - ${title}:-Untitled}`  →  "Untitled" when ${title} is absent
///
/// The fallback itself is literal text — it is NOT re-parsed for variables,
/// so it works reliably even when all variables are missing.
///
/// Uses a `dollar_seen` state machine and a `cond_depth` counter so nested
/// `?{...}` blocks are properly tracked: inner `}` decrements cond_depth,
/// only a standalone `}` at depth 0 closes this conditional.
fn process_conditional(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    result: &mut String,
    vars: &HashMap<String, String>,
    opts: &TemplatePadOptions,
) {
    let mut content = String::new();
    let mut dollar_seen = false;
    let mut cond_depth: usize = 0;

    while let Some(c) = chars.next() {
        if dollar_seen {
            if c == '{' {
                // `${...}` block — read until matching `}`
                content.push('$');
                content.push('{');
                for inner in chars.by_ref() {
                    content.push(inner);
                    if inner == '}' {
                        break;
                    }
                }
            } else {
                // Literal `$` followed by non-`{` — emit both
                content.push('$');
                content.push(c);
            }
            dollar_seen = false;
        } else if c == '$' {
            dollar_seen = true;
        } else if c == '?' {
            if chars.peek() == Some(&'{') {
                // Nested ?{ block — track depth so its `}` doesn't close us
                chars.next(); // consume '{'
                cond_depth += 1;
                content.push_str("?{");
            } else {
                content.push('?');
            }
        } else if c == '%' {
            if chars.peek() == Some(&'{') {
                // %{...} date format block — consume entirely so its `}` doesn't
                // close this conditional. Push the literal text to content so
                // recursive apply_template can format it later.
                chars.next(); // consume '{'
                content.push_str("%{");
                for inner in chars.by_ref() {
                    content.push(inner);
                    if inner == '}' {
                        break;
                    }
                }
            } else {
                content.push('%');
            }
        } else if c == '}' {
            if cond_depth > 0 {
                // Closing a nested ?{ block
                cond_depth -= 1;
                content.push('}');
            } else {
                // Standalone `}` closes this conditional block
                break;
            }
        } else {
            content.push(c);
        }
    }

    if dollar_seen {
        content.push('$');
    }

    if content.is_empty() {
        return;
    }

    // Check for block-level fallback at top-level `:-` (not inside `${...}`)
    let (block_content, fallback) = split_block_fallback(&content);

    // Check if ALL variables referenced in the block are present and non-empty
    if all_vars_present(block_content, vars) {
        // Recursively render the block content
        let rendered = apply_template(block_content, vars, Some(opts));
        result.push_str(&rendered);
    } else if let Some(fb) = fallback {
        // Block-level fallback when variables are absent
        result.push_str(fb);
    }
    // Otherwise: emit nothing — the entire block is suppressed
}

/// Split conditional block content at the first top-level `:-` (not inside `${...}`
/// or nested `?{...}`) to separate the template from its fallback.
///
/// Returns `(template, Some(fallback))` when a block-level `:-` is found,
/// or `(full_content, None)` when there is no block-level fallback.
fn split_block_fallback(content: &str) -> (&str, Option<&str>) {
    let mut depth: usize = 0;
    let mut cond_depth: usize = 0;
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if cond_depth > 0 {
            // Inside a nested ?{...} — skip until we pop back
            if i + 1 < chars.len() && chars[i] == '?' && chars[i + 1] == '{' {
                cond_depth += 1;
                i += 2;
            } else if chars[i] == '}' {
                cond_depth -= 1;
                i += 1;
            } else {
                i += 1;
            }
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '$' && chars[i + 1] == '{' {
            depth += 1;
            i += 2;
        } else if i + 1 < chars.len() && chars[i] == '?' && chars[i + 1] == '{' {
            cond_depth += 1;
            i += 2;
        } else if chars[i] == '}' && depth > 0 {
            depth -= 1;
            i += 1;
        } else if depth == 0
            && cond_depth == 0
            && i + 1 < chars.len()
            && chars[i] == ':'
            && chars[i + 1] == '-'
        {
            let tmpl = &content[..i];
            let fb = &content[i + 2..];
            return (tmpl, Some(fb));
        } else {
            i += 1;
        }
    }

    (content, None)
}

/// Check whether every variable referenced in `content` is present and non-empty.
///
/// Variables inside nested `?{...}` conditional blocks are SKIPPED because
/// those blocks may themselves be conditionally rendered — they can't make
/// the outer block fail.
///
/// Separate pre-scan instead of try-render: rendering would partially emit literal
/// text before hitting a missing variable. Pre-scanning suppresses the ENTIRE block
/// cleanly (SSoT: a block renders fully or vanishes completely).
fn all_vars_present(content: &str, vars: &HashMap<String, String>) -> bool {
    let mut chars = content.chars().peekable();
    let mut cond_depth: usize = 0;

    while let Some(c) = chars.next() {
        if cond_depth > 0 {
            // Inside a nested ?{...} — skip until we pop back to depth 0.
            // We still need to track ?{ and } for the nested block.
            if c == '?' && chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                cond_depth += 1;
            } else if c == '}' {
                cond_depth -= 1;
            }
            continue;
        }

        if c == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_spec = String::new();
                for vc in chars.by_ref() {
                    if vc == '}' {
                        break;
                    }
                    var_spec.push(vc);
                }
                // Extract the base key (strip format spec, default, replacement)
                let key = if let Some(pos) = var_spec.find(":-") {
                    &var_spec[..pos]
                } else if let Some(pos) = var_spec.find('/') {
                    &var_spec[..pos]
                } else {
                    var_spec.split(':').next().unwrap_or(&var_spec)
                };

                let val = vars.get(key).map(|s| s.as_str()).unwrap_or("");
                if val.is_empty() {
                    return false;
                }
            }
        } else if c == '%' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut source = String::new();
                let mut after_colon = false;
                let mut closed = false;
                for vc in chars.by_ref() {
                    if vc == '}' {
                        closed = true;
                        break;
                    }
                    if after_colon {
                        // format chars — skip
                    } else if vc == ':' {
                        after_colon = true;
                    } else {
                        source.push(vc);
                    }
                }
                if closed {
                    let lookup_key = match source.trim().to_lowercase().as_str() {
                        "release" => "__release_date",
                        "download" => "__created_at",
                        _ => continue,
                    };
                    let val = vars.get(lookup_key).map(|s| s.as_str()).unwrap_or("");
                    if val.is_empty() {
                        return false;
                    }
                }
            }
        } else if c == '?' && chars.peek() == Some(&'{') {
            // Enter a nested ?{...} conditional block — skip its contents
            // because variables inside it may be absent without affecting
            // the outer block.
            chars.next(); // consume '{'
            cond_depth += 1;
        }
    }

    true
}

/// Width used by `${variable:auto}`: exactly the number of digits the highest
/// value of that variable needs — the highest season for `season`, the highest
/// episode for `episode` (and, for any other numeric variable, the episode max).
fn auto_width(key: &str, opts: &TemplatePadOptions) -> usize {
    let max = if key == "season" {
        opts.max_season
    } else {
        opts.max_episode
    };
    max.to_string().len().max(1)
}

/// Applies a format specifier to a resolved value and pushes the result.
///
/// Supported specs:
///   `<N`       — crop to N chars
///   `<N:suffix` — crop to N chars, append suffix
///   `0N`       — zero-pad to a minimum width of N digits
///   `auto`     — zero-pad to [`auto_width`] (grows with the highest season/episode)
///   `autoN`    — as `auto`, but never narrower than N digits
fn apply_format(
    spec: &str,
    options: Option<&str>,
    value: &str,
    key: &str,
    result: &mut String,
    opts: &TemplatePadOptions,
) {
    if let Some(crop_part) = spec.strip_prefix('<') {
        // Crop mode.
        if let Ok(max_len) = crop_part.parse::<usize>() {
            let suffix = options.unwrap_or("…");
            if value.chars().count() > max_len {
                let available = max_len.saturating_sub(suffix.chars().count());
                let truncated: String = value.chars().take(available).collect();
                result.push_str(&truncated);
                if !suffix.is_empty() {
                    result.push_str(suffix);
                }
            } else {
                result.push_str(value);
            }
        } else {
            // Invalid crop spec — push value as-is
            result.push_str(value);
        }
    } else if let Some(min) = spec.strip_prefix("auto") {
        // `:auto` (min 0) or `:autoN`: the width the value needs, floored at N.
        let min_width = min.parse::<usize>().unwrap_or(0);
        push_padded(result, value, key, auto_width(key, opts).max(min_width));
    } else if let Ok(width) = spec.parse::<usize>() {
        // Numeric width (`:02`, `:0N`, `:2` are all equivalent): zero-pad to it.
        push_padded(result, value, key, width);
    } else {
        // Unknown format spec — push value as-is
        result.push_str(value);
    }
}

/// Pads a numeric value to `width` with zeros. Season and episode values may be
/// ranges (`"1-3+5"` for a multi-episode file), so each numeric run is padded
/// independently; non-numeric values are emitted verbatim.
fn push_padded(result: &mut String, value: &str, key: &str, width: usize) {
    if key == "episode" || key == "season" {
        let mut token = String::new();
        for c in value.chars() {
            if c.is_ascii_digit() {
                token.push(c);
                continue;
            }
            push_number(result, &token, width);
            token.clear();
            result.push(c);
        }
        push_number(result, &token, width);
    } else {
        push_number(result, value, width);
    }
}

/// Zero-pads a single numeric run to `width`, or emits it verbatim when it is
/// not a number (an empty run emits nothing).
fn push_number(result: &mut String, token: &str, width: usize) {
    match token.parse::<i32>() {
        Ok(num) => result.push_str(&format!("{:0width$}", num, width = width)),
        Err(_) => result.push_str(token),
    }
}
