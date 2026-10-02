//! File assignment behavior tests.
//!
//! Assignment/disown behavior is verified against the database, close to the SSoT
//! (`db::ownership`), so a string literal here cannot drift from the real SQL:
//!
//!   * `tests::regression::test_reorg_multi_episode_sets_path_on_all_covered`
//!   * `tests::regression::test_manual_assign_allows_reassign_to_different_episode`
//!   * `tests::regression::test_assign_holder_guard_is_mode_scoped`
//!   * `tests::regression::test_assign_stamps_release_feed_date`
//!   * `tests::regression::test_assign_falls_back_to_mtime_without_release_info`
//!   * `tests/api_files_ownership.rs` — unassign/delete, multi-episode, multipart
//!
//! (The previous checks here asserted hand-written SQL that no longer existed —
//! e.g. a `series_title`/`quality` upsert key — so they passed while describing
//! nothing. They were removed rather than kept as drift.)
