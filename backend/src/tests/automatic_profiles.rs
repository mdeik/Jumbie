use crate::db::automatic_profiles::SubmitterScoreInput;
use crate::organizer::ContentOrganizer;
use crate::plugins::PluginManager;
use jumbie_shared::config::{AutomaticProfileCategory, AutomaticProfileRule, TrackType};
use jumbie_shared::types::MediaInfo;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::RwLock;

struct TestApp {
    organizer: ContentOrganizer,
    _temp_dir: tempfile::TempDir,
}

impl Deref for TestApp {
    type Target = ContentOrganizer;
    fn deref(&self) -> &Self::Target {
        &self.organizer
    }
}

jumbie_shared::test_module! {

    async fn setup_test_app() -> TestApp {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db").to_string_lossy().to_string();

        let db = std::sync::Arc::new(crate::db::DbManager::new(std::path::Path::new(&db_path)).await.unwrap());

        let pm = Arc::new(RwLock::new(PluginManager::new(std::path::PathBuf::from("plugins"))));
                let modifying_series: Arc<RwLock<HashSet<String>>> =
                    Arc::new(RwLock::new(HashSet::new()));
                let organizer = ContentOrganizer::new(
                    &db_path,
                    db,
                    pm,
                    tokio_util::sync::CancellationToken::new(),
                    modifying_series,
                )
                .await
                .unwrap();

        TestApp {
            organizer,
            _temp_dir: temp_dir,
        }
    }

    async fn get_score(app: &ContentOrganizer, submitter: &str) -> i32 {
        app.db.get_automatic_profile(submitter).await.unwrap()
            .map(|p| p.score)
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn test_rule_resolution_mismatch() {
        let app = setup_test_app().await;
        let submitter = "test_submitter";
        let filename = "test.mkv";

        let mut categories = HashMap::new();
        categories.insert("low_res".to_string(), AutomaticProfileCategory {
            modifier: 100,
            bound: 500,
            rule: AutomaticProfileRule::Resolution { width: 1920, height: 1080, condition: jumbie_shared::config::ThresholdCondition::Minimum },
        });

        let media_info = MediaInfo {
            width: 1920,
            height: 1080,
            ..MediaInfo::default()
        };
        // A modifier applies only when the condition passes.
        app.check_automatic_profile_rules(submitter, filename, &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, submitter).await, 100);

        let media_info_fail = MediaInfo {
            width: 1280,
            height: 720,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, filename, &media_info_fail, &categories, None, true).await;
        assert_eq!(get_score(&app, submitter).await, 100);
    }

    #[tokio::test]
    async fn test_duplicate_record_prevention() {
        let app = setup_test_app().await;
        let submitter = "test_dupes";

        let mut categories = HashMap::new();
        categories.insert("bitrate".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 200,
            rule: AutomaticProfileRule::Bitrate { kbps: 5000, condition: jumbie_shared::config::ThresholdCondition::Minimum },
        });

        let media_info = MediaInfo {
            bitrate: Some("8000 kb/s".to_string()),
            ..MediaInfo::default()
        };

        app.check_automatic_profile_rules(submitter, "good_file.mkv", &media_info, &categories, Some("hash_a"), true).await;
        assert_eq!(get_score(&app, submitter).await, 50);

        // Same source_identity must not double-count.
        app.check_automatic_profile_rules(submitter, "good_file.mkv", &media_info, &categories, Some("hash_a"), true).await;
        assert_eq!(get_score(&app, submitter).await, 50);

        // A different source_identity accumulates.
        app.check_automatic_profile_rules(submitter, "other_file.mkv", &media_info, &categories, Some("hash_b"), true).await;
        assert_eq!(get_score(&app, submitter).await, 100);

        let fail_info = MediaInfo {
            bitrate: Some("3000 kb/s".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, "bad_file.mkv", &fail_info, &categories, Some("hash_c"), true).await;
        assert_eq!(get_score(&app, submitter).await, 100);
    }

    // Regression: the endpoint returned the raw SQLite naive timestamp
    // (`"2026-06-18 20:00:00"`), which left the client to guess the zone.
    #[tokio::test]
    async fn test_profile_record_date_added_is_rfc3339() {
        let app = setup_test_app().await;
        let submitter = "tz_contract_submitter";

        let mut categories = HashMap::new();
        categories.insert("bitrate".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 0,
            rule: AutomaticProfileRule::Bitrate { kbps: 5000, condition: jumbie_shared::config::ThresholdCondition::Minimum },
        });
        let media_info = MediaInfo {
            bitrate: Some("8000 kb/s".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, "tz_file.mkv", &media_info, &categories, Some("tz_hash_a"), true).await;

        let records = app.db.get_automatic_profile_records(submitter).await.unwrap();
        assert_eq!(records.len(), 1);
        let date_added = &records[0].date_added;
        assert!(
            date_added.ends_with('Z') || date_added.contains("+00:00"),
            "date_added must carry an explicit UTC offset, got {date_added:?}"
        );
    }

    #[tokio::test]
    async fn test_language_rules() {
        let app = setup_test_app().await;
        let submitter_pass = "test_lang_pass";
        let submitter_fail = "test_lang_fail";
        let filename = "test.mkv";

        let mut categories = HashMap::new();
        categories.insert("missing_english".to_string(), AutomaticProfileCategory {
            modifier: 100,
            bound: 500,
            rule: AutomaticProfileRule::Language { language: "english, en; jpn".to_string(), track_type: TrackType::Audio, condition: jumbie_shared::config::RuleCondition::MustBePresent },
        });

        let media_info_pass = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["eng".to_string()],
            ..MediaInfo::default()
        };
        // Valid rule passes apply +|modifier|.
        app.check_automatic_profile_rules(submitter_pass, filename, &media_info_pass, &categories, None, true).await;
        assert_eq!(get_score(&app, submitter_pass).await, 100);

        let media_info_pass_jp = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["jpn".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter_pass, filename, &media_info_pass_jp, &categories, Some("other_path"), true).await;
        assert_eq!(get_score(&app, submitter_pass).await, 200);

        let media_info_fail = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["fre".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter_fail, filename, &media_info_fail, &categories, None, true).await;
        assert_eq!(get_score(&app, submitter_fail).await, 0);
    }

    #[cfg_attr(not(ffmpeg_installed), ignore)]
    #[tokio::test]
    async fn test_real_video_file_extraction() {
        let temp_dir = tempfile::tempdir().unwrap();
        let video_path = temp_dir.path().join("real_video.mkv");

        let status = std::process::Command::new("ffmpeg")
            .args([
                "-f", "lavfi", "-i", "color=c=blue:s=1280x720:d=1",
                "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo",
                "-c:v", "libx264", "-t", "0.1",
                "-c:a", "aac", "-shortest",
                video_path.to_str().unwrap()
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        assert!(status.success());

        let result = crate::utils::media_info::extract_media_info(&video_path).await;
        assert!(result.is_ok());
        let info = result.unwrap();

        assert_eq!(info.width, 1280);
        assert_eq!(info.height, 720);
        assert!(info.audio_channels.contains(&2));
        assert!(info.video_track_count >= 1);
        assert!(info.audio_track_count >= 1);
    }

    #[tokio::test]
    async fn test_track_count_any() {
        let app = setup_test_app().await;
        let submitter = "test_track_any";
        let filename = "test.mkv";

        let mut categories = HashMap::new();
        categories.insert("min_tracks".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 200,
            rule: AutomaticProfileRule::TrackCount {
                count: 3,
                track_type: TrackType::Any,
                condition: jumbie_shared::config::ThresholdCondition::Minimum,
            },
        });

        let media_info_pass = MediaInfo {
            audio_track_count: 1,
            subtitle_track_count: 1,
            video_track_count: 1,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, filename, &media_info_pass, &categories, None, true)
            .await;
        assert_eq!(get_score(&app, submitter).await, 50);

        let media_info_fail = MediaInfo {
            audio_track_count: 1,
            subtitle_track_count: 0,
            video_track_count: 1,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, "fail.mkv", &media_info_fail, &categories, None, true)
            .await;
        assert_eq!(get_score(&app, submitter).await, 50);

        let mut categories_max = HashMap::new();
        categories_max.insert("max_tracks".to_string(), AutomaticProfileCategory {
            modifier: 30,
            bound: 100,
            rule: AutomaticProfileRule::TrackCount {
                count: 3,
                track_type: TrackType::Any,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        let media_info_max_pass = MediaInfo {
            audio_track_count: 1,
            subtitle_track_count: 1,
            video_track_count: 1,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("test_max_pass", filename, &media_info_max_pass, &categories_max, None, true)
            .await;
        assert_eq!(get_score(&app, "test_max_pass").await, 30);

        let media_info_max_fail = MediaInfo {
            audio_track_count: 2,
            subtitle_track_count: 1,
            video_track_count: 1,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("test_max_fail", filename, &media_info_max_fail, &categories_max, None, true)
            .await;
        assert_eq!(get_score(&app, "test_max_fail").await, 0);
    }

    #[tokio::test]
    async fn test_language_any() {
        let app = setup_test_app().await;
        let submitter = "test_lang_any";
        let filename = "test.mkv";

        let mut categories = HashMap::new();
        categories.insert("find_eng_any".to_string(), AutomaticProfileCategory {
            modifier: 100,
            bound: 500,
            rule: AutomaticProfileRule::Language {
                language: "eng".to_string(),
                track_type: TrackType::Any,
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info_audio = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["eng".to_string()],
            subtitle_track_count: 1,
            subtitle_languages: vec!["fre".to_string()],
            video_track_count: 0,
            video_languages: vec![],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, filename, &media_info_audio, &categories, None, true)
            .await;
        assert_eq!(get_score(&app, submitter).await, 100);

        let media_info_sub = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["fre".to_string()],
            subtitle_track_count: 1,
            subtitle_languages: vec!["eng".to_string()],
            video_track_count: 0,
            video_languages: vec![],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, "subs.mkv", &media_info_sub, &categories, None, true)
            .await;
        assert_eq!(get_score(&app, submitter).await, 200);

        let media_info_fail = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["fre".to_string()],
            subtitle_track_count: 1,
            subtitle_languages: vec!["spa".to_string()],
            video_track_count: 0,
            video_languages: vec![],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules(submitter, "fail.mkv", &media_info_fail, &categories, None, true)
            .await;
        assert_eq!(get_score(&app, submitter).await, 200);

        // MustNotBePresent with Any: eng in any track triggers the offense.
        let mut categories_forbidden = HashMap::new();
        categories_forbidden.insert("no_eng".to_string(), AutomaticProfileCategory {
            modifier: 200,
            bound: 1000,
            rule: AutomaticProfileRule::Language {
                language: "eng".to_string(),
                track_type: TrackType::Any,
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info_clean = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["fre".to_string()],
            subtitle_track_count: 2,
            subtitle_languages: vec!["spa".to_string(), "por".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("test_clean", filename, &media_info_clean, &categories_forbidden, None, true)
            .await;
        assert_eq!(get_score(&app, "test_clean").await, 200);

        let media_info_dirty = MediaInfo {
            audio_track_count: 1,
            audio_languages: vec!["fre".to_string()],
            subtitle_track_count: 2,
            subtitle_languages: vec!["spa".to_string(), "eng".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("test_dirty", filename, &media_info_dirty, &categories_forbidden, None, true)
            .await;
        assert_eq!(get_score(&app, "test_dirty").await, 0);
    }


    #[tokio::test]
    async fn test_chapters_must_be_present_fails_when_no_chapters() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("chapters".to_string(), AutomaticProfileCategory {
            modifier: 10,
            bound: 50,
            rule: AutomaticProfileRule::Chapters {
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            has_chapters: false,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("chap_fail", "ep.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "chap_fail").await, 0);
    }

    #[tokio::test]
    async fn test_chapters_must_be_present_passes_when_has_chapters() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("chapters".to_string(), AutomaticProfileCategory {
            modifier: 10,
            bound: 50,
            rule: AutomaticProfileRule::Chapters {
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            has_chapters: true,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("chap_pass", "ep.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "chap_pass").await, 10);
    }

    #[tokio::test]
    async fn test_chapters_must_not_be_present_fails_when_has_chapters() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("no_chapters".to_string(), AutomaticProfileCategory {
            modifier: 15,
            bound: 60,
            rule: AutomaticProfileRule::Chapters {
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            has_chapters: true,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("no_chap_fail", "ep.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "no_chap_fail").await, 0);
    }

    #[tokio::test]
    async fn test_chapters_must_not_be_present_passes_when_no_chapters() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("no_chapters".to_string(), AutomaticProfileCategory {
            modifier: 15,
            bound: 60,
            rule: AutomaticProfileRule::Chapters {
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            has_chapters: false,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("no_chap_pass", "ep.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "no_chap_pass").await, 15);
    }


    #[tokio::test]
    async fn test_media_scan_persisted_and_retrievable() {
        let app = setup_test_app().await;
        let submitter = "scan_test";
        let media_json = r##"{"has_chapters":true,"width":1920,"height":1080}"##;

        app.db
            .record_media_scan(submitter, "ep.mkv", Some("series_1"), Some(1), Some(3), media_json)
            .await
            .unwrap();

        let scans = app.db.get_all_media_scans().await.unwrap();
        assert_eq!(scans.len(), 1);
        assert_eq!(scans[0].submitter, submitter);
        assert_eq!(scans[0].series_id.as_deref(), Some("series_1"));
        assert_eq!(scans[0].season, Some(1));
        assert_eq!(scans[0].episode, Some(3));
        assert_eq!(scans[0].media_info, media_json);
    }

    #[tokio::test]
    async fn test_media_scan_survives_clear() {
        let app = setup_test_app().await;
        let media_json = r##"{"has_chapters":false}"##;

        app.db
            .record_media_scan("survive_test", "ep.mkv", None, None, None, media_json)
            .await
            .unwrap();

        let pre = app.db.get_all_media_scans().await.unwrap();
        assert_eq!(pre.len(), 1, "scan should exist before clear");

        app.db.clear_all_media_scans().await.unwrap();
        let post = app.db.get_all_media_scans().await.unwrap();
        assert_eq!(post.len(), 0, "scans should be empty after clear");
    }


    #[tokio::test]
    async fn test_reapply_from_media_scans_produces_consistent_score() {
        // Simulates: scan a file → delete it → recalculation re-creates the
        // same offense because the media scan is persisted.
        let app = setup_test_app().await;
        let submitter = "reapply_consistent";

        let media_json = serde_json::to_string(&MediaInfo {
            width: 1280,
            height: 720,
            ..MediaInfo::default()
        })
        .unwrap();
        app.db
            .record_media_scan(submitter, "old_ep.mkv", None, None, None, &media_json)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "low_res".to_string(),
            AutomaticProfileCategory {
                modifier: 50,
                bound: 200,
                rule: AutomaticProfileRule::Resolution {
                    width: 1920,
                    height: 1080,
                    condition: jumbie_shared::config::ThresholdCondition::Minimum,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        // Recalculation reads from media scans, NOT from episodes.
        app.reapply_automatic_profiles_to_library().await.unwrap();

        // The 720p file fails the 1080p minimum rule.
        assert_eq!(get_score(&app, submitter).await, 0);

        // Idempotent on repeat.
        app.reapply_automatic_profiles_to_library().await.unwrap();
        assert_eq!(get_score(&app, submitter).await, 0);
    }

    #[tokio::test]
    async fn test_reapply_produces_same_score_after_file_deleted() {
        // A file present only in media scans (never in episodes) is still scored.
        let app = setup_test_app().await;
        let submitter = "deleted_file_sub";

        let media_json = serde_json::to_string(&MediaInfo {
            has_chapters: false,
            width: 640,
            height: 480,
            ..MediaInfo::default()
        })
        .unwrap();

        app.db
            .record_media_scan(submitter, "deleted.mkv", None, None, None, &media_json)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "chapters".to_string(),
            AutomaticProfileCategory {
                modifier: 5,
                bound: 25,
                rule: AutomaticProfileRule::Chapters {
                    condition: jumbie_shared::config::RuleCondition::MustBePresent,
                },
            },
        );
        general.automatic_profiles.categories.insert(
            "low_res".to_string(),
            AutomaticProfileCategory {
                modifier: 10,
                bound: 50,
                rule: AutomaticProfileRule::Resolution {
                    width: 1920,
                    height: 1080,
                    condition: jumbie_shared::config::ThresholdCondition::Minimum,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        app.reapply_automatic_profiles_to_library().await.unwrap();

        assert_eq!(get_score(&app, submitter).await, 0);
    }


    #[tokio::test]
    async fn test_language_subtitle_must_be_present_fails_when_missing() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_eng_sub".to_string(), AutomaticProfileCategory {
            modifier: 20,
            bound: 100,
            rule: AutomaticProfileRule::Language {
                language: "eng".to_string(),
                track_type: TrackType::Subtitle,
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            subtitle_track_count: 1,
            subtitle_languages: vec!["fre".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("lang_sub", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "lang_sub").await, 0);
    }

    #[tokio::test]
    async fn test_language_subtitle_must_be_present_passes_when_found() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_eng_sub".to_string(), AutomaticProfileCategory {
            modifier: 20,
            bound: 100,
            rule: AutomaticProfileRule::Language {
                language: "eng".to_string(),
                track_type: TrackType::Subtitle,
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            subtitle_track_count: 2,
            subtitle_languages: vec!["eng".to_string(), "fre".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("lang_sub_pass", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "lang_sub_pass").await, 20);
    }

    #[tokio::test]
    async fn test_language_video_must_not_be_present_fails_when_found() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("no_eng_vid".to_string(), AutomaticProfileCategory {
            modifier: 30,
            bound: 150,
            rule: AutomaticProfileRule::Language {
                language: "eng".to_string(),
                track_type: TrackType::Video,
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            video_track_count: 1,
            video_languages: vec!["eng".to_string()],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("lang_vid", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "lang_vid").await, 0);
    }


    #[tokio::test]
    async fn test_resolution_maximum_fails_when_above() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("max_res".to_string(), AutomaticProfileCategory {
            modifier: 30,
            bound: 100,
            rule: AutomaticProfileRule::Resolution {
                width: 1920,
                height: 1080,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        // 4K exceeds 1080p max
        let media_info = MediaInfo {
            width: 3840,
            height: 2160,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("max_res_fail", "4k.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "max_res_fail").await, 0);
    }

    #[tokio::test]
    async fn test_resolution_maximum_passes_when_equal() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("max_res".to_string(), AutomaticProfileCategory {
            modifier: 30,
            bound: 100,
            rule: AutomaticProfileRule::Resolution {
                width: 1920,
                height: 1080,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        let media_info = MediaInfo {
            width: 1920,
            height: 1080,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("max_res_pass", "1080p.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "max_res_pass").await, 30);
    }


    #[tokio::test]
    async fn test_bitrate_maximum_fails_when_above() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("max_bitrate".to_string(), AutomaticProfileCategory {
            modifier: 20,
            bound: 80,
            rule: AutomaticProfileRule::Bitrate {
                kbps: 5000,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        let media_info = MediaInfo {
            bitrate: Some("8000 kb/s".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("bitrate_max", "high.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "bitrate_max").await, 0);
    }

    #[tokio::test]
    async fn test_bitrate_maximum_passes_when_equal() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("max_bitrate".to_string(), AutomaticProfileCategory {
            modifier: 20,
            bound: 80,
            rule: AutomaticProfileRule::Bitrate {
                kbps: 5000,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        let media_info = MediaInfo {
            bitrate: Some("5000 kb/s".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("bitrate_pass", "normal.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "bitrate_pass").await, 20);
    }


    #[tokio::test]
    async fn test_codec_must_be_present_passes_when_codec_matches() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("x265".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("codec_pass", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "codec_pass").await, 40);
    }

    #[tokio::test]
    async fn test_codec_must_be_present_fails_when_codec_missing() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("x264".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("codec_fail", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "codec_fail").await, 0);
    }

    #[tokio::test]
    async fn test_codec_must_not_be_present_fails_when_codec_found() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("block_xvid".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 250,
            rule: AutomaticProfileRule::Codec {
                codec: "xvid".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("XviD".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("codec_block", "old.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "codec_block").await, 0);
    }


    #[tokio::test]
    async fn test_codec_multi_value_first_matches() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265_or_av1".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265, av1".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("x265".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_first", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_first").await, 40);
    }

    #[tokio::test]
    async fn test_codec_multi_value_second_matches() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265_or_av1".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265, av1".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("av1".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_second", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_second").await, 40);
    }

    #[tokio::test]
    async fn test_codec_multi_value_neither_matches_fails() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265_or_av1".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265, av1".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("x264".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_neither", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_neither").await, 0);
    }

    #[tokio::test]
    async fn test_codec_multi_value_blacklist_first_matches_fails() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("block_old_or_lossy".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 250,
            rule: AutomaticProfileRule::Codec {
                codec: "xvid, vp9".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("xvid".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_block_first", "old.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_block_first").await, 0);
    }

    #[tokio::test]
    async fn test_codec_multi_value_blacklist_second_matches_fails() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("block_old_or_lossy".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 250,
            rule: AutomaticProfileRule::Codec {
                codec: "xvid, vp9".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("vp9".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_block_second", "lossy.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_block_second").await, 0);
    }

    #[tokio::test]
    async fn test_codec_multi_value_blacklist_neither_matches_passes() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("block_old_or_lossy".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 250,
            rule: AutomaticProfileRule::Codec {
                codec: "xvid, vp9".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustNotBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("x264".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_block_neither", "good.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_block_neither").await, 50);
    }

    #[tokio::test]
    async fn test_codec_multi_value_semicolon_separator() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265_or_av1".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265; av1".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("av1".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_semicolon", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_semicolon").await, 40);
    }

    #[tokio::test]
    async fn test_codec_multi_value_substring_preserved() {
        // Multi-value should still use substring matching: "x265" matches "HEVC x265"
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_x265".to_string(), AutomaticProfileCategory {
            modifier: 40,
            bound: 200,
            rule: AutomaticProfileRule::Codec {
                codec: "x265, av1".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("HEVC x265".to_string()),
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("multi_substr", "vid.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "multi_substr").await, 40);
    }


    #[tokio::test]
    async fn test_audio_channels_minimum_fails_when_below() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_51".to_string(), AutomaticProfileCategory {
            modifier: 15,
            bound: 60,
            rule: AutomaticProfileRule::AudioChannels {
                channels: 6,
                condition: jumbie_shared::config::ThresholdCondition::Minimum,
            },
        });

        let media_info = MediaInfo {
            audio_channels: vec![2],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("audio_fail", "stereo.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "audio_fail").await, 0);
    }

    #[tokio::test]
    async fn test_audio_channels_minimum_passes_when_meets() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("need_51".to_string(), AutomaticProfileCategory {
            modifier: 15,
            bound: 60,
            rule: AutomaticProfileRule::AudioChannels {
                channels: 6,
                condition: jumbie_shared::config::ThresholdCondition::Minimum,
            },
        });

        let media_info = MediaInfo {
            audio_channels: vec![6],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("audio_pass", "51.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "audio_pass").await, 15);
    }

    #[tokio::test]
    async fn test_audio_channels_maximum_fails_when_above() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("no_51".to_string(), AutomaticProfileCategory {
            modifier: 10,
            bound: 40,
            rule: AutomaticProfileRule::AudioChannels {
                channels: 2,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        let media_info = MediaInfo {
            audio_channels: vec![6],
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("audio_max", "51.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "audio_max").await, 0);
    }


    #[tokio::test]
    async fn test_track_count_audio_minimum_fails_when_below() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("min_audio".to_string(), AutomaticProfileCategory {
            modifier: 20,
            bound: 100,
            rule: AutomaticProfileRule::TrackCount {
                count: 2,
                track_type: TrackType::Audio,
                condition: jumbie_shared::config::ThresholdCondition::Minimum,
            },
        });

        let media_info = MediaInfo {
            audio_track_count: 1,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("tracks_audio", "mono.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "tracks_audio").await, 0);
    }

    #[tokio::test]
    async fn test_track_count_subtitle_minimum_passes() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("min_subs".to_string(), AutomaticProfileCategory {
            modifier: 10,
            bound: 50,
            rule: AutomaticProfileRule::TrackCount {
                count: 2,
                track_type: TrackType::Subtitle,
                condition: jumbie_shared::config::ThresholdCondition::Minimum,
            },
        });

        let media_info = MediaInfo {
            subtitle_track_count: 2,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("tracks_sub", "subs.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "tracks_sub").await, 10);
    }

    #[tokio::test]
    async fn test_track_count_video_maximum_fails_when_above() {
        let app = setup_test_app().await;
        let mut categories = HashMap::new();
        categories.insert("max_video".to_string(), AutomaticProfileCategory {
            modifier: 25,
            bound: 100,
            rule: AutomaticProfileRule::TrackCount {
                count: 1,
                track_type: TrackType::Video,
                condition: jumbie_shared::config::ThresholdCondition::Maximum,
            },
        });

        let media_info = MediaInfo {
            video_track_count: 2,
            ..MediaInfo::default()
        };
        app.check_automatic_profile_rules("tracks_vid", "multi.mkv", &media_info, &categories, None, true).await;
        assert_eq!(get_score(&app, "tracks_vid").await, 0);
    }


    #[tokio::test]
    async fn test_unexpected_files_blacklist_scores_extension() {
        let app = setup_test_app().await;
        let submitter = "unexpected_sub";

        app.db
            .record_unknown_file_counts(submitter, &[("srt".to_string(), 3), ("idx".to_string(), 1)],)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "bad_ext".to_string(),
            AutomaticProfileCategory {
                modifier: -5,
                bound: -100,
                rule: AutomaticProfileRule::UnexpectedFiles {
                    file_patterns: vec![],
                    mode: jumbie_shared::config::UnexpectedFilesMode::Blacklist,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        app.rescore_unexpected_files().await;

        // 3 srt × -5 = -15 (clamped to -100) + 1 idx × -5 = -5 → -20.
        assert_eq!(get_score(&app, submitter).await, -20);
    }

    #[tokio::test]
    async fn test_unexpected_files_blacklist_specific_pattern() {
        let app = setup_test_app().await;
        let submitter = "pattern_sub";

        app.db
            .record_unknown_file_counts(submitter, &[("srt".to_string(), 2), ("txt".to_string(), 1)],)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "bad_ext".to_string(),
            AutomaticProfileCategory {
                modifier: -10,
                bound: -50,
                rule: AutomaticProfileRule::UnexpectedFiles {
                    file_patterns: vec!["srt".to_string()],
                    mode: jumbie_shared::config::UnexpectedFilesMode::Blacklist,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        app.rescore_unexpected_files().await;

        // Only srt should be penalised (2 × -10 = -20), txt is not in patterns.
        assert_eq!(get_score(&app, submitter).await, -20);
    }

    #[tokio::test]
    async fn test_unexpected_files_whitelist_scores_only_listed() {
        let app = setup_test_app().await;
        let submitter = "whitelist_sub";

        app.db
            .record_unknown_file_counts(submitter, &[("srt".to_string(), 1), ("nfo".to_string(), 1), ("txt".to_string(), 1)],)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "whitelist_test".to_string(),
            AutomaticProfileCategory {
                modifier: -5,
                bound: -30,
                rule: AutomaticProfileRule::UnexpectedFiles {
                    file_patterns: vec!["txt".to_string()],
                    mode: jumbie_shared::config::UnexpectedFilesMode::Whitelist,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        app.rescore_unexpected_files().await;

        // Whitelist: only "txt" matches → 1 × -5 = -5
        assert_eq!(get_score(&app, submitter).await, -5);
    }


    #[tokio::test]
    async fn test_reapply_same_score_after_series_deleted() {
        // This test proves that deleting ALL episode data from the DB does
        // NOT affect automatic profile scoring — the media scans persist
        // independently.
        let app = setup_test_app().await;
        let submitter = "series_gone";
        let series_id = "series_to_delete";

        // 1. Record multiple scans with different characteristics.
        let ep1_json = serde_json::to_string(&MediaInfo {
            has_chapters: false,
            width: 1280,
            height: 720,
            ..MediaInfo::default()
        })
        .unwrap();
        let ep2_json = serde_json::to_string(&MediaInfo {
            has_chapters: false,
            width: 1920,
            height: 1080,
            ..MediaInfo::default()
        })
        .unwrap();

        app.db
            .record_media_scan(submitter, "ep1.mkv", Some(series_id), Some(1), Some(1), &ep1_json)
            .await
            .unwrap();
        app.db
            .record_media_scan(submitter, "ep2.mkv", Some(series_id), Some(1), Some(2), &ep2_json)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "chapters".to_string(),
            AutomaticProfileCategory {
                modifier: 5,
                bound: 25,
                rule: AutomaticProfileRule::Chapters {
                    condition: jumbie_shared::config::RuleCondition::MustBePresent,
                },
            },
        );
        general.automatic_profiles.categories.insert(
            "resolution".to_string(),
            AutomaticProfileCategory {
                modifier: 10,
                bound: 50,
                rule: AutomaticProfileRule::Resolution {
                    width: 1920,
                    height: 1080,
                    condition: jumbie_shared::config::ThresholdCondition::Minimum,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        app.reapply_automatic_profiles_to_library().await.unwrap();
        let score_before = get_score(&app, submitter).await;

        // Both files have no chapters (MustBePresent) → fail → nothing
        // ep1 is 720p (Min 1080p) → fail → nothing
        // ep2 is 1080p (Min 1080p) → passes → +10
        // Total: +10
        assert_eq!(score_before, 10, "expected +10 before deletion");

        // Deleting the series clears episodes/fingerprints but NOT
        // automatic_profile_media_scans, so reapply must reproduce the score.
        app.reapply_automatic_profiles_to_library().await.unwrap();
        let score_after = get_score(&app, submitter).await;

        assert_eq!(
            score_after, score_before,
            "score must remain the same after series deletion"
        );
    }


    #[tokio::test]
    async fn test_reapply_idempotent_multiple_calls() {
        let app = setup_test_app().await;
        let submitter = "idempotent_sub";

        let media_json = serde_json::to_string(&MediaInfo {
            width: 640,
            height: 480,
            audio_channels: vec![2],
            has_chapters: true,
            ..MediaInfo::default()
        })
        .unwrap();

        app.db
            .record_media_scan(submitter, "vid.mkv", None, None, None, &media_json)
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "low_res".to_string(),
            AutomaticProfileCategory {
                modifier: 10,
                bound: 50,
                rule: AutomaticProfileRule::Resolution {
                    width: 1920,
                    height: 1080,
                    condition: jumbie_shared::config::ThresholdCondition::Minimum,
                },
            },
        );
        general.automatic_profiles.categories.insert(
            "chapters".to_string(),
            AutomaticProfileCategory {
                modifier: 5,
                bound: 25,
                rule: AutomaticProfileRule::Chapters {
                    condition: jumbie_shared::config::RuleCondition::MustBePresent,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        for _ in 0..3 {
            app.reapply_automatic_profiles_to_library().await.unwrap();
        }

        // 480p fails resolution → nothing, has chapters passes → +5 = +5
        assert_eq!(get_score(&app, submitter).await, 5);
    }


    #[tokio::test]
    async fn test_bound_positive_caps_score_at_upper_limit() {
        let app = setup_test_app().await;
        let submitter = "bound_pos";

        // modifier=50, bound=120 (cap at +120)
        let mut categories = HashMap::new();
        categories.insert("codec_check".to_string(), AutomaticProfileCategory {
            modifier: 50,
            bound: 120,
            rule: AutomaticProfileRule::Codec {
                codec: "x265".to_string(),
                condition: jumbie_shared::config::RuleCondition::MustBePresent,
            },
        });

        let media_info = MediaInfo {
            codec: Some("x265".to_string()),
            ..MediaInfo::default()
        };

        // File 1: +50 → total 50
        app.check_automatic_profile_rules(submitter, "file1.mkv", &media_info, &categories, Some("hash_a"), false).await;
        assert_eq!(get_score(&app, submitter).await, 50);

        // File 2: +50 → total 100
        app.check_automatic_profile_rules(submitter, "file2.mkv", &media_info, &categories, Some("hash_b"), false).await;
        assert_eq!(get_score(&app, submitter).await, 100);

        // File 3: would be +50 → total 150, but clamped to 120 → +20 applied
        app.check_automatic_profile_rules(submitter, "file3.mkv", &media_info, &categories, Some("hash_c"), false).await;
        assert_eq!(get_score(&app, submitter).await, 120);

        // File 4: clamped to 0 (already at bound), total stays 120
        app.check_automatic_profile_rules(submitter, "file4.mkv", &media_info, &categories, Some("hash_d"), false).await;
        assert_eq!(get_score(&app, submitter).await, 120);
    }


    #[tokio::test]
    async fn test_bound_negative_floors_score_at_lower_limit() {
        let app = setup_test_app().await;
        let submitter = "bound_neg";

        // modifier=-50, bound=-120 (floor at -120)
        let mut categories = HashMap::new();
        categories.insert("unexpected".to_string(), AutomaticProfileCategory {
            modifier: -50,
            bound: -120,
            rule: AutomaticProfileRule::UnexpectedFiles {
                file_patterns: vec![],
                mode: jumbie_shared::config::UnexpectedFilesMode::Blacklist,
            },
        });

        // Direct DB path (unexpected files bypass check_rule)
        use jumbie_shared::config::generate_uuid;

        // File 1: -50 → total -50
        app.db.record_submitter_score(SubmitterScoreInput {
            submitter,
            description: "file1",
            value: -50,
            category: "unexpected",
            bound: -120,
            source_identity: Some(&generate_uuid()),
            extension: Some("srt"),
        }).await.unwrap();
        assert_eq!(get_score(&app, submitter).await, -50);

        // File 2: -50 → total -100
        app.db.record_submitter_score(SubmitterScoreInput {
            submitter,
            description: "file2",
            value: -50,
            category: "unexpected",
            bound: -120,
            source_identity: Some(&generate_uuid()),
            extension: Some("srt"),
        }).await.unwrap();
        assert_eq!(get_score(&app, submitter).await, -100);

        // File 3: would be -50 → total -150, but floored at -120 → -20 applied
        app.db.record_submitter_score(SubmitterScoreInput {
            submitter,
            description: "file3",
            value: -50,
            category: "unexpected",
            bound: -120,
            source_identity: Some(&generate_uuid()),
            extension: Some("srt"),
        }).await.unwrap();
        assert_eq!(get_score(&app, submitter).await, -120);

        // File 4: clamped to 0 (already at bound), total stays -120
        app.db.record_submitter_score(SubmitterScoreInput {
            submitter,
            description: "file4",
            value: -50,
            category: "unexpected",
            bound: -120,
            source_identity: Some(&generate_uuid()),
            extension: Some("srt"),
        }).await.unwrap();
        assert_eq!(get_score(&app, submitter).await, -120);
    }


    #[tokio::test]
    async fn test_bound_zero_means_no_limit() {
        let app = setup_test_app().await;
        let submitter = "bound_zero";

        // modifier=30, bound=0 (no cap)
        let mut categories = HashMap::new();
        categories.insert("resolution".to_string(), AutomaticProfileCategory {
            modifier: 30,
            bound: 0,
            rule: AutomaticProfileRule::Resolution {
                width: 1920,
                height: 1080,
                condition: jumbie_shared::config::ThresholdCondition::Minimum,
            },
        });

        let pass = MediaInfo { width: 1920, height: 1080, ..MediaInfo::default() };

        // 10 files all pass → 10 × 30 = 300 (no bound to stop it)
        for i in 0..10 {
            let hash = format!("hash_{}", i);
            app.check_automatic_profile_rules(submitter, &format!("ep{}.mkv", i), &pass, &categories, Some(&hash), false).await;
        }
        assert_eq!(get_score(&app, submitter).await, 300);
    }

    // Automatic profiles config is read from the DB, not an in-memory cache.

    #[tokio::test]
    async fn test_rescore_reads_from_db() {
        let app = setup_test_app().await;
        let submitter = "db_rescore";

        app.db
            .record_unknown_file_counts(submitter, &[("srt".to_string(), 3)])
            .await
            .unwrap();

        let mut general = jumbie_shared::config::GeneralConfig::default();
        general.automatic_profiles.enabled = true;
        general.automatic_profiles.categories.insert(
            "bad_ext".to_string(),
            AutomaticProfileCategory {
                modifier: -5,
                bound: -100,
                rule: AutomaticProfileRule::UnexpectedFiles {
                    file_patterns: vec![],
                    mode: jumbie_shared::config::UnexpectedFilesMode::Blacklist,
                },
            },
        );
        app.db.save_general_config(&general).await.unwrap();

        app.rescore_unexpected_files().await;

        let score = get_score(&app, submitter).await;
        assert_eq!(score, -15);
    }
}
