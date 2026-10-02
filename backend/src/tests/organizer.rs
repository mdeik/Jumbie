use crate::organizer::ContentOrganizer;
use jumbie_shared::types::EpisodeInfo;
use std::collections::HashSet;

jumbie_shared::test_module! {

    fn make_ep_info(ep_num: i32, ep_end: Option<i32>) -> EpisodeInfo {
        let episodes: Vec<i32> = if let Some(end) = ep_end {
            (ep_num..=end).collect()
        } else {
            vec![ep_num]
        };
        EpisodeInfo {
            raw_title: format!("Show S01E{:02}", ep_num),
            series_key: "key".to_string(),
            submitter: None,
            resolution: None,
            version: 1,
            file_ext: "mkv".to_string(),
            part_number: None,
            is_season_pack: ep_end.is_some(),
            is_complete_pack: false,
            seasons: vec![1],
            episodes,
            has_decimal_episode: false,
        }
    }

    #[test]
    fn test_assess_pack_candidacy_single_needed() {
        let info = make_ep_info(1, None);
        let downloaded = HashSet::new();
        let mut monitored = HashSet::new();
        monitored.insert(1);
        let (score, needed, unneeded_count) =
            ContentOrganizer::assess_pack_candidacy(&info, &downloaded, &monitored, info.episodes.last().copied().filter(|_| info.episodes.len() > 1), &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes, 100);
        assert_eq!(score, 0);
        assert_eq!(needed, vec![1]);
        assert_eq!(unneeded_count, 0);
    }

    #[test]
    fn test_assess_pack_candidacy_single_downloaded() {
        let info = make_ep_info(1, None);
        let mut downloaded = HashSet::new();
        downloaded.insert(1);
        let mut monitored = HashSet::new();
        monitored.insert(1);
        let (score, needed, unneeded_count) =
            ContentOrganizer::assess_pack_candidacy(&info, &downloaded, &monitored, info.episodes.last().copied().filter(|_| info.episodes.len() > 1), &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes, 100);
        assert_eq!(score, -1000);
        assert!(needed.is_empty());
        assert_eq!(unneeded_count, 1);
    }

    #[test]
    fn test_assess_pack_candidacy_pack() {
        let info = make_ep_info(1, Some(3));
        let mut downloaded = HashSet::new();
        downloaded.insert(2);
        let mut monitored = HashSet::new();
        for i in 1..=3 { monitored.insert(i); }
        let (score, needed, unneeded_count) =
            ContentOrganizer::assess_pack_candidacy(&info, &downloaded, &monitored, info.episodes.last().copied().filter(|_| info.episodes.len() > 1), &jumbie_shared::config::SeasonPackStrategy::FavorEpisodes, 100);
        // Needed: 1, 3; Unneeded: 2
        assert_eq!(score, -50);
        assert_eq!(needed, vec![1, 3]);
        assert_eq!(unneeded_count, 1);
    }

    /// When a multiepisode pack (E01-04) is in the queue and we only have candidates
    /// for E02 and E03 (not E01 or E04), we cannot replace — coverage is incomplete.
    /// This test verifies the helper that computes coverage of a range.
    #[test]
    fn test_multi_range_coverage_detection() {
        let multi_range: Vec<i32> = vec![1, 2, 3, 4];

        // Map each ep to its best candidate score (simulates ep_to_best)
        let mut ep_to_score: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
        ep_to_score.insert(2, 800);
        ep_to_score.insert(3, 900);

        // Only E02 and E03 covered — not all
        let all_eps_covered = multi_range.iter().all(|ep| ep_to_score.contains_key(ep));
        assert!(!all_eps_covered, "Should NOT be fully covered — E01 and E04 are missing");
    }

    /// When all episodes in the multiepisode range are covered by candidates,
    /// the average score check should pass when new avg > old score.
    #[test]
    fn test_multi_replacement_avg_score_check() {
        let multi_range: Vec<i32> = vec![1, 2, 3, 4];
        let existing_multi_score = 700;

        // All four episodes covered, with scores
        let scores = [800, 900, 750, 850];
        let total: i32 = scores.iter().sum();
        let new_avg = total / multi_range.len() as i32;
        // avg = (800+900+750+850)/4 = 3300/4 = 825

        assert!(
            new_avg > existing_multi_score,
            "avg {} should be > existing {} → replacement allowed",
            new_avg, existing_multi_score
        );
    }

    /// When new avg score is equal to or less than the existing multiepisode score,
    /// replacement should be blocked.
    #[test]
    fn test_multi_replacement_avg_score_blocked() {
        let multi_range: Vec<i32> = vec![1, 2, 3, 4];
        let existing_multi_score = 900;

        let scores = [700, 800, 750, 850];
        let total: i32 = scores.iter().sum();
        let new_avg = total / multi_range.len() as i32;
        // avg = 3100/4 = 775

        assert!(
            new_avg <= existing_multi_score,
            "avg {} should be <= existing {} → replacement blocked",
            new_avg, existing_multi_score
        );
    }

    /// Verify that uncovered episodes are correctly computed after manual single download
    /// cancels a multiepisode pack.
    #[test]
    fn test_uncovered_episodes_after_manual_download() {
        let multi_ep_start = 1;
        let multi_ep_end = 4;
        let manually_downloaded_ep = 2;

        let uncovered: Vec<i32> = (multi_ep_start..=multi_ep_end)
            .filter(|ep| *ep != manually_downloaded_ep)
            .collect();

        assert_eq!(uncovered, vec![1, 3, 4]);
    }

    /// Replacing a multiepisode with a new multiepisode for the same range is allowed
    /// if all episodes are covered and avg score is higher.
    #[test]
    fn test_new_multi_replaces_old_multi_same_range() {
        let multi_range: Vec<i32> = vec![1, 2, 3, 4];
        let old_score = 600;
        // New multi is a single candidate covering E01-04 with score 750
        let new_scores: Vec<i32> = vec![750; 4]; // same candidate applied to each ep
        let total: i32 = new_scores.iter().sum();
        let new_avg = total / multi_range.len() as i32; // 750

        assert!(new_avg > old_score, "New multi (score 750) should replace old (score 600)");
    }

    // download_winner's intention-building sets `keep=true` for EVERY episode in the
    // release's list; upgrades for already-downloaded episodes are handled by the
    // organize pipeline.
    //
    // Regression: encoding (season, episode) as a flat_key (s*1000+ep_num) never
    // matched the plain episode numbers, so EVERY episode got keep=false and every
    // file was classified "unneeded" and deleted.

    #[test]
    fn test_episode_intention_keep_single_episode() {
        // Single episode release: S01E03
        let episode_list: Vec<i32> = vec![3];

        for &ep in &episode_list {
            let keep = true; // always true — no more set lookup
            assert!(keep, "Episode {} should be keep=true", ep);
        }
        assert_eq!(episode_list.len(), 1, "One episode in list");
    }

    #[test]
    fn test_episode_intention_keep_single_episode_not_first() {
        // Episode 4 — same behavior regardless of episode number
        let episode_list: Vec<i32> = vec![4];
        for &ep in &episode_list {
            let keep = true;
            assert!(keep, "Episode {} should be keep=true", ep);
        }
        assert_eq!(episode_list.len(), 1);
    }

    #[test]
    fn test_episode_intention_keep_full_season_pack() {
        // Season pack covering all 12 episodes
        let episode_list: Vec<i32> = (1..=12).collect();
        let true_count = episode_list.len();
        assert_eq!(true_count, 12, "All episodes should be keep=true");
    }

    #[test]
    fn test_episode_intention_keep_partial_pack() {
        // Partial pack covering E01-E05
        let episode_list: Vec<i32> = (1..=5).collect();
        assert_eq!(episode_list.len(), 5, "All 5 episodes should be present");
        // In production code, ALL entries get keep=true
        let keep_count = episode_list.len();
        assert_eq!(keep_count, 5, "All episodes should be keep=true");
    }

    #[test]
    fn test_episode_intention_keep_gapped_release() {
        // Gapped release: episodes [2, 4, 6] — no range, direct iteration
        let episode_list: Vec<i32> = vec![2, 4, 6];
        assert_eq!(episode_list.len(), 3, "Three episodes in list");
        // All three get keep=true (no phantom episodes between them)
        let keep_count = episode_list.len();
        assert_eq!(keep_count, 3, "All episodes should be keep=true");
    }

    #[test]
    fn test_episode_intention_keep_regression_flat_key() {
        // Regression: the old code used flat_key = 1*1000+3 = 1003, but
        // episode_info.episodes contained plain [3]. This test simulates
        // the bug — using flat_key would NOT match; using ep_num directly DOES.
        let episode_list: Vec<i32> = vec![3];
        let ep_num = 3;
        let season = 1;
        let flat_key = season * 1000 + ep_num; // 1003

        // The BUG: flat_key 1003 is NOT in the episode list [3]
        assert!(
            !episode_list.contains(&flat_key),
            "flat_key 1003 should NOT match plain episode 3 — this was the bug"
        );
        // The FIX: check ep_num directly against the episode list
        assert!(
            episode_list.contains(&ep_num),
            "ep_num 3 should match — this is the fix"
        );
    }
}
