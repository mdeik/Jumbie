use std::collections::HashSet;

/// CSS color class for a score's sign (positive → success, negative → danger).
pub fn score_class(val: i32) -> &'static str {
    if val > 0 {
        "text-success"
    } else if val < 0 {
        "text-danger"
    } else {
        "text-muted-color"
    }
}

/// Generate a unique name by appending a counter if the base name already exists.
///
/// # Example
/// ```
/// use std::collections::HashSet;
/// use jumbie_frontend::components::profiles::generate_unique_name;
///
/// let existing: HashSet<String> = ["New Quality", "New Quality 2"]
///     .iter().map(|s| s.to_string()).collect();
/// let name = generate_unique_name(&existing, "New Quality");
/// assert_eq!(name, "New Quality 3");
/// ```
pub fn generate_unique_name(existing_names: &HashSet<String>, base_name: &str) -> String {
    let mut name = base_name.to_string();
    let mut counter = 2;
    while existing_names.contains(&name) {
        name = format!("{} {}", base_name, counter);
        counter += 1;
    }
    name
}
