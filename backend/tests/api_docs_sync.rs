//! Guards that `docs/api-docs.md` stays in sync with the implementation.
//!
//! These are "doc tests" in the sense that they test the documentation itself:
//!
//! 1. [`documented_endpoints_match_router`] parses the Axum route table in
//!    `src/api/router.rs` and the `### \`METHOD /path\`` headings in
//!    `docs/api-docs.md` and asserts they describe exactly the same endpoints —
//!    so adding/removing/renaming a route without updating the docs fails CI.
//! 2. [`documented_json_examples_are_valid`] extracts every ```json example from
//!    the endpoint sections and asserts it parses — so a hand-edited request or
//!    response example can't silently become invalid JSON.
//!
//! `include_str!` registers both files as build dependencies, so editing either
//! one recompiles and re-runs this test.

use std::collections::BTreeSet;

const ROUTER_SRC: &str = include_str!("../src/api/router.rs");
const DOCS: &str = include_str!("../../docs/api-docs.md");

/// `(METHOD, path)` pairs declared in `docs/api-docs.md`, parsed from the
/// `### \`GET /api/...\`` / `#### \`POST /api/...\`` endpoint headings.
fn documented_endpoints(md: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for raw in md.lines() {
        let trimmed = raw.trim_start();
        let heading = trimmed
            .strip_prefix("#### ")
            .or_else(|| trimmed.strip_prefix("### "));
        let Some(heading) = heading else { continue };

        // Endpoint headings are exactly `\`METHOD /path\``.
        let Some(inner) = heading
            .trim()
            .strip_prefix('`')
            .and_then(|s| s.strip_suffix('`'))
        else {
            continue;
        };
        let mut parts = inner.splitn(2, ' ');
        let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        if matches!(method, "GET" | "POST" | "PUT" | "DELETE" | "PATCH") {
            out.insert((method.to_string(), path.trim().to_string()));
        }
    }
    out
}

/// `(METHOD, path)` pairs declared in the Axum route table. Each `.route("...",
/// handler)` call contributes one entry per HTTP method builder (`get`, `post`,
/// `put`, `delete`, `patch`) it composes.
fn routed_endpoints(src: &str) -> BTreeSet<(String, String)> {
    const METHODS: [(&str, &str); 5] = [
        ("get(", "GET"),
        ("post(", "POST"),
        ("put(", "PUT"),
        ("delete(", "DELETE"),
        ("patch(", "PATCH"),
    ];

    let mut out = BTreeSet::new();
    // Each chunk starts immediately after a `.route(` call.
    for chunk in src.split(".route(").skip(1) {
        let Some(path) = first_string_literal(chunk) else {
            continue;
        };
        let bytes = chunk.as_bytes();
        for (needle, method) in METHODS {
            let mut from = 0;
            while let Some(rel) = chunk[from..].find(needle) {
                let at = from + rel;
                // Require an identifier boundary so `get(` doesn't match `target(`.
                let boundary_ok = at == 0 || {
                    let prev = bytes[at - 1];
                    !(prev.is_ascii_alphanumeric() || prev == b'_')
                };
                if boundary_ok {
                    out.insert((method.to_string(), path.clone()));
                }
                from = at + needle.len();
            }
        }
    }
    out
}

/// The contents of the first double-quoted string literal in `s`.
fn first_string_literal(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

/// ```json fenced blocks in the endpoint sections (before §10, which uses
/// illustrative, intentionally non-JSON placeholders like `[...]`).
fn json_examples(md: &str) -> Vec<String> {
    let endpoint_sections = md.split("\n## 10. Validation").next().unwrap_or(md);
    let mut blocks = Vec::new();
    let mut lines = endpoint_sections.lines();
    while let Some(line) = lines.next() {
        if !line.trim_start().starts_with("```json") {
            continue;
        }
        let mut block = String::new();
        for body_line in lines.by_ref() {
            if body_line.trim_start().starts_with("```") {
                break;
            }
            block.push_str(body_line);
            block.push('\n');
        }
        blocks.push(block);
    }
    blocks
}

#[test]
fn documented_endpoints_match_router() {
    let routed = routed_endpoints(ROUTER_SRC);
    let documented = documented_endpoints(DOCS);

    let undocumented: Vec<_> = routed.difference(&documented).collect();
    let unmatched: Vec<_> = documented.difference(&routed).collect();

    assert!(
        undocumented.is_empty() && unmatched.is_empty(),
        "docs/api-docs.md is out of sync with src/api/router.rs\n\
         \n\
         routed but not documented ({n_routed}):\n{routed:#?}\n\
         \n\
         documented but not routed ({n_doc}):\n{doc:#?}",
        n_routed = undocumented.len(),
        routed = undocumented,
        n_doc = unmatched.len(),
        doc = unmatched,
    );
}

#[test]
fn documented_json_examples_are_valid() {
    for (index, block) in json_examples(DOCS).into_iter().enumerate() {
        if let Err(err) = serde_json::from_str::<serde_json::Value>(&block) {
            panic!("docs/api-docs.md JSON example #{index} is not valid JSON: {err}\n\n{block}");
        }
    }
}
