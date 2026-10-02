mod custom;
mod extract;
mod parse;
mod variant;

pub use custom::*;
pub use extract::*;
pub use parse::*;
pub use variant::*;

crate::test_module! {
    #[test]
    fn test_extract_submitter_bracket() {
        assert_eq!(
            extract_submitter("[MockGroup] Mock Series - 01 (1080p).mkv"),
            Some("MockGroup".to_string())
        );
        assert_eq!(
            extract_submitter("[MockGroup] Show - 02 [1080p].mkv"),
            Some("MockGroup".to_string())
        );
    }

    #[test]
    fn test_extract_submitter_bracket_prefix() {
            assert_eq!(
                extract_submitter("[MockGroup]_Mock_Series_1163_[VOSTFR][HD_1280x720].mp4"),
                Some("MockGroup".to_string())
            );
        assert_eq!(
            extract_submitter("[MockGroup] Mock Series (01-28) (1080p) [Batch]"),
            Some("MockGroup".to_string())
        );
        assert_eq!(
            extract_submitter("[MockGroup] Mock Series - 32 [1080p CR WEBRip HEVC AAC][MultiSub][ABCD1234]"),
            Some("MockGroup".to_string())
        );
    }

    #[test]
    fn test_extract_submitter_bracket_suffix() {
        // Bracket suffix style: "... [tags]-Group-Name"
        assert_eq!(
            extract_submitter("Mock Series (2024) - S01E07 - Episode Title [WEBDL-1080p][AAC 2.0][x264]-MockGroup.mkv"),
            Some("MockGroup".to_string())
        );
        assert_eq!(
            extract_submitter("Show - 01 [1080p][x264]-SubGroup.mkv"),
            Some("SubGroup".to_string())
        );
    }

    #[test]
    fn test_extract_submitter_spaced_suffix() {
        // Spaced-dash suffix style: "... - Group-Name" or "... - Group-Name (trailing)"
        assert_eq!(
            extract_submitter("Mock Series (Uncensored) S01E06 VOSTFR 1080p WEB AV1 AAC - MockGroup (CRC)"),
            Some("MockGroup".to_string())
        );
        assert_eq!(
            extract_submitter("Show S01E01 - SomeGroup"),
            Some("SomeGroup".to_string())
        );
        // Trailing parenthesized metadata
        assert_eq!(
            extract_submitter("Show S01E01 - Group-Name (CRC)"),
            Some("Group-Name".to_string())
        );
        // Quality tags should NOT match the spaced-dash suffix (filtered by INVALID_GROUPS)
        assert_eq!(extract_submitter("Show S01E01 - 1080p"), None);
        assert_eq!(extract_submitter("Show S01E01 - HEVC"), None);
    }

    #[test]
    fn test_extract_submitter_invalid_quality() {
        // Quality tags should NOT be returned as group names
        assert_eq!(extract_submitter("Show S01E01 - 1080p"), None);
        assert_eq!(extract_submitter("Show S01E01 - HEVC"), None);
    }

    #[test]
    fn test_extract_submitter_season_episode_markers_blocked() {
        // Season/episode markers should NOT be returned as group names
        assert_eq!(extract_submitter("Show - S01E01"), None);
        assert_eq!(extract_submitter("Show - S01E01E02"), None);
        assert_eq!(extract_submitter("Show - S01"), None);
        assert_eq!(extract_submitter("Show - E01"), None);
        // Version markers should not be treated as group
        assert_eq!(extract_submitter("Show S01E01 - v2"), None);
        assert_eq!(extract_submitter("Show S01E01 - v3"), None);
    }

    #[test]
    fn test_extract_submitter_no_group() {
        assert_eq!(extract_submitter("ShowWithNoGroup S01E01"), None);
    }

    #[test]
    fn test_clean_title() {
        assert_eq!(clean_title("My Show [1080p]"), "My Show");
        assert_eq!(clean_title("Title (2024) [BD]"), "Title");
        assert_eq!(clean_title("Show [Group] [720p] HEVC stuff"), "Show");
        assert_eq!(clean_title("Mock Series [1080p]"), "Mock Series");
    }

    #[test]
    fn test_parse_filename_absolute_dash_range() {
        // A bracketed release with a bare dash range (` - 01-13`) is a
        // multi-episode range, not a season pack. The download queue relies on
        // this to show `S01E01-E13` instead of an unresolved label.
        let info = parse_filename(
            "[MockGroup] Whisker Mini Anime - 01-13 (1080p) (Whisker)",
            ParseContext::Search,
        )
        .expect("dash range should parse");

        assert_eq!(info.seasons, vec![1], "defaults to season 1 when absent");
        assert_eq!(info.episodes, (1..=13).collect::<Vec<i32>>());
        assert!(
            !info.is_season_pack && !info.is_complete_pack,
            "a numeric range is not a pack"
        );
    }

    #[test]
    fn test_parse_filename_standard() {
        let ctx = ParseContext::FileScan;
        let cases = vec![
            (
                "[MockGroup] Mock Series - 01 (1080p) [ABCD1234].mkv",
                "Mock Series",
                1,
            ),
            (
                "[MockGroup] Mock Series 2 - 02 [1080p].mkv",
                "Mock Series 2",
                2,
            ),
            ("Mock Series 3 - 03 [1080p].mkv", "Mock Series 3", 3),
            ("Mock Series 4 - S01E04.mkv", "Mock Series 4", 4),
        ];

        for (filename, expected_series, expected_ep) in cases {
            let res = parse_filename(filename, ctx);
            if res.is_none() {
                println!("FAILED to parse: '{}'", filename);
                panic!("Parsing failed for '{}'", filename);
            }
            let res = res.unwrap();
            assert_eq!(
                res.series_key, expected_series,
                "Failed series for {}",
                filename
            );
            assert_eq!(
                res.episodes.first().copied().unwrap_or(1), expected_ep,
                "Failed episode for {}",
                filename
            );
        }
    }

    #[test]
    fn test_parse_filename_season_packs() {
        let ctx = ParseContext::Search;
        let cases = vec![
            ("[MockRaws] Mock Frontier - S02 [1080p].mkv", "Mock Frontier", 2),
            ("Dungeon Meshi - S01.mkv", "Dungeon Meshi", 1),
            ("Frankenstein S01 [1080p].mkv", "Frankenstein", 1),
            ("[Group] Title - S03", "Title", 3),
            ("Show Name S04", "Show Name", 4),
            ("[Group] Title Season 2", "Title", 2),
            ("Test Series - Season 5", "Test Series", 5),
            ("Show Name Season 1 Complete.mkv", "Show Name", 1),
        ];

        for (filename, expected_series, expected_season) in cases {
            let res = parse_filename(filename, ctx);
            if res.is_none() {
                println!("FAILED to parse season pack: '{}'", filename);
                panic!("Parsing failed for '{}'", filename);
            }
            let res = res.unwrap();
            assert_eq!(res.series_key, expected_series, "Failed series for {}", filename);
            assert_eq!(res.seasons.first().copied(), Some(expected_season), "Failed season for {}", filename);
            assert_eq!(res.episodes.first().copied().unwrap_or(1), 1, "Failed episode_num for {}", filename);
            assert!(res.is_season_pack, "Failed is_season_pack for {}", filename);
        }
    }

    #[test]
    fn test_parse_filename_season_strict_requires_declared_season() {
        let ctx = ParseContext::Search;

        // No season declared → the `S01` fallback is disabled, so the title is
        // rejected rather than matched as season 1.
        assert!(parse_filename_for_season("Show - 05 [1080p]", ctx, 1).is_none());
        assert!(parse_filename_for_season("Show E05 [1080p]", ctx, 1).is_none());
        assert!(parse_filename_for_season("Show - 01-13 [1080p]", ctx, 1).is_none());
        assert!(parse_filename_for_season("Show Episodes 1, 2, 3", ctx, 1).is_none());

        // The lenient parse still falls back to season 1 (RSS / file scan).
        let lenient = parse_filename("Show - 05 [1080p]", ctx).expect("lenient parse");
        assert_eq!(lenient.seasons, vec![1]);
    }

    #[test]
    fn test_parse_filename_season_strict_matches_declared_season() {
        let ctx = ParseContext::Search;

        // The result is season 1 and season 1 was searched for → accepted.
        let info = parse_filename_for_season("Show S01E05 [1080p]", ctx, 1)
            .expect("matching season should parse");
        assert_eq!(info.seasons, vec![1]);
        assert_eq!(info.episodes, vec![5]);

        // A different season is unrelated to the search → rejected.
        assert!(parse_filename_for_season("Show S02E05 [1080p]", ctx, 1).is_none());
        assert!(parse_filename_for_season("Show S05E05 [1080p]", ctx, 1).is_none());
    }

    #[test]
    fn test_parse_filename_season_strict_packs_and_ranges() {
        let ctx = ParseContext::Search;

        // A season pack declaring the searched season is accepted.
        let pack = parse_filename_for_season("Show S02 COMPLETE [1080p]", ctx, 2)
            .expect("matching season pack");
        assert!(pack.is_season_pack);
        assert_eq!(pack.seasons, vec![2]);

        // A pack of another season, or a complete-series pack with no season, is
        // not usable for a season-scoped search.
        assert!(parse_filename_for_season("Show S03 COMPLETE", ctx, 2).is_none());
        assert!(parse_filename_for_season("Show COMPLETE", ctx, 2).is_none());

        // A multi-season range is accepted only when it covers the searched season.
        let range = parse_filename_for_season("Show S01-S03 COMPLETE", ctx, 2)
            .expect("range covering the season");
        assert_eq!(range.seasons, vec![1, 2, 3]);
        assert!(parse_filename_for_season("Show S01-S03 COMPLETE", ctx, 5).is_none());
    }

    #[test]
    fn test_parse_filename_season_episode_words() {
        // Spelled-out "Season N Episode M" is a season-aware pair, not a
        // season-less "Episode M".
        let cases = [
            ("Test Series Season 3 Episode 4 1080p", ParseContext::Search, 3, 4),
            ("Test Series Season 3, Episode 4 [1080p]", ParseContext::Search, 3, 4),
            ("Test Series Season 3 - Episode 4", ParseContext::Search, 3, 4),
            ("[Group] Test Series Season 12 Episode 7", ParseContext::Search, 12, 7),
            ("Test Series 2nd Season - Episode 4", ParseContext::Search, 2, 4),
            ("Test Series 1st Season Episode 1", ParseContext::Search, 1, 1),
            ("Test Series 3rd Season, Episode 4", ParseContext::Search, 3, 4),
            ("[Group] Test Series 22nd Season - Episode 7", ParseContext::Search, 22, 7),
            ("Test Series Season 3 Episode 4.mkv", ParseContext::FileScan, 3, 4),
            ("Test Series 2nd Season - Episode 4.mkv", ParseContext::FileScan, 2, 4),
        ];
        for (filename, ctx, season, episode) in cases {
            let res = parse_filename(filename, ctx)
                .unwrap_or_else(|| panic!("failed to parse '{filename}'"));
            assert_eq!(res.series_key, "Test Series", "series for '{filename}'");
            assert_eq!(res.seasons, vec![season], "season for '{filename}'");
            assert_eq!(res.episodes, vec![episode], "episode for '{filename}'");
            assert!(!res.is_season_pack, "pack for '{filename}'");
        }

        // The strict season parse still applies to the pair.
        assert!(
            parse_filename_for_season("Test Series Season 3 Episode 4", ParseContext::Search, 3)
                .is_some()
        );
        assert!(
            parse_filename_for_season("Test Series Season 3 Episode 4", ParseContext::Search, 4)
                .is_none()
        );
        assert!(
            parse_filename_for_season("Test Series 2nd Season - Episode 4", ParseContext::Search, 2)
                .is_some()
        );
        assert!(
            parse_filename_for_season("Test Series 2nd Season - Episode 4", ParseContext::Search, 3)
                .is_none()
        );

        // Regression: "Season N" alone (no Episode) is still a season pack.
        let pack = parse_filename("Test Series Season 3 Complete", ParseContext::Search)
            .expect("season pack");
        assert!(pack.is_season_pack);
        assert_eq!(pack.seasons, vec![3]);
    }

    #[test]
    fn test_parse_filename_part_and_year_are_not_seasons() {
        // Regression guard for the season-aware patterns (spelled-out
        // "Season N Episode M" and ordinal "Nth Season"): a part marker or a
        // year must never be read as a season.

        // Search context does not match these at all.
        assert!(parse_filename("Series Part 4", ParseContext::Search).is_none());
        assert!(parse_filename("Series Part 4 1080p", ParseContext::Search).is_none());
        assert!(parse_filename("Series Pt 4", ParseContext::Search).is_none());
        assert!(parse_filename("Series CD 2", ParseContext::Search).is_none());
        assert!(parse_filename("Series Disc 3", ParseContext::Search).is_none());

        // FileScan: the marker is a part number and the release stays season 1.
        for (name, part) in [
            ("Series Part 4", 4),
            ("Series Pt 4", 4),
            ("Series CD 2", 2),
            ("Series Disc 3", 3),
        ] {
            let info = parse_filename(name, ParseContext::FileScan)
                .unwrap_or_else(|| panic!("failed to parse {name}"));
            assert_eq!(info.seasons, vec![1], "{name}: part marker must not be a season");
            assert_eq!(info.part_number, Some(part), "{name}: marker is a part");
        }

        // A year is never a season.
        let year = parse_filename("Show (2024) Episode 4", ParseContext::Search)
            .expect("year title should parse");
        assert_eq!(year.seasons, vec![1]);
        assert_eq!(year.episodes, vec![4]);

        // "2nd Part" is not the ordinal-season form (that requires "Season").
        let second_part = parse_filename("Show 2nd Part - Episode 4", ParseContext::Search)
            .expect("2nd Part title should parse");
        assert_eq!(second_part.seasons, vec![1], "2nd Part is not 2nd Season");
        assert_eq!(second_part.episodes, vec![4]);

        // A real "Season N" with a part marker is still season N (pack).
        let real = parse_filename("Show Season 3 Part 2", ParseContext::Search)
            .expect("Season 3 Part 2 should parse");
        assert_eq!(real.seasons, vec![3]);
        assert!(real.is_season_pack);
        assert_eq!(real.part_number, Some(2));

        // The strict season parse must not accept a part marker as a season.
        assert!(parse_filename_for_season("Series Part 4", ParseContext::Search, 4).is_none());
        assert!(parse_filename_for_season("Series CD 2", ParseContext::Search, 2).is_none());
    }

    #[test]
    fn test_parse_filename_season_tricky_false_positives() {
        let ctx = ParseContext::Search;

        // A year before "Season" is part of the title, not the season.
        for name in [
            "Show 2019 Season 3 Episode 4",
            "[Group] Show 2019 Season 3 Episode 4",
        ] {
            let info = parse_filename(name, ctx).unwrap_or_else(|| panic!("parse {name}"));
            assert_eq!(info.seasons, vec![3], "{name}: 2019 must not be the season");
            assert_eq!(info.episodes, vec![4], "{name}");
        }

        // A year used as an episode number is an episode, never a season.
        let ep_year = parse_filename("Show Episode 2024", ctx).expect("parse");
        assert_eq!(ep_year.seasons, vec![1]);
        assert_eq!(ep_year.episodes, vec![2024]);

        // Ordinals without "Season" are not seasons.
        assert!(parse_filename("Show 2nd", ctx).is_none());
        assert!(parse_filename("Show 3rd Anniversary", ctx).is_none());
        assert!(parse_filename("Show 2nd", ParseContext::FileScan).is_none());

        // Spelled-out "Season" markers without a numeric season are not matched.
        assert!(parse_filename("Show Season Finale", ctx).is_none());
        assert!(parse_filename("Show Season One Episode Two", ctx).is_none());

        // Dashes around the markers don't change the parse.
        let dashed = parse_filename("Show - Season 3 - Episode 4", ctx).expect("parse");
        assert_eq!(dashed.seasons, vec![3]);
        assert_eq!(dashed.episodes, vec![4]);

        // A bare 4-digit year is neither a season nor an episode (lone-number
        // fallback caps at 3 digits).
        assert!(parse_filename("Show 2019", ParseContext::FileScan).is_none());
    }

    #[test]
    fn test_season_range_patterns_direct_verify() {
        // Verify the SEASON_RANGE_PATTERNS static is accessible and correct
        // Current: S01-S02 (0), Season N-M (1), Seasons N-M (2),
        //   S01, S02 (3), S01 + S02 (4), S01 S02 S03 (5),
        //   Season/Seasons 1, 2, 3 (6), S01-04 implicit (7)
        let patterns = &crate::patterns::SEASON_RANGE_PATTERNS;
        assert_eq!(patterns.len(), 8, "Should have 8 season-range patterns");

        let test = "Test Series Season 1-2 Complete [1080p]";
        let re = &patterns[1];
        let caps = re.captures(test)
            .unwrap_or_else(|| panic!("Pattern 1 should match {}", test));
        assert_eq!(caps.get(1).map(|m| m.as_str()), Some("Test Series"));
        assert_eq!(caps.get(2).map(|m| m.as_str()), Some("1"));
        assert_eq!(caps.get(3).map(|m| m.as_str()), Some("2"));

        let test2 = "Show Name Seasons 1-2";
        let re2 = &patterns[2];
        let caps2 = re2.captures(test2)
            .unwrap_or_else(|| panic!("Pattern 2 should match {}", test2));
        assert_eq!(caps2.get(1).map(|m| m.as_str()), Some("Show Name"));
        assert_eq!(caps2.get(2).map(|m| m.as_str()), Some("1"));
        assert_eq!(caps2.get(3).map(|m| m.as_str()), Some("2"));

        // Verify comma pattern
        let caps3 = patterns[3].captures("Test S01, S02, S03")
            .expect("Comma pattern should match");
        assert_eq!(caps3.get(2).map(|m| m.as_str()), Some("01"));
        assert_eq!(caps3.get(3).map(|m| m.as_str()), Some("02"));
        assert_eq!(caps3.get(4).map(|m| m.as_str()), Some(", S03"));

        // Verify plus pattern
        let caps4 = patterns[4].captures("Show S01 + S02")
            .expect("Plus pattern should match");
        assert_eq!(caps4.get(2).map(|m| m.as_str()), Some("01"));
        assert_eq!(caps4.get(3).map(|m| m.as_str()), Some("02"));
    }

    #[test]
    fn test_parse_filename_season_pack_spelled_out() {
        // "Season N" spelled out (common in P2P/web-dl naming)
        // This must correctly extract season number, not bake it into the title.
        let ctx = ParseContext::Search;
        let filename = "Test Series Season 5 Complete 720p BluRay x264 [Group]";
        let res = parse_filename(filename, ctx).expect("Failed to parse season pack with spelled-out season");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(5));
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 1);
        assert!(res.is_season_pack);
        assert!(res.is_complete_pack);
    }

    #[test]
    fn test_parse_filename_season_range_s01_s02() {
        // Scene-style season range: S01-S02
        let filename = "Test Series S01-S02 Complete 1080p Web-DL x264";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse S01-S02 season range");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(2));
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 1);
        assert!(res.is_season_pack);
        assert!(res.is_complete_pack);

        // Spaced dash variant: S01 - S02
        let spaced = "Show Name S01 - S02 [1080p]";
        let res2 = parse_filename(spaced, ParseContext::Search)
            .expect("Failed to parse S01 - S02 season range");
        assert_eq!(res2.series_key, "Show Name");
        assert_eq!(res2.seasons.first().copied(), Some(1));
        assert_eq!(res2.seasons.last().copied(), Some(2));
        assert!(res2.is_season_pack);
        assert!(!res2.is_complete_pack);
    }

    #[test]
    fn test_parse_filename_season_range_spelled_out() {
        // Spelled-out season range: Season 1-2
        let filename = "Test Series Season 1-2 Complete [1080p]";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse Season 1-2 range");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(2));
        assert!(res.is_season_pack);
        assert!(res.is_complete_pack);

        // With spaced dash: Season 1 - 2
        let spaced = "Show Name Season 1 - 2 720p";
        let res2 = parse_filename(spaced, ParseContext::Search)
            .expect("Failed to parse Season 1 - 2 range");
        assert_eq!(res2.series_key, "Show Name");
        assert_eq!(res2.seasons.first().copied(), Some(1));
        assert_eq!(res2.seasons.last().copied(), Some(2));
        assert!(res2.is_season_pack);
        assert!(!res2.is_complete_pack);
    }

    #[test]
    fn test_parse_filename_season_range_plural() {
        // Plural: Seasons 1-2
        let filename = "Test Series Seasons 1-2 [1080p]";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse Seasons 1-2 range");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(2));
        assert!(res.is_season_pack);
        assert!(!res.is_complete_pack);
    }

    #[test]
    fn test_parse_filename_comma_season_range() {
        // Comma-separated season range: S01, S02
        let filename = "Test Series S01, S02 Complete 1080p";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse S01, S02 season range");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(2));
        assert!(res.is_season_pack);
        assert!(res.is_complete_pack);

        // Three seasons: S01, S02, S03
        let three = "Show Name S01, S02, S03 [1080p]";
        let res2 = parse_filename(three, ParseContext::Search)
            .expect("Failed to parse S01, S02, S03");
        assert_eq!(res2.series_key, "Show Name");
        assert_eq!(res2.seasons.first().copied(), Some(1));
        assert_eq!(res2.seasons.last().copied(), Some(3));
        assert_eq!(res2.seasons, vec![1, 2, 3], "Should capture all three seasons");
    }

    #[test]
    fn test_parse_filename_plus_season_range() {
        // Plus-separated season range: S01 + S02
        let filename = "Test Series S01 + S02 Complete";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse S01 + S02");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(2));
        assert!(res.is_season_pack);
        assert!(res.is_complete_pack);
    }

    #[test]
    fn test_parse_filename_space_season_range() {
        // Space-separated seasons: S01 S02 S03
        let filename = "Test Series S01 S02 S03 [1080p]";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse S01 S02 S03");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(3));
        assert_eq!(res.seasons, vec![1, 2, 3]);
        assert!(res.is_season_pack);
    }

    #[test]
    fn test_parse_filename_spelled_out_season_comma() {
        // Season 1, 2, 3 — spelled-out with comma list
        let filename = "Test Series Season 1, 2, 3 [1080p]";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse Season 1, 2, 3");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(3));
        assert_eq!(res.seasons, vec![1, 2, 3]);
        assert!(res.is_season_pack);

        // Seasons 1, 2, 3 — plural variant
        let plural = "Show Seasons 1, 2, 3 [720p]";
        let res2 = parse_filename(plural, ParseContext::Search)
            .expect("Failed to parse Seasons 1, 2, 3");
        assert_eq!(res2.series_key, "Show");
        assert_eq!(res2.seasons, vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_filename_spelled_out_episodes_comma() {
        // Episodes 1, 2, 3 — spelled-out episode list (absolute numbered)
        let filename = "Test Series Episodes 1, 2, 3 [1080p]";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse Episodes 1, 2, 3");
        assert_eq!(res.series_key, "Test Series");
        // Absolute-numbered episodes default to season 1
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 1);
        assert_eq!(res.episodes, vec![1, 2, 3]);
        assert!(!res.is_season_pack);

        // Episode 1, 2, 3
        let singular = "Show Episode 1, 2, 3";
        let res2 = parse_filename(singular, ParseContext::Search)
            .expect("Failed to parse Episode 1, 2, 3");
        assert_eq!(res2.series_key, "Show");
        assert_eq!(res2.episodes, vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_filename_comma_episode_list() {
        // S01E02, E03, E04 — comma-separated episode list with season
        let filename = "Test Series - S01E02, E03, E04";
        let res = parse_filename(filename, ParseContext::Search)
            .expect("Failed to parse comma-separated episodes");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 2);
        assert_eq!(res.episodes, vec![2, 3, 4]);
        assert!(!res.is_season_pack);
    }

    #[test]
    fn test_parse_filename_season_pack_comma() {
        // Comma + Season pattern: "Show Name, Season 1 Complete"
        let ctx = ParseContext::Search;
        let cases = vec![
            ("Show Name, Season 1 Complete [1080p]", "Show Name", 1, true),
            ("Star Trek, Season 3 [1080p].mkv", "Star Trek", 3, false),
        ];
        for (filename, expected_series, expected_season, expect_complete) in cases {
            let res = parse_filename(filename, ctx)
                .expect("Failed to parse comma+Season pack");
            assert_eq!(res.series_key, expected_series);
            assert_eq!(res.seasons.first().copied(), Some(expected_season));
            assert_eq!(res.episodes.first().copied().unwrap_or(1), 1);
            assert!(res.is_season_pack);
            assert_eq!(res.is_complete_pack, expect_complete);
        }
    }

    #[test]
    fn test_parse_filename_season_pack_descriptor_strip() {
        // CLEAN_TRAILING_DESCRIPTORS strips " - Complete ..." from series_key.
        // This must NOT strip standalone "Complete" that's part of the series name.
        let ctx = ParseContext::Search;

        // Strip: " - Complete ANIMATED TV Series, Season 1-2" suffix
        let filename = "THE WAR OF THE WORLDS - Complete ANIMATED TV Series, Season 1-2 S01-S02 - 1080p Web-DL x264";
        let res = parse_filename(filename, ctx)
            .expect("Failed to parse resistance-like filename");
        assert_eq!(
            res.series_key, "THE WAR OF THE WORLDS",
            "Should strip ' - Complete ...' from series_key"
        );
        assert_eq!(res.seasons.first().copied(), Some(1));
        assert_eq!(res.seasons.last().copied(), Some(2));
        assert!(res.is_season_pack);
        assert!(res.is_complete_pack);

        // NOT strip: standalone "Complete" in series name (no ` - ` before it)
        let keep = "The Complete Series S01 [1080p]";
        let res2 = parse_filename(keep, ctx)
            .expect("Failed to parse series with 'Complete' in name");
        assert_eq!(
            res2.series_key, "The Complete Series",
            "Should NOT strip standalone 'Complete' in series name"
        );
        assert_eq!(res2.seasons.first().copied(), Some(1));
    }

    #[test]
    fn test_parse_filename_season_range_invalid() {
        // Season ranges with s2 <= s1 are invalid — must not produce season_end
        let ctx = ParseContext::Search;

        // Reverse range: S02-S01 (s2 < s1) — should not match as season range
        let rev = "Test Series S02-S01 [1080p]";
        let res = parse_filename(rev, ctx);
        match res {
            None => { /* fell through gracefully — fine */ }
            Some(info) => {
                // If it matched via S02 single-season, season_end must be None
                assert!(
                    info.seasons.len() <= 1,
                    "Reverse range S02-S01 should not produce season_end"
                );
            }
        }

        // Same season: S01-S01 — invalid range
        let same = "Test Series S01-S01 [1080p]";
        let res2 = parse_filename(same, ctx);
        match res2 {
            None => {}
            Some(info) => {
                assert!(
                    info.seasons.len() <= 1,
                    "S01-S01 should not produce season_end"
                );
            }
        }
    }

    #[test]
    fn test_parse_filename_season_range_filescan_skipped() {
        // Season-range patterns are search-only — FileScan must skip them.
        let ctx = ParseContext::FileScan;
        let filename = "Test Series S01-S02 Complete 1080p";
        let res = parse_filename(filename, ctx);
        assert!(
            res.is_none(),
            "Season-range patterns must be skipped in FileScan context"
        );
    }

    #[test]
    fn test_parse_filename_exact_user_filenames() {
        // The two filenames the user asked about
        let ctx = ParseContext::Search;

        // Filename 1: Multi-season pack with descriptive noise
        let f1 = "THE WAR OF THE WORLDS (2018-2020) - Complete ANIMATED TV Series, Season 1-2 S01-S02 - 1080p Web-DL x264";
        let r1 = parse_filename(f1, ctx).expect("Failed on user filename 1");
        assert_eq!(r1.series_key, "THE WAR OF THE WORLDS");
        assert_eq!(r1.seasons.first().copied(), Some(1));
        assert_eq!(r1.seasons.last().copied(), Some(2));
        assert!(r1.is_season_pack);
        assert!(r1.is_complete_pack);
        assert_eq!(r1.episodes.first().copied().unwrap_or(1), 1);

        // Filename 2: Dot-notation with scene-style season pack
        // Note: The year "2010" after S3 matches SINGLE_PATTERN index 7
        // (S(\\d+) \\d+) as episode=2010, so this is NOT a season pack.
        // This is pre-existing behavior — the year after S3 is ambiguous.
        let f2 = "Star.Wars-Clone.Wars.S3.2010-2011.1080p.NF.WebDL.AVC.DD.5.1-DTOne";
        let r2 = parse_filename(f2, ctx).expect("Failed on user filename 2");
        assert_eq!(r2.seasons.first().copied(), Some(3));
        assert_eq!(r2.episodes.first().copied().unwrap_or(1), 2010, "Year after S3 parsed as episode number");
    }

    #[test]
    fn test_parse_filename_ranges() {
        let res = parse_filename("[Batch] Test Series - 100-102 [1080p]", ParseContext::FileScan).unwrap();
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 100);
        assert_eq!(res.episodes.last().copied(), Some(102));
        assert_eq!(res.part_number, None);
    }

    #[test]
    fn test_parse_filename_concatenated_episode_range() {
        // "S01E01E02" has no separator, so no separator-based range pattern matches it;
        // it must still cover both episodes (checked in the file-scan context).
        for name in [
            "Test.Show.S01E01E02.mkv",
            "Test Show - S01E01E02.mkv",
            "[Group] Test Show - S01E01E02 1080p.mkv",
            "Test Show S01E01E02E03.mkv",
        ] {
            let res = parse_filename(name, ParseContext::FileScan).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(res.seasons, vec![1], "{name}");
            let expected: Vec<i32> = if name.contains("E03") {
                vec![1, 2, 3]
            } else {
                vec![1, 2]
            };
            assert_eq!(res.episodes, expected, "{name}");
            assert!(!res.is_season_pack, "{name}");
        }
        // A single episode is not a concatenated range.
        let single = parse_filename("Test.Show.S01E01.mkv", ParseContext::FileScan).unwrap();
        assert_eq!(single.episodes, vec![1]);
    }

    #[test]
    fn test_parse_filename_spaced_dash_not_a_range() {
        // Regression: "S22E38 - 1123" (with spaces around the dash) should NOT
        // be parsed as a range E38-E1123. The 1123 is the absolute episode number
        // for Test Series, not an episode range end.
        let filename = "Test Series (1999) - S22E38 - 1123 - Mock Episode Title [HDTV-1080p][8bit][x264][AAC 2.0][JA]-MockFansub.mkv";
        let res = parse_filename(filename, ParseContext::FileScan).expect("Failed to parse Test Series filename");
        assert_eq!(res.series_key, "Test Series");
        assert_eq!(res.seasons.first().copied(), Some(22));
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 38);
        assert!(res.episodes.len() <= 1, "Spaced dash after E38 should NOT be treated as a range");
        assert!(!res.is_season_pack);

        // Verify that legitimate ranges WITHOUT spaces still work (no-E-prefix)
        let legit = parse_filename("Show - S01E01-02.mkv", ParseContext::FileScan).expect("Failed to parse legitimate range");
        assert_eq!(legit.episodes.first().copied().unwrap_or(1), 1);
        assert_eq!(legit.episodes.last().copied(), Some(2));

        // Verify that ranges WITH spaces but WITH E-prefix still work
        let legit_e = parse_filename("Show - S01E01 - E02.mkv", ParseContext::FileScan).expect("Failed to parse E-prefix range");
        assert_eq!(legit_e.episodes.first().copied().unwrap_or(1), 1);
        assert_eq!(legit_e.episodes.last().copied(), Some(2));
    }

    #[test]
    fn test_parse_filename_multi_episode_e_prefix() {
        // S02E08-E12 style (E prefix on end episode)
        let cases = vec![
            ("Test - S02E08-E12.mkv", 2, 8, 12),
            ("Show Name S01E03-E05 [1080p].mkv", 1, 3, 5),
            ("[Group] Series - S03E01-E03 [720p].mkv", 3, 1, 3),
        ];
        for (filename, season, ep_start, ep_end) in cases {
            let res = parse_filename(filename, ParseContext::FileScan)
                .unwrap_or_else(|| panic!("Failed to parse: {}", filename));
            assert_eq!(res.seasons.first().copied(), Some(season), "season mismatch for {}", filename);
            assert_eq!(res.episodes.first().copied().unwrap_or(1), ep_start, "ep_start mismatch for {}", filename);
            assert_eq!(res.episodes.last().copied(), Some(ep_end), "ep_end mismatch for {}", filename);
            assert_eq!(res.part_number, None);
        }
    }

    #[test]
    fn test_parse_filename_multi_part() {
        // Numeric part numbers
        let cases: Vec<(&str, u32)> = vec![
            ("Series Name A (2025) S01E01-part-1.mkv", 1),
            ("Series Name A (2025) S01E01-part-2.mkv", 2),
            // cd suffix on a series file
            ("Show S01E01-cd1.mkv", 1),
            ("Show S01E01-cd2.mkv", 2),
            ("Show S01E01-pt.1.mkv", 1),
            ("Show S01E01_disc_2.mkv", 2),
            ("Show S01E01 disk 3.mkv", 3),
            ("Show S01E01-dvd1.mkv", 1),
        ];
        for (filename, expected_part) in cases {
            let res = parse_filename(filename, ParseContext::FileScan)
                .unwrap_or_else(|| panic!("Failed to parse: {}", filename));
            assert_eq!(res.part_number, Some(expected_part));
        }

        // Letter part: a=1, b=2, c=3, d=4
        let letter_cases: Vec<(&str, u32)> = vec![
            ("Show S01E01-part.a.mkv", 1),
            ("Show S01E01-part.b.mkv", 2),
            ("Show S01E01-part.c.mkv", 3),
            ("Show S01E01-part.d.mkv", 4),
        ];
        for (filename, expected_part) in letter_cases {
            let res = parse_filename(filename, ParseContext::FileScan)
                .unwrap_or_else(|| panic!("Failed to parse: {}", filename));
            assert_eq!(
                res.part_number,
                Some(expected_part),
                "letter part mismatch for {}",
                filename
            );
        }

        // No part suffix — should be None
        let no_part_cases = vec![
            "[MockFansub] Dracula - 01 (1080p).mkv",
            "Test - S02E08-E12.mkv",
            "Frankenstein - S01E04.mkv",
        ];
        for filename in no_part_cases {
            let res = parse_filename(filename, ParseContext::FileScan)
                .unwrap_or_else(|| panic!("Failed to parse: {}", filename));
            assert_eq!(
                res.part_number, None,
                "Expected no part_number for {}",
                filename
            );
        }
    }

    #[test]
    fn test_parse_filename_dot_notation() {
        let res = parse_filename("[Group].Some.Series.Name.S03E01.1080p.mkv", ParseContext::FileScan).unwrap();
        assert_eq!(res.series_key, "Some Series Name");
        assert_eq!(res.seasons.first().copied(), Some(3));
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 1);
        assert_eq!(res.file_ext, "mkv");
    }

    #[test]
    fn test_parse_filename_unicode_dashes() {
        // Dash characters like – (en-dash), — (em-dash), − (minus) should be treated like normal -
        let cases = vec![
            ("Show – 05", 5),
            ("Show — 06.mkv", 6),
            ("Show − 07", 7),
        ];

        for (filename, ep) in cases {
            let res = parse_filename(filename, ParseContext::FileScan).unwrap();
            assert_eq!(res.series_key, "Show");
            assert_eq!(res.episodes.first().copied().unwrap_or(1), ep);
        }
    }

    #[test]
    fn test_parse_filename_missing_extension() {
        // If there's no extension, it should fallback to "mkv"
        let res = parse_filename("[Group] Show S01E01", ParseContext::FileScan).unwrap();
        assert_eq!(res.file_ext, "mkv", "Missing extension should default to mkv");
    }

    #[test]
    fn test_extract_version() {
        assert_eq!(extract_version("[Group] Show S01E01 v2.mkv"), 2);
        assert_eq!(extract_version("[Group] Show - 01 v3 [1080p].mkv"), 3);
        assert_eq!(extract_version("Show S02E05.mkv"), 1, "No version specified should default to 1");
        assert_eq!(extract_version("Show - 01 (v4).mkv"), 4);
        // Underscore and dash delimiters
        assert_eq!(extract_version("Show_S01E01_v2.mkv"), 2, "Underscore prefix");
        assert_eq!(extract_version("Show-S01E01-v2.mkv"), 2, "Dash prefix");
        assert_eq!(extract_version("v2_Show_S01E01.mkv"), 2, "Leading v2");
        // Extended formats: v., ver, ver., version, version.
        assert_eq!(extract_version("Show - 01 v.2 [1080p].mkv"), 2, "v. prefix");
        assert_eq!(extract_version("Show - 01 ver2.mkv"), 2, "ver prefix");
        assert_eq!(extract_version("Show - 01 ver.2.mkv"), 2, "ver. prefix");
        assert_eq!(extract_version("Show - 01 version2.mkv"), 2, "version prefix");
        assert_eq!(extract_version("Show - 01 version.2.mkv"), 2, "version. prefix");
    }

    #[test]
    fn test_parse_filename_absolute() {
        let cases = vec![
            (
                "[MockRaws] Mock Frontier - 12 [1080p].mkv",
                "Mock Frontier",
                12,
            ),
            ("Dungeon Meshi - 23.mkv", "Dungeon Meshi", 23),
            ("Test Series Episode 1000", "Test Series", 1000),
            ("[Group] Title - 02v2.mkv", "Title", 2),
        ];

        for (filename, expected_series, expected_ep) in cases {
            let res = parse_filename(filename, ParseContext::FileScan).unwrap_or_else(|| panic!("Failed parsing: {}", filename));
            assert_eq!(res.series_key, expected_series);
            assert_eq!(res.episodes.first().copied().unwrap_or(1), expected_ep);
            // Absolute-numbered patterns default to season 1
            assert_eq!(res.seasons.first().copied(), Some(1));
        }
    }

    // ── parse_title_with_custom_regex tests ────────────────────────────

    #[test]
    fn test_custom_regex_episode_only_group() {
        // Pattern with episode group only, standard mode with current_season
        let patterns = vec!["Episode\\s*(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "My Show - Episode 5",
            &patterns,
            Some(2),  // current_season = 2
            false,     // standard mode
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
                assert_eq!(info.seasons.first().copied(), Some(2), "Should inherit current_season");
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_episode_only_defaults_to_season_1() {
        // Pattern with episode group only, no current_season provided
        let patterns = vec!["Ep(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show Ep10",
            &patterns,
            None,  // no current season
            false, // standard mode
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 10);
                assert_eq!(info.seasons.first().copied(), Some(1), "Should default to season 1");
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_season_and_episode_groups() {
        let patterns = vec!["S(?P<season>\\d+)EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "My Show - S03EP07",
            &patterns,
            None,  // current_season (ignored since season group is present)
            false, // standard mode
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(3));
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 7);
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_season_group_ignored_in_absolute_mode() {
        // In absolute mode, the season group should be ignored (season = None)
        let patterns = vec!["S(?P<season>\\d+)EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "My Show - S03EP07",
            &patterns,
            None,
            true, // absolute mode — season group is ignored
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert!(info.seasons.is_empty(), "Season group should be ignored in absolute mode");
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 7);
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_absolute_mode_no_season_group() {
        // Absolute mode with episode-only pattern
        let patterns = vec!["EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show EP126",
            &patterns,
            None,
            true, // absolute mode
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert!(info.seasons.is_empty(), "Absolute mode: season should be None");
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 126);
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_no_named_groups_returns_matched_filter() {
        // Pattern matches but has no named groups — should return MatchedFilter
        let patterns = vec!["1080p".to_string()];
        let result = parse_title_with_custom_regex(
            "My Show - S01E01 - 1080p",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::MatchedFilter),
            "Expected MatchedFilter, got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_no_match_returns_no_match() {
        // Pattern doesn't match title at all
        let patterns = vec!["\\[MockFansub\\]".to_string()];
        let result = parse_title_with_custom_regex(
            "OtherGroup - My Show - S01E01",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch, got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_multiple_patterns_uses_first_match() {
        let patterns = vec![
            "\\[GroupA\\].*EP(?P<episode>\\d+)".to_string(),
            "\\[GroupB\\].*EP(?P<episode>\\d+)".to_string(),
        ];
        // Should match the first pattern
        let result = parse_title_with_custom_regex(
            "[GroupA] Show EP42 [1080p]",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 42);
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }

        // Should also match the second pattern (first doesn't match)
        let result2 = parse_title_with_custom_regex(
            "[GroupB] Show EP99",
            &patterns,
            None,
            false,
            None,
        );
        match result2 {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 99);
            }
            _ => panic!("Expected Extracted, got {:?}", result2),
        }
    }

    #[test]
    fn test_custom_regex_mixed_extraction_and_filter_patterns() {
        // First pattern is filter-only (no groups), second is extraction
        let patterns = vec![
            "1080p".to_string(),
            "EP(?P<episode>\\d+)".to_string(),
        ];
        // The first pattern matches (filter) BEFORE reaching the extraction pattern.
        // Since the first pattern matched and has no named groups, it returns MatchedFilter.
        let result = parse_title_with_custom_regex(
            "Show EP07 1080p",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::MatchedFilter),
            "Expected MatchedFilter (first match is filter-only), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_source_scoped_pattern_matches() {
        let patterns = vec!["@nyaa:EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show EP07",
            &patterns,
            None,
            false,
            Some("nyaa"),
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 7);
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_source_scoped_pattern_skipped_when_source_mismatch() {
        let patterns = vec!["@nyaa:EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show EP07",
            &patterns,
            None,
            false,
            Some("tokyotosho"),  // different source
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch (source mismatch), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_source_scoped_pattern_no_source_context() {
        // Source-scoped pattern with no source context provided
        let patterns = vec!["@nyaa:EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show EP07",
            &patterns,
            None,
            false,
            None,  // no source context
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch (no source context for scoped pattern), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_generic_and_source_scoped_mixed() {
        // Mix of generic and source-scoped patterns
        let patterns = vec![
            "@nyaa:EP(?P<episode>\\d+)".to_string(),
            "(?P<episode>\\d+)v2".to_string(),
        ];
        // First pattern is source-scoped for nyaa, doesn't match for tokyotosho
        // Second is generic and should match
        let result = parse_title_with_custom_regex(
            "Show 42v2",
            &patterns,
            None,
            false,
            Some("tokyotosho"),
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 42);
            }
            _ => panic!("Expected Extracted from generic pattern, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_empty_patterns_falls_through() {
        let result = parse_title_with_custom_regex(
            "Any Show S01E01",
            &[],
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch for empty patterns, got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_invalid_pattern_skipped() {
        let patterns = vec![
            "*invalid[regex".to_string(),
            "EP(?P<episode>\\d+)".to_string(),
        ];
        let result = parse_title_with_custom_regex(
            "Show EP05",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
            }
            _ => panic!("Expected Extracted (should skip invalid regex), got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_release_title_cleaning() {
        // Ensure clean_title strips bracket tags from series_key
        let patterns = vec!["EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "My Show [SomeTag] EP03",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 3);
                assert!(
                    !info.series_key.contains("[SomeTag]"),
                    "series_key should not contain bracket tags, got: {}",
                    info.series_key
                );
            }
            _ => panic!("Expected Extracted, got {:?}", result),
        }
    }

    #[test]
    fn test_custom_regex_regex_matches_but_captured_text_not_numeric() {
        // Pattern matches but episode group captures non-numeric text.
        // The episode group content must be parseable as i32.
        let patterns = vec!["Episode_(?P<episode>[a-z]+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show_Episode_abc",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch (non-numeric capture), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_regex_matches_but_season_not_numeric() {
        // Pattern has both season + episode groups, but season captures non-numeric.
        let patterns = vec!["S(?P<season>[a-z]+)EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show_SabEP07",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch (non-numeric season capture), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_non_numeric_falls_through_to_next_pattern() {
        // First pattern matches but captures non-numeric, second pattern should be tried.
        let patterns = vec![
            "Episode_(?P<episode>[a-z]+)".to_string(),
            "Ep(?P<episode>\\d+)".to_string(),
        ];
        let result = parse_title_with_custom_regex(
            "Show_Episode_abc",
            &patterns,
            None,
            false,
            None,
        );
        // No pattern matches "Show_Episode_abc" — first matches but non-numeric,
        // second doesn't match the title format.
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch, got {:?}",
            result
        );

        // But "Show_Ep42" should be caught by the second numeric pattern
        let result2 = parse_title_with_custom_regex(
            "Show_Ep42",
            &patterns,
            None,
            false,
            None,
        );
        match result2 {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 42);
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_loose_pattern_matches_season_pack() {
        // A loose pattern like `(?P<episode>\d+)` can accidentally match
        // a season pack title "Show - S01 [1080p]", capturing episode=1.
        // The result will have is_season_pack=false — the caller must handle
        // this via the MatchedFilter or NoMatch fallback paths.
        let patterns = vec!["(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show - S01 [1080p]",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 1, "Loose pattern captures season number as episode");
                assert!(!info.is_season_pack, "Extracted result is never a season pack");
                assert!(!info.is_complete_pack);
                // Season is inherited (no season group in pattern)
                assert_eq!(info.seasons.first().copied(), Some(1));
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_season_pack_uses_fallback_when_no_named_groups() {
        // A season pack "Show - S01 [1080p]" with filter-only pattern "S\\d+".
        // The pattern has NO named groups at all → MatchedFilter.
        let patterns = vec!["S\\d+".to_string()];
        let result = parse_title_with_custom_regex(
            "Show - S01 [1080p]",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::MatchedFilter),
            "Expected MatchedFilter, got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_season_only_group_detects_season_pack() {
        // Pattern with `season` group only — treats the release as a season pack.
        // This handles non-standard naming like "Show - Season 1 Complete".
        let patterns = vec!["Season (?P<season>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show - Season 2 Complete [1080p]",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(2));
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 1, "Season pack episode_num should be 1");
                assert!(info.is_season_pack, "Should be marked as season pack");
                assert!(info.is_complete_pack, "Title contains 'Complete'");
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_season_only_group_pack_no_complete() {
        // Season-only group without "Complete" in the title.
        let patterns = vec!["S(?P<season>\\d+)\\s*$".to_string()];
        let result = parse_title_with_custom_regex(
            "Show - S03",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(3));
                assert!(info.is_season_pack, "Should be season pack");
                assert!(!info.is_complete_pack, "No 'Complete' in title");
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_season_only_group_non_numeric_falls_through() {
        // Season-only group but captured text is non-numeric.
        let patterns = vec!["S(?P<season>[a-z]+)\\s*$".to_string()];
        let result = parse_title_with_custom_regex(
            "Show - Sab",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::NoMatch),
            "Expected NoMatch (non-numeric season), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_season_and_episode_groups_prioritizes_episode() {
        // Pattern has both season AND episode groups — should be treated as
        // a standard single-episode extraction (NOT a season pack), even
        // though the season group is present.
        let patterns = vec!["S(?P<season>\\d+)EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show - S02EP05",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(2));
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
                assert!(!info.is_season_pack, "Season+episode groups should NOT be a pack");
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_optional_episode_group_matches() {
        // Optional episode group that does participate in the match.
        let patterns = vec!["EP(?P<episode>\\d+)?".to_string()];
        let result = parse_title_with_custom_regex(
            "Show EP05",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_optional_episode_group_absent() {
        // Optional episode group that does NOT participate (just "EP" with no digits).
        // Falls through to MatchedFilter since no named group participated.
        let patterns = vec!["EP(?P<episode>\\d+)?".to_string()];
        let result = parse_title_with_custom_regex(
            "Show EP",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::MatchedFilter),
            "Expected MatchedFilter (no group participated), got {:?}",
            result
        );
    }

    #[test]
    fn test_custom_regex_optional_season_group_present() {
        // Optional season group that participates: S2EP05 with optional S.
        let patterns = vec!["S(?P<season>\\d+)?EP(?P<episode>\\d+)".to_string()];
        let result = parse_title_with_custom_regex(
            "Show S2EP05",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(2));
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_optional_season_group_present_and_absent() {
        // Pattern where the season group is wrapped in an optional group.
        // The season group participates when present, and is None when absent
        // (falling through to inherit from current_season).
        let patterns = vec!["(?:S(?P<season>\\d+))?EP(?P<episode>\\d+)".to_string()];

        // Season absent — inherits from context
        let result = parse_title_with_custom_regex(
            "Show EP05",
            &patterns,
            Some(1),
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(1), "Inherits from current_season when optional season absent");
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }

        // Season present — uses captured value
        let result2 = parse_title_with_custom_regex(
            "Show S02EP05",
            &patterns,
            Some(1),
            false,
            None,
        );
        match result2 {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(2), "Uses captured season when present");
                assert_eq!(info.episodes.first().copied().unwrap_or(1), 5);
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_optional_season_only_group_present() {
        // Optional season-only group (no episode) that participates → season pack.
        let patterns = vec!["S(?P<season>\\d+)?\\s*Complete".to_string()];
        let result = parse_title_with_custom_regex(
            "Show S2 Complete",
            &patterns,
            None,
            false,
            None,
        );
        match result {
            CustomParseResult::Extracted(info) => {
                assert_eq!(info.seasons.first().copied(), Some(2));
                assert!(info.is_season_pack);
            }
            other => panic!("Expected Extracted, got {:?}", other),
        }
    }

    #[test]
    fn test_custom_regex_optional_season_only_group_absent() {
        // Optional season-only group absent — the pattern matches a title with
        // just "Season" but no season number. Falls through to MatchedFilter
        // since no named group participated in the match.
        let patterns = vec!["Season\\s*(?P<season>\\d+)?".to_string()];
        let result = parse_title_with_custom_regex(
            "Show Season",
            &patterns,
            None,
            false,
            None,
        );
        assert!(
            matches!(result, CustomParseResult::MatchedFilter),
            "Expected MatchedFilter (optional season not present), got {:?}",
            result
        );
    }

    #[test]
    fn test_parse_filename_season_pack_with_complete() {
        // "S02 Complete" is a season pack that happens to include "Complete" —
        // it should NOT be treated as a complete series pack.
        // It has a season number so is_season_pack=true, is_complete_pack=true.
        let cases = vec![
            ("[Group] Show - S02 Complete [1080p].mkv", "Show", 2, true),
            ("Show - S02 Complete.mkv", "Show", 2, true),
            ("Show S02 Complete.mkv", "Show", 2, true),
        ];

        for (filename, expected_series, expected_season, expect_season_pack) in cases {
            let res = parse_filename(filename, ParseContext::Search);
            assert!(res.is_some(), "Failed to parse: '{}'", filename);
            let res = res.unwrap();
            assert_eq!(res.series_key, expected_series, "series_key for '{}'", filename);
            assert_eq!(res.seasons.first().copied(), Some(expected_season), "season for '{}'", filename);
            assert_eq!(res.episodes.first().copied().unwrap_or(1), 1, "episode_num for '{}'", filename);
            assert_eq!(res.is_season_pack, expect_season_pack, "is_season_pack for '{}'", filename);
            assert!(res.is_complete_pack, "is_complete_pack should be true for '{}' ('Complete' in name)", filename);
        }
    }

    #[test]
    fn test_parse_filename_complete_series_pack() {
        // "Complete Series" without a season number is a genuine complete
        // SERIES pack — season should be None and is_complete_pack=true.
        let cases = vec![
            // Only works with Search context — season pack patterns are skipped in FileScan.
            ("[Group] Show Complete Series [1080p].mkv", "Show"),
            ("Show Complete Series.mkv", "Show"),
            ("Show Complete [1080p].mkv", "Show"),
        ];

        for (filename, expected_series) in cases {
            let res = parse_filename(filename, ParseContext::Search);
            assert!(res.is_some(), "Failed to parse: '{}'", filename);
            let res = res.unwrap();
            assert_eq!(res.series_key, expected_series, "series_key for '{}'", filename);
            assert!(res.seasons.is_empty(), "season should be empty for complete SERIES pack '{}'", filename);
            assert!(res.is_complete_pack, "is_complete_pack for '{}'", filename);
            assert!(res.is_season_pack, "is_season_pack for '{}'", filename);
        }
    }

    #[test]
    fn test_op_ed_filter_skipped_in_filescan() {
        // These are OP/ED theme files — must return None in FileScan context.
        let cases = vec![
            "NCED - EP12.mkv",
            "NCEDv1 - EP02-05 - Lime Tree.mkv",
            "NCEDv2 - EP06 - Lime Tree.mkv",
            "NCEDv3 - EP07-09 - Lime Tree.mkv",
            "NCEDv4 - EP10-11 - Lime Tree.mkv",
            "NCOP - Hiryuu no Kishi.mkv",
            "[Group] NCOP - Title.mkv",
            "[MockFansub] NCED - Title [1080p].mkv",
            "OP1 - Title.mkv",
            "ED2 - Title.mkv",
            "NCOP.mkv",
            "NCED.mkv",
        ];

        for filename in &cases {
            let res = parse_filename(filename, ParseContext::FileScan);
            assert!(
                res.is_none(),
                "Expected None for OP/ED file '{}' in FileScan, got {:?}",
                filename, res
            );
        }
    }

    #[test]
    fn test_op_ed_not_filtered_in_search() {
        // In Search context, OP/ED files SHOULD be parsed normally.
        // (Search titles don't have actual OP/ED video files, and even if they
        // did, the pattern matching would produce the correct episode info.)
        // Specifically "NCED - EP12.mkv" matches the EP12 pattern.
        let res = parse_filename("NCED - EP12.mkv", ParseContext::Search);
        assert!(
            res.is_some(),
            "NCED - EP12.mkv should parse in Search context"
        );
        let res = res.unwrap();
        assert_eq!(res.episodes.first().copied().unwrap_or(1), 12);
    }

    #[test]
    fn test_op_ed_does_not_block_legitimate_series() {
        // Series with "OP" or "ED" mid-title or at start-as-a-word must NOT be blocked.
        let cases = vec![
            ("Test Series - 01.mkv", "Test Series"),
            ("[Group] Test Series S01E01.mkv", "Test Series"),
            // "Ed" at the start of a real series name (not an ending theme)
            ("Ed, Edd n Eddy - S01E01.mkv", "Ed, Edd n Eddy"),
        ];
        for (filename, expected_series) in cases {
            let res = parse_filename(filename, ParseContext::FileScan);
            assert!(
                res.is_some(),
                "'{}' should not be blocked by OP/ED filter",
                filename
            );
            let res = res.unwrap();
            assert_eq!(res.series_key, expected_series);
        }
    }

    #[test]
    fn test_has_decimal_episode() {
        // ── Should have has_decimal_episode = true ────────────────────
        // These filenames contain decimal episode numbers (S01E1.5, Episode 1.5, etc.)
        let decimal_cases = &[
            "Show.Name.S01E1.5.1080p.mkv",
            "Show.Name.S01E01.50.1080p.mkv",
            "[Group] Show - S01E1.5.mkv",
            "Show - Episode 1.5.mkv",
            "Show Ep 1.5.mkv",
            "Show Episode 1.5.mkv",
            "Show-Ep-1.5.mkv",
            "S01E1.5.mkv",
        ];
        for filename in decimal_cases {
            let res = parse_filename(filename, ParseContext::FileScan);
            assert!(res.is_some(), "Expected parse to succeed for '{}'", filename);
            let res = res.unwrap();
            assert!(
                res.has_decimal_episode,
                "Expected has_decimal_episode=true for '{}' (got episodes={:?})",
                filename, res.episodes
            );
        }

        // ── Should have has_decimal_episode = false ──────────────────
        // Common scene naming with resolution directly after episode number
        // (e.g. S01E01.1080p) should NOT be flagged as decimal episodes.
        let normal_cases = &[
            "Show.Name.S01E01.mkv",
            "Show.Name.S01E01.1080p.mkv",
            "Show.Name.S02E10.2160p.WEB-DL.mkv",
            "Show - Episode 1.mkv",
            "Show Ep 1.mkv",
            "Show.Name.S01E01v2.mkv",
            "Some.Show.S01E01.1080p.WEB-DL.mkv",
        ];
        for filename in normal_cases {
            let res = parse_filename(filename, ParseContext::FileScan);
            assert!(res.is_some(), "Expected parse to succeed for '{}'", filename);
            let res = res.unwrap();
            assert!(
                !res.has_decimal_episode,
                "Expected has_decimal_episode=false for '{}'",
                filename
            );
        }
    }
}
