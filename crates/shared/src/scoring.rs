use crate::parsing::extract_submitter;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod scoring_weights {
    pub const RES_1080P: i32 = 10;
    pub const RES_720P: i32 = 5;
    pub const RES_BLURAY: i32 = 15;
    pub const RES_WEBDL: i32 = 12;
    pub const CODEC_HEVC_EXACT: i32 = 5;
    pub const CODEC_HEVC_REGEX: i32 = 9;
}

/// Scoring rules for evaluating media release quality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseProfile {
    #[serde(default)]
    pub min_score: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub terms: HashMap<String, i32>,
    #[serde(default)]
    pub regex_terms: HashMap<String, i32>,
    #[serde(default)]
    pub submitters: HashMap<String, i32>,
    #[serde(default)]
    pub case_insensitive_terms: bool,
    #[serde(default)]
    pub case_insensitive_submitters: bool,

    #[serde(default)]
    pub size_score_per_gb: i32,
    #[serde(default)]
    pub peers_score_per_peer: i32,
    #[serde(default)]
    pub age_score_per_day: i32,

    // Cache
    #[serde(skip)]
    pub compiled_regex_terms_cache: Vec<(Regex, i32)>,
    #[serde(skip)]
    pub is_compiled: bool,
    #[serde(skip)]
    pub terms_lowercase: HashMap<String, i32>,
    #[serde(skip)]
    pub submitters_lowercase: HashMap<String, i32>,
}

/// Push a signed points value to the reasons list with a `+` prefix for positive values.
/// SSoT for score-reason formatting.
fn push_reason(reasons: &mut Vec<String>, points: i32) {
    let prefix = if points > 0 { "+" } else { "" };
    reasons.push(format!("{}{}", prefix, points));
}

impl Default for ReleaseProfile {
    fn default() -> Self {
        Self {
            min_score: 0,
            name: "Default Profile".to_string(),
            terms: HashMap::new(),
            regex_terms: HashMap::new(),
            submitters: HashMap::new(),
            case_insensitive_terms: false,
            case_insensitive_submitters: false,
            size_score_per_gb: 0,
            peers_score_per_peer: 0,
            age_score_per_day: 0,
            compiled_regex_terms_cache: Vec::new(),
            is_compiled: false,
            terms_lowercase: HashMap::new(),
            submitters_lowercase: HashMap::new(),
        }
    }
}

impl ReleaseProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Release profile name cannot be empty".to_string());
        }
        if self.name.len() > 100 {
            return Err(format!(
                "Release profile name '{}' is too long (max 100 characters)",
                self.name
            ));
        }
        if self.name.chars().any(|c| c.is_control()) {
            return Err("Release profile name cannot contain control characters".to_string());
        }
        for pattern in self.regex_terms.keys() {
            if let Err(e) = Regex::new(pattern) {
                return Err(format!(
                    "Invalid regex pattern '{}' in release profile '{}': {}",
                    pattern, self.name, e
                ));
            }
        }
        Ok(())
    }

    pub fn compile(&mut self) {
        if self.is_compiled {
            return;
        }

        let mut regex_cache = Vec::new();
        for (pattern, points) in &self.regex_terms {
            let full_pattern = if self.case_insensitive_terms {
                format!("(?i){}", pattern)
            } else {
                pattern.clone()
            };
            if let Ok(re) = Regex::new(&full_pattern) {
                regex_cache.push((re, *points));
            }
        }
        self.compiled_regex_terms_cache = regex_cache;

        if self.case_insensitive_terms {
            self.terms_lowercase = self
                .terms
                .iter()
                .map(|(k, v)| (k.to_lowercase(), *v))
                .collect();
        }
        if self.case_insensitive_submitters {
            self.submitters_lowercase = self
                .submitters
                .iter()
                .map(|(k, v)| (k.to_lowercase(), *v))
                .collect();
        }
        self.is_compiled = true;
    }

    pub fn calculate(
        &self,
        title: &str,
        size_bytes: u64,
        seeders: u32,
        meta_date: Option<chrono::DateTime<chrono::Utc>>,
        episode_count: Option<u32>,
    ) -> (i32, Vec<String>) {
        self.calculate_inner(title, size_bytes, seeders, meta_date, episode_count, None)
    }

    /// Like `calculate`, but with an explicit submitter provided by the source
    /// plugin. When `Some`, this submitter is used for submitter scoring instead
    /// of extracting it from the title. When `None`, falls back to the title.
    pub fn calculate_with_submitter(
        &self,
        title: &str,
        size_bytes: u64,
        seeders: u32,
        meta_date: Option<chrono::DateTime<chrono::Utc>>,
        episode_count: Option<u32>,
        submitter: Option<&str>,
    ) -> (i32, Vec<String>) {
        self.calculate_inner(
            title,
            size_bytes,
            seeders,
            meta_date,
            episode_count,
            submitter,
        )
    }

    fn calculate_inner(
        &self,
        title: &str,
        size_bytes: u64,
        seeders: u32,
        meta_date: Option<chrono::DateTime<chrono::Utc>>,
        episode_count: Option<u32>,
        submitter: Option<&str>,
    ) -> (i32, Vec<String>) {
        let mut score = 0;
        let mut reasons = Vec::new();

        let title_lower_owned;
        let check_title = if self.case_insensitive_terms {
            title_lower_owned = title.to_lowercase();
            &title_lower_owned
        } else {
            title
        };

        if self.is_compiled {
            if self.case_insensitive_terms {
                for (term, points) in &self.terms_lowercase {
                    if check_title.contains(term) {
                        score += points;
                        push_reason(&mut reasons, *points);
                    }
                }
            } else {
                for (term, points) in &self.terms {
                    if check_title.contains(term) {
                        score += points;
                        push_reason(&mut reasons, *points);
                    }
                }
            }

            for (re, points) in &self.compiled_regex_terms_cache {
                if re.is_match(title) {
                    score += points;
                    push_reason(&mut reasons, *points);
                }
            }
        } else {
            // Fallback slow path
            for (term, points) in &self.terms {
                let check_term_owned;
                let check_term = if self.case_insensitive_terms {
                    check_term_owned = term.to_lowercase();
                    check_term_owned.as_str()
                } else {
                    term.as_str()
                };

                if check_title.contains(check_term) {
                    score += points;
                    push_reason(&mut reasons, *points);
                }
            }

            for (pattern, points) in &self.regex_terms {
                let full_pattern = if self.case_insensitive_terms {
                    format!("(?i){}", pattern)
                } else {
                    pattern.clone()
                };
                if let Ok(re) = Regex::new(&full_pattern)
                    && re.is_match(title)
                {
                    score += points;
                    push_reason(&mut reasons, *points);
                }
            }
        }

        // Add submitters scoring — use plugin-provided submitter if available,
        // otherwise fall back to parsing from the title.
        let submitter = submitter
            .map(|s| s.to_string())
            .unwrap_or_else(|| extract_submitter(title).unwrap_or_else(|| "Unknown".to_string()));

        let mut applied_submitter = false;

        if self.is_compiled {
            if self.case_insensitive_submitters {
                let submitter_lower = submitter.to_lowercase();
                if let Some(&points) = self.submitters_lowercase.get(&submitter_lower) {
                    score += points;
                    push_reason(&mut reasons, points);
                    applied_submitter = true;
                }
            } else if let Some(&points) = self.submitters.get(&submitter) {
                score += points;
                push_reason(&mut reasons, points);
                applied_submitter = true;
            }
        } else {
            // Fallback slow path
            if self.case_insensitive_submitters {
                let submitter_lower = submitter.to_lowercase();
                for (key, &points) in &self.submitters {
                    if key.to_lowercase() == submitter_lower {
                        score += points;
                        push_reason(&mut reasons, points);
                        applied_submitter = true;
                        break;
                    }
                }
            } else if let Some(&points) = self.submitters.get(&submitter) {
                score += points;
                push_reason(&mut reasons, points);
                applied_submitter = true;
            }
        }

        if !applied_submitter && submitter == "Unknown" {
            // Apply literal "Unknown" scoring when nothing matched and the detected
            // submitter is "Unknown".
            if let Some(&points) = self.submitters.get("Unknown") {
                score += points;
                push_reason(&mut reasons, points);
            }
        }

        // Episode-count normalization: for multi-episode releases (season packs,
        // multi-episode files), size_score_per_gb is applied per episode, so a 50GB
        // pack with 10 episodes scores as 5GB/ep rather than being biased by size.
        if self.size_score_per_gb != 0 {
            let effective_size = match episode_count {
                Some(count) if count > 1 => size_bytes / count as u64,
                _ => size_bytes,
            };
            let size_gb = effective_size as f64 / (1024.0 * 1024.0 * 1024.0);
            let size_points = (size_gb * self.size_score_per_gb as f64).round() as i32;
            if size_points != 0 {
                score += size_points;
                push_reason(&mut reasons, size_points);
            }
        }

        if self.peers_score_per_peer != 0 {
            let peer_points = (seeders as f64 * self.peers_score_per_peer as f64).round() as i32;
            if peer_points != 0 {
                score += peer_points;
                push_reason(&mut reasons, peer_points);
            }
        }

        if self.age_score_per_day != 0
            && let Some(published) = meta_date
        {
            let now = chrono::Utc::now();
            if published <= now {
                let age_days = now.signed_duration_since(published).num_seconds() as f64 / 86400.0;
                let age_points = (age_days * self.age_score_per_day as f64).round() as i32;
                if age_points != 0 {
                    score += age_points;
                    push_reason(&mut reasons, age_points);
                }
            } else {
                // Future release, 0 age
            }
        }

        (score, reasons)
    }

    pub fn merge(&self, other: &ReleaseProfile) -> ReleaseProfile {
        let mut new_weights = self.clone();
        new_weights.terms.extend(other.terms.clone());
        new_weights.regex_terms.extend(other.regex_terms.clone());
        new_weights.submitters.extend(other.submitters.clone());
        new_weights.case_insensitive_terms =
            self.case_insensitive_terms && other.case_insensitive_terms;
        new_weights.case_insensitive_submitters =
            self.case_insensitive_submitters && other.case_insensitive_submitters;

        // Metadata properties: `other` overrides when non-zero.
        if other.size_score_per_gb != 0 {
            new_weights.size_score_per_gb = other.size_score_per_gb;
        }
        if other.peers_score_per_peer != 0 {
            new_weights.peers_score_per_peer = other.peers_score_per_peer;
        }
        if other.age_score_per_day != 0 {
            new_weights.age_score_per_day = other.age_score_per_day;
        }

        new_weights.is_compiled = false;
        new_weights.compiled_regex_terms_cache.clear();
        new_weights.terms_lowercase.clear();

        new_weights.compile();
        new_weights
    }
}

/// Quality tier definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quality {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Quality {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Quality name cannot be empty".to_string());
        }
        if self.name.len() > 100 {
            return Err(format!(
                "Quality name '{}' is too long (max 100 characters)",
                self.name
            ));
        }
        if self.name.chars().any(|c| c.is_control()) {
            return Err("Quality name cannot contain control characters".to_string());
        }
        for tag in &self.tags {
            if tag.trim().is_empty() {
                return Err("Quality tag cannot be empty".to_string());
            }
            if tag.len() > 50 {
                return Err(format!(
                    "Quality tag '{}' is too long (max 50 characters)",
                    tag
                ));
            }
            if tag.chars().any(|c| c.is_control()) {
                return Err("Quality tag cannot contain control characters".to_string());
            }
        }
        Ok(())
    }
}

/// Named grouping of quality tiers with upgrade policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityProfile {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub qualities: Vec<String>,

    #[serde(default)]
    pub upgrade_only_qualities: Vec<String>,
}

impl QualityProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Quality profile name cannot be empty".to_string());
        }
        if self.name.len() > 100 {
            return Err(format!(
                "Quality profile name '{}' is too long (max 100 characters)",
                self.name
            ));
        }
        if self.name.chars().any(|c| c.is_control()) {
            return Err("Quality profile name cannot contain control characters".to_string());
        }
        if self.qualities.is_empty() {
            return Err(format!(
                "Quality profile '{}' must have at least one quality tier",
                self.name
            ));
        }
        if !self
            .upgrade_only_qualities
            .iter()
            .all(|q| self.qualities.contains(q))
        {
            return Err(format!(
                "Quality profile '{}': upgrade-only qualities must be a subset of selected qualities",
                self.name
            ));
        }
        Ok(())
    }

    /// Returns true if the given quality ID is in the upgrade-only list.
    pub fn is_upgrade_only(&self, quality_id: &str) -> bool {
        self.upgrade_only_qualities.iter().any(|q| q == quality_id)
    }

    /// Returns the quality state: 0 = not in profile, 1 = normal, 2 = upgrade-only
    pub fn quality_state(&self, quality_id: &str) -> u8 {
        if !self.qualities.iter().any(|q| q == quality_id) {
            0
        } else if self.upgrade_only_qualities.iter().any(|q| q == quality_id) {
            2
        } else {
            1
        }
    }
}

/// A single custom scoring format rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFormat {
    pub regex: String,
    pub score: i32,
}

crate::test_module! {
    use super::scoring_weights::*;
    use std::collections::HashMap;

    #[test]
    fn test_scoring_weights() {
        let mut terms = HashMap::new();
        terms.insert("1080p".to_string(), RES_1080P);
        terms.insert("hevc".to_string(), CODEC_HEVC_EXACT);

        let weights = ReleaseProfile {
            terms,
            ..Default::default()
        };

        let (score, _) = weights.calculate("[Group] Show S01E01 [1080p][HEVC]", 0, 0, None, None);
        assert!(score > 0, "Expected positive score for 1080p+hevc title");
    }

    #[test]
    fn test_custom_release_profile_scoring() {
        use std::collections::HashMap;
        let mut terms = HashMap::new();
        terms.insert("1080p".to_string(), RES_1080P);

        let mut regex_terms = HashMap::new();
        regex_terms.insert(".*hevc.*".to_string(), CODEC_HEVC_REGEX);

        let mut profile = ReleaseProfile {
            name: "My Custom Profile".to_string(),
            min_score: 5,
            terms,
            regex_terms,
            case_insensitive_terms: true,
            ..Default::default()
        };
        profile.compile();

        // Simulate a search result title
        let title_1 = "[MockFansub] Mock Idol Show - 11 (1080p) [HEVC]";
        let title_2 = "[MockRaws] Mock Idol Show - 11 (720p) [x264]";

        let (score_1, reasons_1) = profile.calculate(title_1, 0, 0, None, None);
        let (score_2, _reasons_2) = profile.calculate(title_2, 0, 0, None, None);

        // 1080p (+10) + hevc (+9) = 19
        assert_eq!(score_1, 19, "Expected 1080p and regex HEVC terms to match");
        assert!(reasons_1.contains(&"+10".to_string()));
        assert!(reasons_1.contains(&"+9".to_string()));

        // Neither 1080p nor hevc matches
        assert_eq!(score_2, 0, "Expected 0 score when no terms match");
    }

    #[test]
    fn test_proportional_metadata_scoring() {
        let mut profile = ReleaseProfile {
            name: "Proportional Scoring".to_string(),
            size_score_per_gb: 10,   // +10 points per GB
            peers_score_per_peer: 2, // +2 points per seeder
            age_score_per_day: -5,   // -5 points per day
            ..Default::default()
        };
        profile.compile();

        // Simulate a 1.5 GB release, 100 seeders, published exactly 2 days ago
        let size_bytes = (1.5 * 1024.0 * 1024.0 * 1024.0) as u64;
        let seeders = 100;
        let meta_date = Some(chrono::Utc::now() - chrono::Duration::days(2));

        let (score, reasons) = profile.calculate(
            "[MockFansub] Mock Idol Show - 11",
            size_bytes,
            seeders,
            meta_date,
            None,
        );

        // Size: 1.5 * 10 = +15
        // Seeders: 100 * 2 = +200
        // Age: 2.0 * -5 = -10
        // Total expected = 205
        assert_eq!(score, 205, "Expected exact proportional scaling score");
        assert!(reasons.contains(&"+15".to_string()));
        assert!(reasons.contains(&"+200".to_string()));
        assert!(reasons.contains(&"-10".to_string()));
    }

    #[test]
    fn test_size_score_per_gb_with_episode_count() {
        let profile = ReleaseProfile {
            name: "Episode Count Normalization".to_string(),
            size_score_per_gb: 10, // +10 per GB per episode
            ..Default::default()
        };

        // 6 GB total, 3 episodes → 2 GB/ep → +20
        let size = (6.0 * 1024.0 * 1024.0 * 1024.0) as u64;
        let (score_3ep, reasons_3ep) =
            profile.calculate("Show S01E01-E03 [1080p]", size, 0, None, Some(3));
        assert_eq!(
            score_3ep, 20,
            "6 GB / 3 episodes * 10 pts/GB = 20"
        );
        assert!(reasons_3ep.contains(&"+20".to_string()));

        // Same size, no episode count → 6 GB → +60 (unchanged behavior)
        let (score_no_count, reasons_no_count) =
            profile.calculate("Show S01E01-E03 [1080p]", size, 0, None, None);
        assert_eq!(
            score_no_count, 60,
            "6 GB * 10 pts/GB = 60 when no episode_count"
        );
        assert!(reasons_no_count.contains(&"+60".to_string()));

        // Same size, 1 episode → No normalization → +60
        let (score_1ep, _) =
            profile.calculate("Show S01E01 [1080p]", size, 0, None, Some(1));
        assert_eq!(
            score_1ep, 60,
            "Some(1) should not normalize"
        );

        // Season pack: 50 GB, 10 episodes → 5 GB/ep → +50
        let pack_size = (50.0 * 1024.0 * 1024.0 * 1024.0) as u64;
        let (score_pack, _) =
            profile.calculate("Show S01 Complete 1080p", pack_size, 0, None, Some(10));
        assert_eq!(
            score_pack, 50,
            "50 GB / 10 episodes * 10 pts/GB = 50"
        );
    }

    #[test]
    fn test_case_sensitive_terms() {
        let mut terms = std::collections::HashMap::new();
        terms.insert("HEVC".to_string(), 20);

        let mut profile_sensitive = ReleaseProfile {
            terms: terms.clone(),
            case_insensitive_terms: false,
            ..Default::default()
        };
        profile_sensitive.compile();

        // Case-sensitive: "hevc" should NOT match the term "HEVC"
        let (score_lower, _) = profile_sensitive.calculate("Show hevc 1080p", 0, 0, None, None);
        assert_eq!(score_lower, 0, "Case-sensitive matching: lowercase should not match");

        // All-caps should match
        let (score_upper, _) = profile_sensitive.calculate("Show HEVC 1080p", 0, 0, None, None);
        assert_eq!(score_upper, 20, "Case-sensitive matching: uppercase should match");

        // Case-insensitive mode: "hevc" SHOULD match "HEVC"
        let mut profile_insensitive = ReleaseProfile {
            terms: terms.clone(),
            case_insensitive_terms: true,
            ..Default::default()
        };
        profile_insensitive.compile();
        let (score_ci, _) = profile_insensitive.calculate("Show hevc 1080p", 0, 0, None, None);
        assert_eq!(score_ci, 20, "Case-insensitive matching: lowercase should match uppercase term");
    }

    #[test]
    fn test_case_sensitive_regex() {
        // Regex + case_sensitive: true — exact case only
        let mut regex_terms = std::collections::HashMap::new();
        regex_terms.insert(r"\bHEVC\b".to_string(), 15);

        let mut profile_sensitive = ReleaseProfile {
            regex_terms: regex_terms.clone(),
            case_insensitive_terms: false,
            ..Default::default()
        };
        profile_sensitive.compile();

        let (score_upper, _) = profile_sensitive.calculate("Show HEVC 1080p", 0, 0, None, None);
        assert_eq!(score_upper, 15, "Regex case-sensitive: uppercase should match");

        let (score_lower, _) = profile_sensitive.calculate("Show hevc 1080p", 0, 0, None, None);
        assert_eq!(score_lower, 0, "Regex case-sensitive: lowercase should NOT match");

        let (score_mixed, _) = profile_sensitive.calculate("Show Hevc 1080p", 0, 0, None, None);
        assert_eq!(score_mixed, 0, "Regex case-sensitive: mixed case should NOT match");

        // Regex + case_insensitive: true — case-insensitive via (?i)
        let mut profile_insensitive = ReleaseProfile {
            regex_terms: regex_terms.clone(),
            case_insensitive_terms: true,
            ..Default::default()
        };
        profile_insensitive.compile();

        let (score_ci_upper, _) = profile_insensitive.calculate("Show HEVC 1080p", 0, 0, None, None);
        assert_eq!(score_ci_upper, 15, "Regex case-insensitive: uppercase should match");

        let (score_ci_lower, _) = profile_insensitive.calculate("Show hevc 1080p", 0, 0, None, None);
        assert_eq!(score_ci_lower, 15, "Regex case-insensitive: lowercase should match");

        let (score_ci_mixed, _) = profile_insensitive.calculate("Show Hevc 1080p", 0, 0, None, None);
        assert_eq!(score_ci_mixed, 15, "Regex case-insensitive: mixed case should match");

        // Mixed keywords + regex under case-insensitive default
        let mut terms = std::collections::HashMap::new();
        terms.insert("1080p".to_string(), 10);

        let mut profile_default = ReleaseProfile {
            terms,
            regex_terms,
            case_insensitive_terms: false,
            ..Default::default()
        };
        profile_default.compile();

        // Both match with correct case
        let (score_both, reasons) = profile_default.calculate("Show HEVC 1080p", 0, 0, None, None);
        assert_eq!(score_both, 25, "Default case-sensitive: both keyword and regex should match");
        assert!(reasons.contains(&"+10".to_string()));
        assert!(reasons.contains(&"+15".to_string()));

        // Keyword matches (1080p), regex doesn't (hevc ≠ HEVC)
        let (score_keyword_only, _) = profile_default.calculate("Show hevc 1080p", 0, 0, None, None);
        assert_eq!(score_keyword_only, 10, "Default case-sensitive: only keyword should match, regex should not");
    }

    #[test]
    fn test_submitter_scoring() {
        let mut submitters = std::collections::HashMap::new();
        submitters.insert("MockFansub".to_string(), 50);
        submitters.insert("Unknown".to_string(), -100);

        let mut profile = ReleaseProfile {
            submitters,
            ..Default::default()
        };
        profile.compile();

        // Known group with bonus
        let (score_known, _) = profile.calculate("[MockFansub] Show S01E01", 0, 0, None, None);
        assert_eq!(score_known, 50, "Known submitter should get +50");

        // Unknown group gets penalty
        let (score_unknown, _) = profile.calculate("Show S01E01 - SomeGroup", 0, 0, None, None);
        // "SomeGroup" is not in the submitters map, falls through to "Unknown" if not found.
        // The extract_submitter function will detect "SomeGroup" from "- SomeGroup" suffix.
        // Since "SomeGroup" is not in the map and it isn't "Unknown", no Unknown penalty applies.
        // Let's verify score is 0 (not penalised).
        assert_eq!(score_unknown, 0, "Unrecognised group with no Unknown fallback should score 0");

        // No brackets at all → submitter extraction fails → score "-100" for Unknown
        let (score_nogroup, _) = profile.calculate("ShowWithNoGroup S01E01", 0, 0, None, None);
        assert_eq!(score_nogroup, -100, "Undetectable group maps to Unknown penalty");
    }

    #[test]
    fn test_submitter_scoring_with_plugin_provided_submitter() {
        let mut submitters = std::collections::HashMap::new();
        submitters.insert("PluginGroup".to_string(), 50);

        let mut profile = ReleaseProfile {
            submitters,
            ..Default::default()
        };
        profile.compile();

        // Title has no group in brackets/suffix, but plugin provides submitter
        let (score, _) = profile.calculate_with_submitter(
            "Show.S01E01.NoGroupInTitle.mkv",
            0,
            0,
            None,
            None,
            Some("PluginGroup"),
        );
        assert_eq!(
            score, 50,
            "Plugin-provided submitter should match even when title has no group"
        );

        // When plugin provides submitter that doesn't match, no scoring applied
        let (score_mismatch, _) = profile.calculate_with_submitter(
            "Show.S01E01.NoGroupInTitle.mkv",
            0,
            0,
            None,
            None,
            Some("OtherGroup"),
        );
        assert_eq!(
            score_mismatch, 0,
            "Unrecognised plugin submitter should not be penalised"
        );
    }

    #[test]
    fn test_profile_merge() {
        let mut base_terms = std::collections::HashMap::new();
        base_terms.insert("1080p".to_string(), RES_1080P);

        let base = ReleaseProfile {
            terms: base_terms,
            size_score_per_gb: 5,
            ..Default::default()
        };

        let mut extra_terms = std::collections::HashMap::new();
        extra_terms.insert("hevc".to_string(), 8);

        let extra = ReleaseProfile {
            terms: extra_terms,
            size_score_per_gb: 20, // non-zero → overrides base
            ..Default::default()
        };

        let merged = base.merge(&extra);

        // Both terms present
        assert!(merged.terms.contains_key("1080p"));
        assert!(merged.terms.contains_key("hevc"));
        // Metadata field: extra had non-zero value so it overrides
        assert_eq!(merged.size_score_per_gb, 20);

        // Functional: title matches both terms
        let (score, _) = merged.calculate("Show 1080p hevc", 0, 0, None, None);
        assert_eq!(score, 18, "Merged terms should both score");
    }

    #[test]
    fn test_compiled_vs_uncompiled_scoring() {
        let make_profile = || {
            let mut terms = std::collections::HashMap::new();
            terms.insert("1080p".to_string(), RES_1080P);
            let mut regex_terms = std::collections::HashMap::new();
            regex_terms.insert(r"(?i)hevc".to_string(), CODEC_HEVC_EXACT);
            ReleaseProfile {
                terms,
                regex_terms,
                ..Default::default()
            }
        };

        let uncompiled = make_profile();
        let mut compiled = make_profile();
        compiled.compile();

        let titles = [
            "[Group] Show 1080p HEVC",
            "[Group] Show 720p",
            "Show 1080p x264",
        ];
        for title in &titles {
            let (s1, _) = uncompiled.calculate(title, 0, 0, None, None);
            let (s2, _) = compiled.calculate(title, 0, 0, None, None);
            assert_eq!(s1, s2, "Score mismatch compiled vs uncompiled for: {}", title);
        }
    }
}

#[cfg(test)]
mod upgrade_only_tests {
    use super::*;

    #[test]
    fn test_upgrade_only_qualities() {
        let profile = QualityProfile {
            name: "Test".to_string(),
            qualities: vec!["q1".to_string(), "q2".to_string()],
            upgrade_only_qualities: vec!["q2".to_string()],
        };

        assert_eq!(profile.quality_state("q1"), 1); // normal
        assert_eq!(profile.quality_state("q2"), 2); // upgrade-only
        assert_eq!(profile.quality_state("q3"), 0); // not in profile
        assert!(profile.is_upgrade_only("q2"));
        assert!(!profile.is_upgrade_only("q1"));
        assert!(!profile.is_upgrade_only("q3"));
    }

    #[test]
    fn test_upgrade_only_subset_validation() {
        // Valid: upgrade_only is a subset of qualities
        let profile = QualityProfile {
            name: "Test".to_string(),
            qualities: vec!["q1".to_string(), "q2".to_string()],
            upgrade_only_qualities: vec!["q2".to_string()],
        };
        assert!(profile.validate().is_ok());

        // Invalid: upgrade_only contains quality not in qualities
        let bad_profile = QualityProfile {
            name: "Test".to_string(),
            qualities: vec!["q1".to_string()],
            upgrade_only_qualities: vec!["q2".to_string()],
        };
        assert!(bad_profile.validate().is_err());
        assert!(bad_profile.validate().unwrap_err().contains("subset"));
    }

    #[test]
    fn test_empty_upgrade_only() {
        // Empty upgrade_only is valid (backward compat)
        let profile = QualityProfile {
            name: "Test".to_string(),
            qualities: vec!["q1".to_string()],
            upgrade_only_qualities: vec![],
        };
        assert_eq!(profile.quality_state("q1"), 1);
        assert!(profile.validate().is_ok());
    }
}
