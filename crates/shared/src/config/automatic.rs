use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// An enum (not a string) so invalid values are impossible and the scoring /
// validation match arms stay exhaustive.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TrackType {
    Any,
    Video,
    Audio,
    Subtitle,
}

// Binary presence check for automatic profiles. Two named variants rather than a
// bool so the intent is self-documenting at every match site.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuleCondition {
    #[default]
    MustBePresent,
    MustNotBePresent,
}

// Comparison direction for numeric rules. Separate from `RuleCondition` because
// that is binary presence while this is comparative; combining them would force
// both kinds of rules to carry unused fields.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdCondition {
    #[default]
    Minimum,
    Maximum,
    Exact,
}

// How `file_patterns` is interpreted for the UnexpectedFiles rule.
// Blacklist (default): listed extensions are flagged (empty = flag all unknown
// files). Whitelist: only listed extensions are flagged (empty = flag nothing).
// Two named variants rather than a bool so match sites stay self-documenting.
#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UnexpectedFilesMode {
    #[default]
    Blacklist,
    Whitelist,
}

// An enum with data variants rather than a trait: each variant is a concrete,
// compile-time-known check, and the set is intentionally bounded — adding a rule
// type changes this enum, forcing review of every match site in the scoring pipeline.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticProfileRule {
    // UnexpectedFiles flags files that don't match the expected naming pattern.
    // file_patterns: optional list of file extensions that should be treated as
    // unexpected. If empty, ALL unknown files are flagged (current behavior).
    // If non-empty, only files matching these extensions are flagged.
    // This allows users to specify file types to prosecute or ignore.
    UnexpectedFiles {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        file_patterns: Vec<String>,
        #[serde(default, skip_serializing_if = "is_blacklist_mode")]
        mode: UnexpectedFilesMode,
    },

    // Resolution checks if a video stream meets minimum/maximum dimensions.
    Resolution {
        width: u32,
        height: u32,
        #[serde(default)]
        condition: ThresholdCondition,
    },
    // Bitrate checks the video bitrate against a threshold (kbps).
    Bitrate {
        kbps: u64,
        #[serde(default)]
        condition: ThresholdCondition,
    },
    // Codec checks if a codec (e.g., "x265", "av1") is present or absent.
    Codec {
        codec: String,
        #[serde(default)]
        condition: RuleCondition,
    },
    // AudioChannels checks the number of audio channels (stereo=2, 5.1=6, etc.).
    AudioChannels {
        channels: u32,
        #[serde(default)]
        condition: ThresholdCondition,
    },
    // Chapters checks whether chapter markers exist in the file.
    Chapters {
        #[serde(default)]
        condition: RuleCondition,
    },
    // TrackCount checks the number of tracks of a given type (video, audio, sub).
    TrackCount {
        count: i32,
        track_type: TrackType,
        #[serde(default)]
        condition: ThresholdCondition,
    },
    // Language checks if a specific language track is present or absent.
    Language {
        language: String,
        track_type: TrackType,
        #[serde(default)]
        condition: RuleCondition,
    },
}

// Wraps a rule with scoring metadata: `modifier` is the score change per matching
// file; `bound` caps the category's total impact (0 = no cap).
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct AutomaticProfileCategory {
    pub modifier: i32,
    /// Caps total score per category:
    ///   positive → upper limit (max score from this category)
    ///   negative → lower limit (min score from this category)
    ///   0 (default) → no limit, allow up to i32 range.
    #[serde(alias = "max_modifier", alias = "max_penalty", default)]
    pub bound: i32,
    pub rule: AutomaticProfileRule,
}

// Disabled by default: automatic profiles change how releases are scored and could
// silently reject good releases, so the user must opt in after configuring rules.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
pub struct AutomaticProfilesConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub categories: HashMap<String, AutomaticProfileCategory>,
}

impl AutomaticProfilesConfig {
    pub fn validate(&self) -> Result<(), String> {
        for (name, category) in &self.categories {
            if name.trim().is_empty() {
                return Err("Category name cannot be empty".to_string());
            }
            category.validate(name)?;
        }
        Ok(())
    }
}

// Validated at config load time so the user gets immediate feedback on save rather
// than discovering a broken rule later when a release fails to match.
impl AutomaticProfileCategory {
    pub fn validate(&self, name: &str) -> Result<(), String> {
        match &self.rule {
            AutomaticProfileRule::Resolution { width, height, .. } => {
                if *width == 0 || *height == 0 {
                    return Err(format!(
                        "Category '{}': resolution must be greater than 0",
                        name
                    ));
                }
            }
            AutomaticProfileRule::Bitrate { kbps, .. } => {
                if *kbps == 0 {
                    return Err(format!(
                        "Category '{}': bitrate must be greater than 0",
                        name
                    ));
                }
            }
            AutomaticProfileRule::Codec { codec, .. } => {
                if codec.trim().is_empty() {
                    return Err(format!("Category '{}': codec cannot be empty", name));
                }
            }
            AutomaticProfileRule::AudioChannels { channels, .. } => {
                if *channels == 0 {
                    return Err(format!(
                        "Category '{}': channels must be greater than 0",
                        name
                    ));
                }
            }
            AutomaticProfileRule::TrackCount { count, .. } => {
                if *count < 0 {
                    return Err(format!(
                        "Category '{}': track count cannot be negative",
                        name
                    ));
                }
            }
            AutomaticProfileRule::Language { language, .. } if language.trim().is_empty() => {
                return Err(format!("Category '{}': language cannot be empty", name));
            }
            // UnexpectedFiles and Chapters have no numeric/codec fields to validate
            // — the defaults (condition: MustBePresent) are always valid.
            _ => {}
        }
        Ok(())
    }
}

/// Serde skip helper: don't serialize `mode` when it's the default (Blacklist).
fn is_blacklist_mode(m: &UnexpectedFilesMode) -> bool {
    matches!(m, UnexpectedFilesMode::Blacklist)
}

impl AutomaticProfileRule {
    /// Convenience accessor for the file_patterns field (UnexpectedFiles only).
    pub fn file_patterns(&self) -> &[String] {
        if let AutomaticProfileRule::UnexpectedFiles { file_patterns, .. } = self {
            file_patterns
        } else {
            &[]
        }
    }

    /// Convenience accessor for the mode field (UnexpectedFiles only).
    /// Returns Blacklist for non-UnexpectedFiles variants.
    pub fn unexpected_files_mode(&self) -> UnexpectedFilesMode {
        if let AutomaticProfileRule::UnexpectedFiles { mode, .. } = self {
            *mode
        } else {
            UnexpectedFilesMode::Blacklist
        }
    }

    /// SSoT for whether a file extension should be flagged by this UnexpectedFiles
    /// rule; used identically during walk scoring and rescoring.
    ///
    /// `ext` is expected dotless but a leading dot is tolerated (both the extension
    /// and each pattern are stripped before comparison). Returns `false` for any
    /// non-UnexpectedFiles variant.
    pub fn matches_unexpected_extension(&self, ext: &str) -> bool {
        // Normalise: strip the leading dot for robust matching.
        let ext = ext.trim_start_matches('.');

        if let AutomaticProfileRule::UnexpectedFiles {
            file_patterns,
            mode,
        } = self
        {
            let patterns_stripped: Vec<&str> = file_patterns
                .iter()
                .map(|p| p.trim_start_matches('.'))
                .collect();

            match mode {
                UnexpectedFilesMode::Whitelist => {
                    // Whitelist: empty = flag nothing; non-empty = exact match.
                    !file_patterns.is_empty()
                        && patterns_stripped
                            .iter()
                            .any(|p| ext.eq_ignore_ascii_case(p))
                }
                UnexpectedFilesMode::Blacklist => {
                    // Blacklist: check file_patterns (empty = match all).
                    let pattern_match = file_patterns.is_empty()
                        || patterns_stripped
                            .iter()
                            .any(|p| ext.eq_ignore_ascii_case(p));
                    if !pattern_match {
                        false
                    } else if crate::media_format::is_ignored_unknown_ext(ext) {
                        // Skip unless this category explicitly lists the extension.
                        patterns_stripped
                            .iter()
                            .any(|p| ext.eq_ignore_ascii_case(p))
                    } else {
                        true
                    }
                }
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unexpected_files_roundtrip_empty_patterns() {
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec![],
            mode: UnexpectedFilesMode::Blacklist,
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert_eq!(
            json, r##"{"unexpected_files":{}}"##,
            "empty patterns + blacklist mode should be skipped in serialization"
        );
        let deserialized: AutomaticProfileRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, deserialized);
    }

    #[test]
    fn test_unexpected_files_roundtrip_with_patterns() {
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec!["srt".to_string(), "idx".to_string()],
            mode: UnexpectedFilesMode::Blacklist,
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(
            json.contains(r##""file_patterns"##),
            "patterns should be present in serialization: {}",
            json
        );
        assert!(
            !json.contains(r##""mode"##),
            "blacklist mode should be skipped in serialization: {}",
            json
        );
        let deserialized: AutomaticProfileRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, deserialized);
    }

    #[test]
    fn test_codec_serializes_as_codec_not_blacklisted() {
        let rule = AutomaticProfileRule::Codec {
            codec: "x265".to_string(),
            condition: RuleCondition::MustNotBePresent,
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(
            json.contains(r##""codec"##),
            "Codec should serialize as 'codec', got: {}",
            json
        );
        assert!(
            !json.contains("blacklisted"),
            "Serialization should not contain 'blacklisted', got: {}",
            json
        );
    }

    #[test]
    fn test_unexpected_files_with_patterns_validates_ok() {
        let category = AutomaticProfileCategory {
            modifier: -5,
            bound: -50,
            rule: AutomaticProfileRule::UnexpectedFiles {
                file_patterns: vec!["srt".to_string(), "idx".to_string()],
                mode: UnexpectedFilesMode::Blacklist,
            },
        };
        assert!(category.validate("test-cat").is_ok());
    }

    #[test]
    fn test_unexpected_files_empty_patterns_validates_ok() {
        let category = AutomaticProfileCategory {
            modifier: -5,
            bound: -50,
            rule: AutomaticProfileRule::UnexpectedFiles {
                file_patterns: vec![],
                mode: UnexpectedFilesMode::Blacklist,
            },
        };
        assert!(category.validate("test-cat").is_ok());
    }

    #[test]
    fn test_unexpected_files_patterns_with_leading_dots() {
        // Simulate what the frontend parsing does: ".srt, .idx" → ["srt", "idx"]
        let input = ".srt, .idx, .txt";
        let patterns: Vec<String> = input
            .split([',', ';'])
            .map(|s| s.trim().trim_start_matches('.').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(patterns, vec!["srt", "idx", "txt"]);

        // Verify the stripped patterns work correctly in the type
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: patterns,
            mode: UnexpectedFilesMode::Blacklist,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: AutomaticProfileRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, deserialized);

        // Verify the SSoT method matches (extensions are always dotless,
        // as they come from file.extension() which returns without leading dot).
        // Patterns are ["srt", "idx", "txt"] in blacklist mode.
        assert!(deserialized.matches_unexpected_extension("srt")); // in patterns
        assert!(deserialized.matches_unexpected_extension("SRT")); // case insensitive
        assert!(deserialized.matches_unexpected_extension("idx")); // in patterns
        assert!(deserialized.matches_unexpected_extension("txt")); // in patterns
        assert!(!deserialized.matches_unexpected_extension("nfo")); // in IGNORED_UNKNOWN_EXTS, not in patterns
    }

    #[test]
    fn test_matches_unexpected_extension_blacklist() {
        // Blacklist, empty patterns = match all (minus ignored)
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec![],
            mode: UnexpectedFilesMode::Blacklist,
        };
        // "txt" is not in IGNORED_UNKNOWN_EXTS → match
        assert!(rule.matches_unexpected_extension("txt"));
        // "nfo" is in IGNORED_UNKNOWN_EXTS → no match (not overridden)
        assert!(!rule.matches_unexpected_extension("nfo"));

        // Blacklist, non-empty patterns
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec!["srt".to_string()],
            mode: UnexpectedFilesMode::Blacklist,
        };
        assert!(rule.matches_unexpected_extension("srt"));
        assert!(!rule.matches_unexpected_extension("txt"));
        // "nfo" not in patterns → no match (even though it's ignored, it's also not in patterns)
        assert!(!rule.matches_unexpected_extension("nfo"));

        // Blacklist, patterns override IGNORED_UNKNOWN_EXTS
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec!["nfo".to_string(), "txt".to_string()],
            mode: UnexpectedFilesMode::Blacklist,
        };
        assert!(rule.matches_unexpected_extension("nfo")); // explicitly listed → override
        assert!(rule.matches_unexpected_extension("txt")); // explicitly listed
        assert!(!rule.matches_unexpected_extension("jpg")); // ignored, not in patterns
    }

    #[test]
    fn test_matches_unexpected_extension_whitelist() {
        // Whitelist, empty patterns = match nothing
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec![],
            mode: UnexpectedFilesMode::Whitelist,
        };
        assert!(!rule.matches_unexpected_extension("txt"));
        assert!(!rule.matches_unexpected_extension("nfo"));

        // Whitelist, non-empty patterns
        let rule = AutomaticProfileRule::UnexpectedFiles {
            file_patterns: vec!["srt".to_string(), "idx".to_string()],
            mode: UnexpectedFilesMode::Whitelist,
        };
        assert!(rule.matches_unexpected_extension("srt"));
        assert!(!rule.matches_unexpected_extension("txt"));
        // IGNORED_UNKNOWN_EXTS does NOT apply in whitelist mode
        assert!(!rule.matches_unexpected_extension("nfo")); // not in patterns, so no match
    }

    #[test]
    fn test_matches_unexpected_extension_non_unexpected_rule() {
        // Non-UnexpectedFiles rules always return false
        let rule = AutomaticProfileRule::Chapters {
            condition: RuleCondition::MustBePresent,
        };
        assert!(!rule.matches_unexpected_extension("txt"));
    }
}
