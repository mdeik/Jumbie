// Standard monitor mode options for dropdowns.
//
// Each entry is (API value sent to the backend, human-readable label for <select>).
// The shared crate owns the backend MonitorMode enum; the labels are UI concerns,
// so they live here rather than polluting the shared crate.
pub const MONITOR_OPTIONS: &[(&str, &str)] = &[
    ("None", "None"),
    ("All", "All Episodes"),
    ("Future", "Future Episodes"),
    ("Missing", "Missing Episodes"),
    ("Existing", "Existing Episodes"),
    ("Pilot", "Pilot Episode"),
    ("FirstSeason", "First Season"),
    ("Specials", "Specials"),
];
