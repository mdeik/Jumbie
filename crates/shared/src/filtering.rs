use regex::Regex;
use serde::{Deserialize, Serialize};

/// How a filter term is matched against a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    #[default]
    Substring,
    Exact,
    Wildcard,
    Regex,
}

/// A rule that determines whether a media title is accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterRule {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub excluded: Vec<String>,
    #[serde(default)]
    pub regex_required: Vec<String>,
    #[serde(default)]
    pub regex_excluded: Vec<String>,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub match_mode: MatchMode,
    #[serde(default)]
    pub search_in_description: bool,
    #[serde(default)]
    pub search_in_files: bool,

    // Optimization: Cache compiled regexes
    #[serde(skip)]
    pub compiled_required: Vec<Regex>,
    #[serde(skip)]
    pub compiled_excluded: Vec<Regex>,
    #[serde(skip)]
    pub is_compiled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for FilterRule {
    fn default() -> Self {
        Self {
            required: Vec::new(),
            excluded: Vec::new(),
            regex_required: Vec::new(),
            regex_excluded: Vec::new(),
            case_sensitive: true,
            match_mode: MatchMode::default(),
            search_in_description: false,
            search_in_files: false,
            compiled_required: Vec::new(),
            compiled_excluded: Vec::new(),
            is_compiled: false,
        }
    }
}

impl FilterRule {
    pub fn compile(&mut self) {
        if self.is_compiled {
            return;
        }

        let mut req = Vec::new();
        for pat in &self.regex_required {
            if let Ok(re) = Regex::new(pat) {
                req.push(re);
            }
        }
        if self.match_mode == MatchMode::Regex {
            for pat in &self.required {
                if let Ok(re) = Regex::new(pat) {
                    req.push(re);
                }
            }
        }
        self.compiled_required = req;

        let mut excl = Vec::new();
        for pat in &self.regex_excluded {
            if let Ok(re) = Regex::new(pat) {
                excl.push(re);
            }
        }
        if self.match_mode == MatchMode::Regex {
            for pat in &self.excluded {
                if let Ok(re) = Regex::new(pat) {
                    excl.push(re);
                }
            }
        }
        self.compiled_excluded = excl;
        self.is_compiled = true;
    }

    pub fn matches(
        &self,
        title: &str,
        description: Option<&str>,
        files: Option<&[String]>,
    ) -> Result<bool, String> {
        let mut fields: Vec<(&str, Option<String>)> = vec![(title, None)];

        if self.search_in_description
            && let Some(desc) = description
        {
            fields.push((desc, None));
        }

        if !self.case_sensitive && self.match_mode != MatchMode::Regex {
            for f in &mut fields {
                f.1 = Some(f.0.to_lowercase());
            }
        }

        let files_precalc = if let Some(fs) = files {
            if self.search_in_files {
                if !self.case_sensitive && self.match_mode != MatchMode::Regex {
                    Some(
                        fs.iter()
                            .map(|f| (f.as_str(), Some(f.to_lowercase())))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    Some(fs.iter().map(|f| (f.as_str(), None)).collect::<Vec<_>>())
                }
            } else {
                None
            }
        } else {
            None
        };

        let check_term = |text: &str, text_lower: Option<&str>, term: &str| -> bool {
            match self.match_mode {
                MatchMode::Exact => {
                    if self.case_sensitive {
                        text == term
                    } else {
                        text.to_lowercase() == term.to_lowercase()
                    }
                }
                MatchMode::Substring => {
                    if self.case_sensitive {
                        text.contains(term)
                    } else {
                        text_lower
                            .map(|l| l.contains(&term.to_lowercase()))
                            .unwrap_or_else(|| text.to_lowercase().contains(&term.to_lowercase()))
                    }
                }
                MatchMode::Wildcard => {
                    // Escape regex metacharacters, then map glob `*` → `.*`, `?` → `.`.
                    let regex_pat = format!(
                        "^{}$",
                        regex::escape(term).replace("\\*", ".*").replace("\\?", ".")
                    );
                    let flags = if self.case_sensitive { "" } else { "(?i)" };
                    let full_pat = format!("{}{}", flags, regex_pat);
                    Regex::new(&full_pat)
                        .map(|re| re.is_match(text))
                        .unwrap_or(false)
                }
                MatchMode::Regex => {
                    if let Ok(re) = Regex::new(term) {
                        re.is_match(text)
                    } else {
                        false
                    }
                }
            }
        };

        if self.is_compiled {
            for re in &self.compiled_required {
                if !self.matches_any_field_or_file(re, &fields, files) {
                    return Ok(false);
                }
            }

            for re in &self.compiled_excluded {
                if self.matches_any_field_or_file(re, &fields, files) {
                    return Ok(false);
                }
            }
        }

        // Slow path when not compiled.
        if !self.is_compiled {
            for pat in &self.regex_required {
                if let Ok(re) = Regex::new(pat)
                    && !self.matches_any_field_or_file(&re, &fields, files)
                {
                    return Ok(false);
                }
            }
            for pat in &self.regex_excluded {
                if let Ok(re) = Regex::new(pat)
                    && self.matches_any_field_or_file(&re, &fields, files)
                {
                    return Ok(false);
                }
            }
        }

        // In Regex mode the compiled path above already handled required/excluded.
        if self.match_mode != MatchMode::Regex || !self.is_compiled {
            if self.match_mode == MatchMode::Regex {
                // Compile and run required/excluded as regexes.
                for pat in &self.required {
                    if let Ok(re) = Regex::new(pat)
                        && !self.matches_any_field_or_file(&re, &fields, files)
                    {
                        return Ok(false);
                    }
                }
                for pat in &self.excluded {
                    if let Ok(re) = Regex::new(pat)
                        && self.matches_any_field_or_file(&re, &fields, files)
                    {
                        return Ok(false);
                    }
                }
            } else {
                // Substring / Exact / Wildcard modes.
                for term in &self.required {
                    let mut found = false;
                    for (text, text_lower) in &fields {
                        if check_term(text, text_lower.as_deref(), term) {
                            found = true;
                            break;
                        }
                    }
                    if !found
                        && self.search_in_files
                        && let Some(fs) = &files_precalc
                    {
                        for (f, f_lower) in fs {
                            if check_term(f, f_lower.as_deref(), term) {
                                found = true;
                                break;
                            }
                        }
                    }
                    if !found {
                        return Ok(false);
                    }
                }

                for term in &self.excluded {
                    for (text, text_lower) in &fields {
                        if check_term(text, text_lower.as_deref(), term) {
                            return Ok(false);
                        }
                    }
                    if self.search_in_files
                        && let Some(fs) = &files_precalc
                    {
                        for (f, f_lower) in fs {
                            if check_term(f, f_lower.as_deref(), term) {
                                return Ok(false);
                            }
                        }
                    }
                }
            }
        }

        Ok(true)
    }

    fn matches_any_field_or_file(
        &self,
        re: &Regex,
        fields: &[(&str, Option<String>)],
        files: Option<&[String]>,
    ) -> bool {
        let mut found = false;
        for (text, _) in fields {
            if re.is_match(text) {
                found = true;
                break;
            }
        }
        if !found
            && self.search_in_files
            && let Some(fs) = files
        {
            for f in fs {
                if re.is_match(f) {
                    found = true;
                    break;
                }
            }
        }
        found
    }

    pub fn merge(&self, other: &FilterRule) -> FilterRule {
        let mut new_rule = self.clone();
        new_rule.required.extend(other.required.clone());
        new_rule.excluded.extend(other.excluded.clone());
        new_rule.regex_required.extend(other.regex_required.clone());
        new_rule.regex_excluded.extend(other.regex_excluded.clone());

        new_rule.search_in_description = self.search_in_description || other.search_in_description;
        new_rule.search_in_files = self.search_in_files || other.search_in_files;
        new_rule.case_sensitive = self.case_sensitive && other.case_sensitive;

        new_rule.is_compiled = false;
        new_rule.compiled_required.clear();
        new_rule.compiled_excluded.clear();

        new_rule.compile();
        new_rule
    }
}

crate::test_module! {
    #[test]
    fn test_filter_rule_logic() {
        let rule = FilterRule {
            required: vec!["required".to_string()],
            excluded: vec!["bad".to_string()],
            ..Default::default()
        };

        assert!(rule.matches("This is required", None, None).unwrap());
        assert!(!rule.matches("This is optional", None, None).unwrap());
        assert!(!rule
            .matches("This is required but bad", None, None)
            .unwrap());
    }

    #[test]
    fn test_wildcard_mode() {
        let rule = FilterRule {
            required: vec!["*1080p*".to_string()],
            match_mode: MatchMode::Wildcard,
            ..Default::default()
        };
        assert!(rule.matches("Show S01E01 [1080p]", None, None).unwrap());
        assert!(!rule.matches("Show S01E01 [720p]", None, None).unwrap());

        // Single-char wildcard
        let rule2 = FilterRule {
            required: vec!["S01E0?".to_string()],
            match_mode: MatchMode::Wildcard,
            ..Default::default()
        };
        assert!(rule2.matches("S01E01", None, None).unwrap());
        assert!(rule2.matches("S01E09", None, None).unwrap());
        assert!(!rule2.matches("S01E10", None, None).unwrap());
    }

    #[test]
    fn test_exact_mode() {
        let rule = FilterRule {
            required: vec!["1080p".to_string()],
            match_mode: MatchMode::Exact,
            ..Default::default()
        };
        // Exact: must match the entire string
        assert!(rule.matches("1080p", None, None).unwrap());
        assert!(!rule.matches("Show [1080p]", None, None).unwrap());
    }

    #[test]
    fn test_regex_mode() {
        let rule = FilterRule {
            required: vec![r"S\d{2}E\d{2}".to_string()],
            match_mode: MatchMode::Regex,
            ..Default::default()
        };
        assert!(rule.matches("Show S01E05", None, None).unwrap());
        assert!(!rule.matches("Show - 05", None, None).unwrap());

        // Excluded regex
        let rule2 = FilterRule {
            excluded: vec![r"(?i)cam".to_string()],
            match_mode: MatchMode::Regex,
            ..Default::default()
        };
        assert!(rule2.matches("Good Release 1080p", None, None).unwrap());
        assert!(!rule2.matches("Shitty CAMrip", None, None).unwrap());
    }

    #[test]
    fn test_case_sensitive_flag() {
        let rule = FilterRule {
            required: vec!["HEVC".to_string()],
            case_sensitive: true,
            ..Default::default()
        };
        // Exact case match
        assert!(rule.matches("Show [HEVC]", None, None).unwrap());
        // Lowercase should NOT match when case_sensitive = true
        assert!(!rule.matches("Show [hevc]", None, None).unwrap());

        // Same rule but case_insensitive (default)
        let rule_ci = FilterRule {
            required: vec!["HEVC".to_string()],
            case_sensitive: false,
            ..Default::default()
        };
        assert!(rule_ci.matches("Show [hevc]", None, None).unwrap());
    }

    #[test]
    fn test_search_in_description() {
        let rule = FilterRule {
            required: vec!["bluray".to_string()],
            search_in_description: true,
            ..Default::default()
        };
        // Title has no "bluray" but description does
        assert!(rule
            .matches("Some Show S01E01", Some("Encoded from bluray"), None)
            .unwrap());
        // Neither title nor description
        assert!(!rule
            .matches("Some Show S01E01", Some("WebRip release"), None)
            .unwrap());
        // Required term only in title — still passes
        assert!(rule.matches("bluray release", None, None).unwrap());
    }

    #[test]
    fn test_search_in_files() {
        let rule = FilterRule {
            required: vec!["subs.srt".to_string()],
            search_in_files: true,
            ..Default::default()
        };
        let files = vec!["show.mkv".to_string(), "subs.srt".to_string()];
        assert!(rule.matches("Show S01E01", None, Some(&files)).unwrap());

        let no_sub_files = vec!["show.mkv".to_string()];
        assert!(!rule
            .matches("Show S01E01", None, Some(&no_sub_files))
            .unwrap());
    }

    #[test]
    fn test_merge_filter_rules() {
        let global = FilterRule {
            required: vec!["1080p".to_string()],
            excluded: vec!["cam".to_string()],
            case_sensitive: false,
            ..Default::default()
        };
        let local = FilterRule {
            required: vec!["hevc".to_string()],
            excluded: vec!["dubbed".to_string()],
            case_sensitive: false,
            ..Default::default()
        };
        let merged = global.merge(&local);

        // Both required terms present
        assert!(merged.required.contains(&"1080p".to_string()));
        assert!(merged.required.contains(&"hevc".to_string()));
        // Both excluded terms present
        assert!(merged.excluded.contains(&"cam".to_string()));
        assert!(merged.excluded.contains(&"dubbed".to_string()));

        // Functional check
        assert!(merged
            .matches("Good Show 1080p HEVC", None, None)
            .unwrap());
        assert!(!merged.matches("Show 1080p hevc cam", None, None).unwrap());
        assert!(!merged
            .matches("Show 1080p hevc dubbed", None, None)
            .unwrap());
    }

    #[test]
    fn test_compiled_vs_uncompiled_same_result() {
        let make_rule = || FilterRule {
            required: vec!["hevc".to_string()],
            regex_required: vec![r"(?i)1080p".to_string()],
            excluded: vec!["cam".to_string()],
            ..Default::default()
        };

        let uncompiled = make_rule();
        let mut compiled = make_rule();
        compiled.compile();

        let titles = [
            "Show 1080p HEVC",
            "Show 720p hevc",
            "Show 1080p HEVC cam",
        ];
        for title in &titles {
            assert_eq!(
                uncompiled.matches(title, None, None).unwrap(),
                compiled.matches(title, None, None).unwrap(),
                "Mismatch for title: {}",
                title
            );
        }
    }
}
