// Automatic profile rules implement post-download quality assurance: because media
// info is only known after download, each file is checked against the user's rules and
// satisfying files add a modifier to the submitter's score. That score ranks future
// searches — distinct from pre-download scoring in the source processor (resolution, seeders).
// The engine is keyed by category name so users can define custom categories.
use crate::db::automatic_profiles::SubmitterScoreInput;
use crate::organizer::ContentOrganizer;
use anyhow::Result;
use jumbie_shared::languages::normalize_language;

// Shared helper: split a comma/semicolon-separated value into trimmed, non-empty parts.
fn split_multi_value(val: &str) -> impl Iterator<Item = &str> + '_ {
    val.split(&[',', ';'][..])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

struct CheckRuleParams<'a> {
    valid: bool,
    submitter: &'a str,
    desc: &'a str,
    cat: &'a jumbie_shared::config::AutomaticProfileCategory,
    cat_name: &'a str,
    source_identity: Option<&'a str>,
    clear_on_pass: bool,
}

impl ContentOrganizer {
    /// Apply the category's modifier when the rule condition is satisfied.
    ///
    /// The modifier's sign is respected as-is: positive = reward, negative = penalty.
    /// `clear_on_pass` clears any existing record for this source_identity first
    /// (used during scan-time rescans where a file may have been replaced).
    async fn check_rule(&self, p: CheckRuleParams<'_>) {
        if p.clear_on_pass
            && let Some(identity) = p.source_identity
        {
            let _ = self
                .db
                .clear_submitter_score(p.submitter, p.cat_name, identity)
                .await;
        }

        // The modifier applies when the condition is SATISFIED and its sign is the
        // reward/penalty — e.g. MustBePresent with a negative modifier penalizes files
        // that have chapters. Intentional: maps conditions to scores with no separate
        // "penalty" field.
        if p.valid {
            let _ = self
                .db
                .record_submitter_score(SubmitterScoreInput {
                    submitter: p.submitter,
                    description: p.desc,
                    value: p.cat.modifier,
                    category: p.cat_name,
                    bound: p.cat.bound,
                    source_identity: p.source_identity,
                    extension: None,
                })
                .await;
        }
    }

    pub async fn check_automatic_profile_rules(
        &self,
        submitter: &str,
        filename: &str,
        media_info: &jumbie_shared::types::MediaInfo,
        categories: &std::collections::HashMap<
            String,
            jumbie_shared::config::AutomaticProfileCategory,
        >,
        source_identity: Option<&str>,
        clear_on_pass: bool,
    ) {
        use jumbie_shared::config::{AutomaticProfileRule, RuleCondition, ThresholdCondition};

        for (cat_name, cat) in categories {
            match &cat.rule {
                AutomaticProfileRule::UnexpectedFiles { .. } => {}
                AutomaticProfileRule::AudioChannels {
                    channels,
                    condition,
                } => {
                    let has_multi = media_info.audio_channels.iter().any(|&ch| match condition {
                        ThresholdCondition::Minimum => ch >= *channels,
                        ThresholdCondition::Maximum => ch <= *channels,
                        ThresholdCondition::Exact => ch == *channels,
                    });

                    let condition_str = match condition {
                        ThresholdCondition::Minimum => "min",
                        ThresholdCondition::Maximum => "max",
                        ThresholdCondition::Exact => "exactly",
                    };

                    let desc = format!(
                        "{}: audio channels {} (rule: {} {})",
                        filename,
                        media_info.audio_channels.first().copied().unwrap_or(0),
                        condition_str,
                        channels
                    );
                    self.check_rule(CheckRuleParams {
                        valid: has_multi,
                        submitter,
                        desc: &desc,
                        cat,
                        cat_name,
                        source_identity,
                        clear_on_pass,
                    })
                    .await;
                }
                AutomaticProfileRule::Resolution {
                    width,
                    height,
                    condition,
                } => {
                    let valid = match condition {
                        ThresholdCondition::Minimum => {
                            media_info.width >= *width && media_info.height >= *height
                        }
                        ThresholdCondition::Maximum => {
                            media_info.width <= *width && media_info.height <= *height
                        }
                        ThresholdCondition::Exact => {
                            media_info.width == *width && media_info.height == *height
                        }
                    };
                    let expected = match condition {
                        ThresholdCondition::Minimum => "at least",
                        ThresholdCondition::Maximum => "at most",
                        ThresholdCondition::Exact => "exactly",
                    };
                    let desc = format!(
                        "{}: resolution {}x{} (rule: {} {}x{})",
                        filename, media_info.width, media_info.height, expected, width, height
                    );
                    self.check_rule(CheckRuleParams {
                        valid,
                        submitter,
                        desc: &desc,
                        cat,
                        cat_name,
                        source_identity,
                        clear_on_pass,
                    })
                    .await;
                }
                AutomaticProfileRule::Bitrate { kbps, condition } => {
                    if let Some(bitrate_str) = &media_info.bitrate {
                        let actual_kbps = bitrate_str
                            .split_whitespace()
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);

                        let valid = match condition {
                            ThresholdCondition::Minimum => actual_kbps >= *kbps,
                            ThresholdCondition::Maximum => actual_kbps <= *kbps,
                            ThresholdCondition::Exact => actual_kbps == *kbps,
                        };

                        let expected = match condition {
                            ThresholdCondition::Minimum => "at least",
                            ThresholdCondition::Maximum => "at most",
                            ThresholdCondition::Exact => "exactly",
                        };
                        let desc = format!(
                            "{}: bitrate {} kbps (rule: {} {} kbps)",
                            filename, actual_kbps, expected, kbps
                        );
                        self.check_rule(CheckRuleParams {
                            valid,
                            submitter,
                            desc: &desc,
                            cat,
                            cat_name,
                            source_identity,
                            clear_on_pass,
                        })
                        .await;
                    }
                }
                AutomaticProfileRule::Codec { codec, condition } => {
                    if let Some(actual_codec) = &media_info.codec {
                        let actual_lower = actual_codec.to_lowercase();
                        let found = split_multi_value(codec)
                            .map(|s| s.to_lowercase())
                            .any(|t| actual_lower.contains(&t));
                        let valid = match condition {
                            RuleCondition::MustBePresent => found,
                            RuleCondition::MustNotBePresent => !found,
                        };

                        let condition_str = match condition {
                            RuleCondition::MustBePresent => "match required",
                            RuleCondition::MustNotBePresent => "not allowed",
                        };
                        let desc = format!(
                            "{}: codec includes {} (rule: {})",
                            filename, codec, condition_str
                        );
                        self.check_rule(CheckRuleParams {
                            valid,
                            submitter,
                            desc: &desc,
                            cat,
                            cat_name,
                            source_identity,
                            clear_on_pass,
                        })
                        .await;
                    }
                }
                AutomaticProfileRule::TrackCount {
                    count,
                    track_type,
                    condition,
                } => {
                    let actual = match track_type {
                        jumbie_shared::config::TrackType::Any => {
                            media_info.audio_track_count
                                + media_info.subtitle_track_count
                                + media_info.video_track_count
                        }
                        jumbie_shared::config::TrackType::Audio => media_info.audio_track_count,
                        jumbie_shared::config::TrackType::Subtitle => {
                            media_info.subtitle_track_count
                        }
                        jumbie_shared::config::TrackType::Video => media_info.video_track_count,
                    };

                    let valid = match condition {
                        ThresholdCondition::Minimum => actual >= (*count as u32),
                        ThresholdCondition::Maximum => actual <= (*count as u32),
                        ThresholdCondition::Exact => actual == (*count as u32),
                    };

                    let track_type_str = match track_type {
                        jumbie_shared::config::TrackType::Any => "any",
                        jumbie_shared::config::TrackType::Audio => "audio",
                        jumbie_shared::config::TrackType::Subtitle => "subtitle",
                        jumbie_shared::config::TrackType::Video => "video",
                    };
                    let expected = match condition {
                        ThresholdCondition::Minimum => "at least",
                        ThresholdCondition::Maximum => "at most",
                        ThresholdCondition::Exact => "exactly",
                    };
                    let desc = format!(
                        "{}: {} tracks: {} (rule: {} {})",
                        filename, track_type_str, actual, expected, count
                    );
                    self.check_rule(CheckRuleParams {
                        valid,
                        submitter,
                        desc: &desc,
                        cat,
                        cat_name,
                        source_identity,
                        clear_on_pass,
                    })
                    .await;
                }
                AutomaticProfileRule::Language {
                    language,
                    track_type,
                    condition,
                } => {
                    let actual_tracks = match track_type {
                        jumbie_shared::config::TrackType::Any => {
                            media_info.audio_track_count
                                + media_info.subtitle_track_count
                                + media_info.video_track_count
                        }
                        jumbie_shared::config::TrackType::Audio => media_info.audio_track_count,
                        jumbie_shared::config::TrackType::Subtitle => {
                            media_info.subtitle_track_count
                        }
                        jumbie_shared::config::TrackType::Video => media_info.video_track_count,
                    };

                    if actual_tracks > 0 {
                        let langs: Vec<String> = match track_type {
                            jumbie_shared::config::TrackType::Any => media_info
                                .audio_languages
                                .iter()
                                .chain(media_info.subtitle_languages.iter())
                                .chain(media_info.video_languages.iter())
                                .cloned()
                                .collect(),
                            jumbie_shared::config::TrackType::Audio => {
                                media_info.audio_languages.clone()
                            }
                            jumbie_shared::config::TrackType::Subtitle => {
                                media_info.subtitle_languages.clone()
                            }
                            jumbie_shared::config::TrackType::Video => {
                                media_info.video_languages.clone()
                            }
                        };

                        let target_langs: Vec<String> = split_multi_value(language)
                            .map(normalize_language)
                            .collect();

                        let found = langs.iter().any(|l| {
                            let norm_l = normalize_language(l);
                            target_langs
                                .iter()
                                .any(|target| norm_l.contains(target) || target.contains(&norm_l))
                        });

                        let valid = match condition {
                            RuleCondition::MustBePresent => found,
                            RuleCondition::MustNotBePresent => !found,
                        };

                        let track_type_str = match track_type {
                            jumbie_shared::config::TrackType::Any => "any",
                            jumbie_shared::config::TrackType::Audio => "audio",
                            jumbie_shared::config::TrackType::Subtitle => "subtitle",
                            jumbie_shared::config::TrackType::Video => "video",
                        };
                        let condition_str = match condition {
                            RuleCondition::MustBePresent => "match required",
                            RuleCondition::MustNotBePresent => "not allowed",
                        };
                        let desc = format!(
                            "{}: {} track language includes {} (rule: {})",
                            filename, track_type_str, language, condition_str
                        );
                        self.check_rule(CheckRuleParams {
                            valid,
                            submitter,
                            desc: &desc,
                            cat,
                            cat_name,
                            source_identity,
                            clear_on_pass,
                        })
                        .await;
                    }
                }
                AutomaticProfileRule::Chapters { condition } => {
                    let found = media_info.has_chapters;
                    let valid = match condition {
                        RuleCondition::MustBePresent => found,
                        RuleCondition::MustNotBePresent => !found,
                    };

                    let condition_str = match condition {
                        RuleCondition::MustBePresent => "match required",
                        RuleCondition::MustNotBePresent => "not allowed",
                    };
                    let desc = format!("{}: chapters present (rule: {})", filename, condition_str);
                    self.check_rule(CheckRuleParams {
                        valid,
                        submitter,
                        desc: &desc,
                        cat,
                        cat_name,
                        source_identity,
                        clear_on_pass,
                    })
                    .await;
                }
            }
        }
    }

    // Re-apply profiles to the whole library when the user changes the rules: iterate
    // all recorded media scans (the SSoT for scoring data in automatic_profile_media_scans)
    // rather than re-downloading. That table persists independently of episodes and
    // file_paths, so recalculation survives deletion of the original files.
    pub async fn reapply_automatic_profiles_to_library(&self) -> Result<()> {
        let scans = self.db.get_all_media_scans().await?;

        // Read automatic profiles from the DB (SSoT) — self.config is built from TOML
        // only and may lack DB-overlaid values.
        let general_cfg = self.db.get_general_config().await?;
        if !general_cfg.automatic_profiles.enabled {
            return Ok(());
        }
        let categories = &general_cfg.automatic_profiles.categories;

        // 1. Cleanup records for categories that no longer exist
        let _ = self.db.cleanup_orphaned_automatic_records(categories).await;

        // 2. Re-score unexpected files after config changes to file_patterns
        self.rescore_unexpected_files().await;

        // 3. Clear all records for media-info categories to recalculate from a clean
        //    slate. Otherwise original-scan records (source_identity = quick_hash) sit
        //    alongside the new ones (source_identity = scan.id), DOUBLING the score.
        let media_cat_names: Vec<String> = categories
            .keys()
            .filter(|name| {
                !matches!(
                    categories.get(*name).map(|c| &c.rule),
                    Some(jumbie_shared::config::AutomaticProfileRule::UnexpectedFiles { .. }),
                )
            })
            .cloned()
            .collect();

        if !media_cat_names.is_empty() {
            self.db
                .clear_records_by_categories(&media_cat_names)
                .await?;
        }

        // 4. Re-apply rules to all media scans
        for scan in &scans {
            if let Ok(info) =
                serde_json::from_str::<jumbie_shared::types::MediaInfo>(&scan.media_info)
            {
                self.check_automatic_profile_rules(
                    &scan.submitter,
                    &scan.filename,
                    &info,
                    categories,
                    // Scan id as identity (not the file hash) so each scan yields a unique
                    // record per category — no cross-scan de-duping. The two paths use
                    // different identities, so the bulk-clear in step 3 prevents stacking.
                    Some(&scan.id),
                    false,
                )
                .await;
            }
        }

        Ok(())
    }

    /// Rebuild all unexpected_files records from persisted (submitter, extension, count)
    /// data against the current category configuration, so the original score can be
    /// recreated even after the files are deleted.
    ///
    /// Uses `check_rule` (same as media-info rules) so the scoring path is unified:
    /// `check_rule` → `record_submitter_score`.
    pub async fn rescore_unexpected_files(&self) {
        let Ok(general_cfg) = self.db.get_general_config().await else {
            tracing::warn!("rescore_unexpected_files: failed to read general config from DB");
            return;
        };
        if !general_cfg.automatic_profiles.enabled {
            return;
        }
        let categories = &general_cfg.automatic_profiles.categories;

        let unexpected_cat_names: Vec<String> = categories
            .iter()
            .filter(|(_, cat)| {
                matches!(
                    cat.rule,
                    jumbie_shared::config::AutomaticProfileRule::UnexpectedFiles { .. }
                )
            })
            .map(|(name, _)| name.clone())
            .collect();

        if unexpected_cat_names.is_empty() {
            return;
        }

        // Bulk-clear existing records, then rebuild.
        if let Err(e) = self
            .db
            .clear_records_by_categories(&unexpected_cat_names)
            .await
        {
            tracing::warn!("Failed to clear records for rescoring: {}", e);
            return;
        }

        let unknown_extensions = match self.db.get_unknown_file_extensions().await {
            Ok(exts) => exts,
            Err(e) => {
                tracing::warn!(
                    "Failed to load unknown file extensions for rescoring: {}",
                    e
                );
                return;
            }
        };

        for (submitter, ext, count) in &unknown_extensions {
            for (cat_name, cat) in categories {
                if !cat.rule.matches_unexpected_extension(ext) {
                    continue;
                }

                // Each occurrence gets a unique synthetic identity so repeated
                // rescoring produces the same number of records (no dedup).
                for i in 0..*count {
                    let synthetic_id = format!("rescore:{}:{}:{}", ext, cat_name, i);
                    let desc = format!("Unexpected file extension '.{}' (rescored)", ext);
                    self.check_rule(CheckRuleParams {
                        valid: true, // already matched; always apply
                        submitter,
                        desc: &desc,
                        cat,
                        cat_name,
                        source_identity: Some(&synthetic_id),
                        clear_on_pass: false,
                    })
                    .await;
                }
            }
        }
    }
}
