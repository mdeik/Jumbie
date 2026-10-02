use super::fetch::merge_series_aliases;

#[test]
fn merge_empty_inputs_yields_empty() {
    let result = merge_series_aliases(&[], &[], "");
    assert!(result.is_empty());
}

#[test]
fn merge_new_aliases_only() {
    let result = merge_series_aliases(&[], &["A".into(), "B".into()], "");
    assert_eq!(result, vec!["A", "B"]);
}

#[test]
fn merge_existing_only() {
    let result = merge_series_aliases(&["X".into(), "Y".into()], &[], "");
    assert_eq!(result, vec!["X", "Y"]);
}

#[test]
fn merge_existing_then_new() {
    let result = merge_series_aliases(
        &["Existing1".into(), "Existing2".into()],
        &["New1".into(), "New2".into()],
        "",
    );
    assert_eq!(result, vec!["Existing1", "Existing2", "New1", "New2"]);
}

#[test]
fn merge_dedup_new_against_existing() {
    let result = merge_series_aliases(
        &["Common".into(), "Existing".into()],
        &["Common".into(), "New".into()],
        "",
    );
    assert_eq!(result, vec!["Common", "Existing", "New"]);
}

#[test]
fn merge_empty_existing_aliases_skipped() {
    // Empty strings in existing DB are skipped (legacy data).
    let result = merge_series_aliases(&["".into(), "Valid".into()], &["AlsoValid".into()], "");
    assert_eq!(result, vec!["Valid", "AlsoValid"]);
}

#[test]
fn merge_title_injected_when_aliases_exist() {
    let result = merge_series_aliases(&["Alias1".into()], &["Alias2".into()], "Series Title");
    assert_eq!(result, vec!["Alias1", "Series Title", "Alias2"]);
}

#[test]
fn merge_title_not_injected_when_no_aliases() {
    let result = merge_series_aliases(&[], &[], "Series Title");
    assert!(result.is_empty());
}

#[test]
fn merge_title_not_injected_when_empty_title() {
    let result = merge_series_aliases(&["Alias1".into()], &["Alias2".into()], "");
    assert_eq!(result, vec!["Alias1", "Alias2"]);
}

#[test]
fn merge_title_not_injected_when_already_in_existing() {
    // Provider title is already part of existing aliases — should not be duplicated.
    let result = merge_series_aliases(
        &["Series Title".into(), "Alias1".into()],
        &["Alias2".into()],
        "Series Title",
    );
    assert_eq!(result, vec!["Series Title", "Alias1", "Alias2"]);
}
