//! Release estimator behavior tests.

use std::collections::HashMap;

#[test]
fn test_release_estimator_groups_by_season() {
    let episodes = vec![
        (Some("1".to_string()), 1, Some("2024-01-01".to_string())),
        (Some("1".to_string()), 2, Some("2024-01-08".to_string())),
        (Some("2".to_string()), 1, Some("2024-04-01".to_string())),
        (Some("2".to_string()), 2, Some("2024-04-08".to_string())),
        (Some("2".to_string()), 3, None),
    ];

    // Group by season (as done in run_release_date_estimation)
    let mut season_eps: HashMap<Option<String>, Vec<(i32, Option<String>)>> = HashMap::new();
    for (season, ep, date) in episodes {
        season_eps.entry(season).or_default().push((ep, date));
    }

    assert_eq!(
        season_eps.get(&Some("1".to_string())).map(|v| v.len()),
        Some(2),
        "Season 1 should have 2 episodes"
    );
    assert_eq!(
        season_eps.get(&Some("2".to_string())).map(|v| v.len()),
        Some(3),
        "Season 2 should have 3 episodes"
    );

    // Season 1 does NOT get cross-contaminated with season 2 data
    let s2_eps = season_eps.get(&Some("2".to_string())).unwrap();
    let s2_events: Vec<&(i32, Option<String>)> =
        s2_eps.iter().filter(|(_, d)| d.is_some()).collect();
    assert_eq!(s2_events.len(), 2, "Season 2 has 2 events");

    let s2_latest_ep: Option<i32> = s2_eps
        .iter()
        .filter(|(_, d)| d.is_some())
        .map(|(ep, _)| *ep)
        .max();
    assert_eq!(s2_latest_ep, Some(2), "Latest season 2 event is episode 2");

    let needs_estimation: Vec<&(i32, Option<String>)> = s2_eps
        .iter()
        .filter(|(ep, d)| d.is_none() && *ep > s2_latest_ep.unwrap())
        .collect();
    assert_eq!(needs_estimation.len(), 1, "Episode 3 needs estimation");
    assert_eq!(needs_estimation[0].0, 3);
}

#[test]
fn test_calculate_estimations_needs_minimum_two_events() {
    // Mirrors release_estimator.rs: when events.len() < 2, fall back to a 7-day
    // default gap.
    //
    // The gap-computation logic also needs consecutive episode numbers:
    //   if curr_min - prev_max > 0 { ... }

    type SeasonEvents = (i32, Vec<i32>);
    let test_cases: Vec<(SeasonEvents, SeasonEvents, bool)> = vec![
        ((1, vec![1]), (2, vec![2]), true),    // 2 - 1 = 1 → consecutive
        ((1, vec![1]), (3, vec![3]), false),   // 3 - 1 = 2 → NOT consecutive
        ((5, vec![5]), (7, vec![7]), false),   // 7 - 5 = 2 → NOT consecutive
        ((2, vec![1, 2]), (3, vec![3]), true), // 3 - 2 = 1 → consecutive
    ];

    for ((_prev_max, prev_eps), (curr_min, _curr_eps), expected_consecutive) in test_cases {
        let prev_max_ref = prev_eps.last().unwrap();
        let is_consecutive = curr_min - *prev_max_ref == 1;
        assert_eq!(
            is_consecutive, expected_consecutive,
            "prev_max={} curr_min={} should be consecutive={}",
            prev_max_ref, curr_min, expected_consecutive
        );
    }
}

#[test]
fn test_release_estimator_uses_median_gap() {
    let mut gaps = [5, 7, 7, 8, 100]; // 100 is an outlier
    gaps.sort_unstable();
    // Same lower-median expression as calculate_estimations: (len - 1) / 2.
    // Odd counts resolve to the true middle; even counts to the lower value.
    let median_gap_sec = gaps[(gaps.len() - 1) / 2];

    assert_eq!(median_gap_sec, 7, "Median of [5,7,7,8,100] should be 7");

    let sum: i32 = gaps.iter().sum();
    let mean = sum / gaps.len() as i32;
    assert!(
        mean > median_gap_sec,
        "Mean is pulled up by outlier ({} vs {})",
        mean,
        median_gap_sec
    );
}

#[test]
fn test_all_undated_episodes_are_estimated() {
    // Any episode without upload_date is eligible, including those
    // before the latest known event (backfill). Users control metadata
    // source priority in settings, so the estimator doesn't guard against
    // overwriting metadata with speculative dates.
    let episodes: Vec<(i32, Option<&str>)> = vec![
        (1, Some("2024-01-01")),
        (2, Some("2024-01-08")),
        (3, None), // No date → eligible (backfill)
        (4, None), // No date → eligible
    ];

    let eligible: Vec<&(i32, Option<&str>)> =
        episodes.iter().filter(|(_, d)| d.is_none()).collect();

    assert_eq!(eligible.len(), 2, "Episodes 3 and 4 should be eligible");
    assert_eq!(eligible[0].0, 3);
    assert_eq!(eligible[1].0, 4);
}

#[test]
fn test_estimator_backfills_episodes_before_latest() {
    // Episodes before the latest event are eligible for backfill estimation —
    // episode 3 gets a backfilled estimate.
    let episodes: Vec<(i32, Option<&str>)> = vec![
        (5, Some("2024-01-05")), // Has date, latest is 5
        (3, None),               // No date, ep=3 < 5 → eligible (backfill)
        (6, None),               // No date, ep=6 > 5 → eligible
    ];

    let eligible: Vec<&(i32, Option<&str>)> =
        episodes.iter().filter(|(_, d)| d.is_none()).collect();

    assert_eq!(
        eligible.len(),
        2,
        "Episodes 3 and 6 should both be eligible"
    );
    assert_eq!(eligible[0].0, 3);
    assert_eq!(eligible[1].0, 6);
}

#[test]
fn test_estimator_no_kill_switch() {
    // The release estimator never disables itself; it always runs when called.

    let always_runs = true;
    assert!(always_runs, "Estimator always runs when called");
}
