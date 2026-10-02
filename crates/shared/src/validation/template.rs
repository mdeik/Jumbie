use crate::validation::fields::ValidationError;

/// Validate a template string against a list of allowed variables.
///
/// Supports all template syntax that `apply_template` renders:
///   - `${variable}` — basic substitution (no padding)
///   - `${variable:0N}` — zero-pad to minimum width N; `${variable:auto}` / `${variable:autoN}`
///     — zero-pad to the width the highest episode needs, optionally floored at N
///   - `${variable:<N}` / `${variable:<N:suffix}` — crop
///   - `${variable:-default}` — default value
///   - `${variable/old/new}` — replace first; `${variable//old/new}` — replace all
///   - `?{...}` — conditional block (vars inside are validated too)
///   - `$$` — escaped dollar
///
/// # Parser sync
///
/// Must be kept in sync with `apply_template` in `backend/src/utils/mod.rs`; any new
/// syntax added to the renderer must be handled here too.
///
/// # Brace semantics
///
/// `${...}` opens a variable block, `?{...}` a conditional block, and a standalone
/// `{`/`}` is literal. `${` and `?{` depths are tracked separately so a `}` inside a
/// conditional isn't mistaken for a variable close, and vice versa.
pub fn validate_template(
    template: &str,
    allowed_vars: &[&str],
    allow_empty: bool,
) -> Result<(), ValidationError> {
    if template.trim().is_empty() {
        if allow_empty {
            return Ok(());
        }
        return Err(ValidationError("Template cannot be empty".to_string()));
    }

    // Track two nested contexts: `var_depth` for `${...}`, `cond_depth` for `?{...}`.
    // A `}` closes a variable first, then a conditional; a stray `}` at depth 0 is an error.

    let mut var_depth: usize = 0;
    let mut cond_depth: usize = 0;
    // Literal `{` depth so `{text}` pairs don't trigger false positives.
    let mut literal_brace_depth: usize = 0;

    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '$' && chars[i + 1] == '{' {
            var_depth += 1;
            i += 1;
        } else if i + 1 < chars.len() && chars[i] == '?' && chars[i + 1] == '{' {
            cond_depth += 1;
            i += 1;
        } else if chars[i] == '{' {
            // Standalone `{` — not part of `${` or `?{`
            literal_brace_depth += 1;
        } else if chars[i] == '}' {
            if var_depth > 0 {
                var_depth -= 1; // closes a ${...}
            } else if cond_depth > 0 {
                cond_depth -= 1; // closes a ?{...}
            } else if literal_brace_depth > 0 {
                literal_brace_depth -= 1; // closes a literal `{`
            } else {
                return Err(ValidationError(
                    "Unexpected closing brace '}' in template".to_string(),
                ));
            }
        }
        i += 1;
    }

    if var_depth > 0 || cond_depth > 0 {
        return Err(ValidationError("Unbalanced braces in template".to_string()));
    }

    // ── Variable Name Extraction & Validation ────────────────────────────────
    // Extract every variable name from `${...}` blocks, stripping `:-default`, `/...`
    // and `:format` modifiers, and walk into `?{...}` blocks to validate their vars.

    let mut in_var = false;
    let mut in_cond = 0usize; // depth of conditional block nesting (should be 0 or 1)
    let mut var_name = String::new();
    let mut unknown_vars: Vec<String> = Vec::new();

    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '$' && chars[i + 1] == '{' {
            in_var = true;
            var_name.clear();
            i += 1;
        } else if i + 1 < chars.len() && chars[i] == '?' && chars[i + 1] == '{' {
            in_cond += 1;
            i += 1;
        } else if chars[i] == '}' {
            if in_var && !var_name.is_empty() {
                // Extract the base variable name by stripping modifiers
                let var_base = if let Some(pos) = var_name.find(":-") {
                    &var_name[..pos]
                } else if let Some(pos) = var_name.find('/') {
                    &var_name[..pos]
                } else {
                    var_name.split(':').next().unwrap_or(&var_name)
                };

                if !allowed_vars.contains(&var_base) {
                    unknown_vars.push(var_base.to_string());
                }
            }
            if in_var {
                in_var = false;
            } else {
                in_cond = in_cond.saturating_sub(1);
            }
        } else if in_var && chars[i] != ' ' {
            var_name.push(chars[i]);
        }
        i += 1;
    }

    if !unknown_vars.is_empty() {
        unknown_vars.sort();
        unknown_vars.dedup();
        let vars_list = unknown_vars.join(", ");
        let plural = if unknown_vars.len() == 1 {
            "variable"
        } else {
            "variables"
        };
        return Err(ValidationError(format!(
            "template contains unknown template {}: {}",
            plural, vars_list
        )));
    }

    Ok(())
}

/// Validate a search-key template.
///
/// Only `${season}` and `${episode}` are permitted, each optionally followed by
/// a numeric padding spec (e.g. `${season:02}`); literal text is allowed.
/// Replacements, defaults, crops, conditionals and date blocks are rejected — a
/// search key has no use for them, and rejecting them keeps the tooltip honest.
pub fn validate_search_template(template: &str, allow_empty: bool) -> Result<(), ValidationError> {
    if template.trim().is_empty() {
        if allow_empty {
            return Ok(());
        }
        return Err(ValidationError("Template cannot be empty".to_string()));
    }

    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '$' if chars.get(i + 1) == Some(&'{') => {
                let spec_start = i + 2;
                let mut end = None;
                let mut j = spec_start;
                while j < chars.len() {
                    if chars[j] == '}' {
                        end = Some(j);
                        break;
                    }
                    j += 1;
                }
                let Some(end) = end else {
                    return Err(ValidationError("Unbalanced braces in template".to_string()));
                };
                let spec: String = chars[spec_start..end].iter().collect();
                validate_search_variable(&spec)?;
                i = end + 1;
            }
            '?' if chars.get(i + 1) == Some(&'{') => {
                return Err(ValidationError(
                    "Conditional blocks are not allowed in a search template".to_string(),
                ));
            }
            '%' if chars.get(i + 1) == Some(&'{') => {
                return Err(ValidationError(
                    "Date blocks are not allowed in a search template".to_string(),
                ));
            }
            '{' | '}' => {
                return Err(ValidationError(format!(
                    "Unexpected brace '{}' in template: use ${{season}} / ${{episode}}",
                    chars[i]
                )));
            }
            _ => i += 1,
        }
    }

    Ok(())
}

/// Validate one `${...}` spec for a search template: `season` or `episode` with
/// an optional all-digits padding suffix.
fn validate_search_variable(spec: &str) -> Result<(), ValidationError> {
    let (name, padding) = match spec.split_once(':') {
        Some((name, padding)) => (name, Some(padding)),
        None => (spec, None),
    };

    if name != "season" && name != "episode" {
        return Err(ValidationError(format!(
            "unknown search template variable '{name}': only 'season' and 'episode' are allowed"
        )));
    }

    if let Some(padding) = padding
        && (padding.is_empty() || !padding.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(ValidationError(format!(
            "invalid padding ':{padding}' for '{name}': padding must be digits (e.g. :02)"
        )));
    }

    Ok(())
}

/// Whether a template references `${variable}`, optionally with a modifier such as
/// a padding spec (`${season:02}`).
///
/// SSoT for "is this template scoped to this variable", used by the resolver to
/// flag season-scoped search templates. Kept consistent with
/// [`validate_search_template`]'s variable handling (same `name[:modifier]` split).
pub fn template_uses_variable(template: &str, variable: &str) -> bool {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '$' && chars[i + 1] == '{' {
            let start = i + 2;
            let Some(offset) = chars[start..].iter().position(|&c| c == '}') else {
                return false;
            };
            let end = start + offset;
            let spec: String = chars[start..end].iter().collect();
            if spec.split(':').next() == Some(variable) {
                return true;
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_uses_variable_detects_plain_and_padded() {
        assert!(template_uses_variable(
            "S${season:02}E${episode:02}",
            "season"
        ));
        assert!(template_uses_variable("S${season}E${episode}", "season"));
        assert!(template_uses_variable("E${episode:02}", "episode"));
        assert!(!template_uses_variable("E${episode:02}", "season"));
        assert!(!template_uses_variable("", "season"));
        assert!(!template_uses_variable("S01E02", "season"));
    }

    #[test]
    fn template_uses_variable_ignores_similar_names_and_literals() {
        // No `${`, so not a variable reference.
        assert!(!template_uses_variable("$season", "season"));
        // The base name must match exactly, not as a prefix.
        assert!(!template_uses_variable("${seasonable}", "season"));
    }

    #[test]
    fn template_uses_variable_handles_unbalanced_braces() {
        assert!(!template_uses_variable("S${season:02", "season"));
    }
}
