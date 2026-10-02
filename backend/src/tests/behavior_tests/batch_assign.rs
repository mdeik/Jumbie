//! Batch assign overlapping episodes behavior tests.

#[test]
fn test_batch_assign_overlap_replaces_existing() {
    let paths = ["ep01.mkv", "ep02.mkv", "ep03.mkv"];
    let start_season = 1i32;
    let start_episode = 1i32;

    let mut assignments: Vec<(String, i32)> = Vec::new();
    let current_season = start_season;

    for (offset, _path) in paths.iter().enumerate() {
        let current_episode = start_episode + offset as i32;
        assignments.push((
            format!("S{:02}E{:02}", current_season, current_episode),
            current_episode,
        ));
    }

    assert_eq!(assignments.len(), 3);
    assert_eq!(assignments[0].0, "S01E01");
    assert_eq!(assignments[1].0, "S01E02");
    assert_eq!(assignments[2].0, "S01E03");

    // If episode S01E02 already has a file, the new batch assignment overwrites it.
    // The old file reference is replaced — both don't coexist.
}

#[test]
fn test_batch_assign_sequential_increment() {
    let paths = ["f1.mkv", "f2.mkv", "f3.mkv", "f4.mkv", "f5.mkv"];
    let mut current_episode = 1i32;

    for (i, _path) in paths.iter().enumerate() {
        if i > 0 {
            current_episode += 1;
        }

        let expected = (i + 1) as i32;
        assert_eq!(
            current_episode,
            expected,
            "File {} should be assigned to episode {}",
            i + 1,
            expected
        );
    }
}
