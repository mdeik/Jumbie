use jumbie_shared::mapping::{
    ResolvedSeason, SeriesSettings, parse_season_num, resolve_season_for_series,
};
use jumbie_shared::parsing::{ParseContext, parse_filename};
use std::path::Path;

/// Strip a trailing auxiliary extension (`Show - S01E01.en.srt` → `Show - S01E01.en`)
/// so the name can be fed to the shared parser as if it were a media filename.
///
/// Trailing language/flag tokens after the episode are intentionally left alone:
/// library files are renamed to the user's template by the rename queue, and the
/// shared [`parse_filename`] already ignores anything past the episode token.
fn strip_aux_extension(filename: &str) -> String {
    if let Some(ext) = Path::new(filename).extension().and_then(|e| e.to_str())
        && jumbie_shared::media_format::is_auxiliary_ext(ext)
    {
        return filename[..filename.len() - ext.len() - 1].to_string();
    }
    filename.to_string()
}

/// Parse a file path into (season, episode_display, part_number) for the
/// Manage Series Files auto-assign modal.
///
/// Filename parsing delegates entirely to the shared crate's [`parse_filename`]
/// (SSoT), so every naming pattern the backend scanner supports works here
/// automatically.
///
/// The season is resolved through the shared [`resolve_season_for_series`] (SSoT),
/// so the preview matches exactly what a scan would produce: a season alias in
/// the title wins, then the filename's season, then a season folder in the path,
/// then the default (season 1). Ambiguous season aliases resolve to an empty
/// result, matching the scanner's "leave unassigned" behaviour.
pub fn parse_season_episode_from_filename(
    path: &str,
    settings: &SeriesSettings,
    absolute: bool,
) -> (String, String, Option<u32>) {
    // Only the basename participates in filename parsing; the full path is still
    // handed to the resolver so a season folder can supply the season.
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    // Auxiliary sidecars use a subtitle/nfo extension; strip it so
    // `Show - S01E01.en.srt` is parsed like its video. Any language/flag tokens
    // after the episode are ignored by the shared parser.
    let filename = strip_aux_extension(file_name);
    // The shared crate's parse_filename expects a full filename including
    // extension. Keep a real media extension; otherwise append a dummy video one
    // (a bare stem, or a title containing dots like `Show.S01E03`).
    let has_media_ext = Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| {
            jumbie_shared::media_format::is_valid_media_ext(&e)
                || jumbie_shared::media_format::is_nfo_ext(&e)
        });
    let full_name = if has_media_ext {
        filename
    } else {
        format!("{}.mkv", filename)
    };

    if let Some(info) = parse_filename(&full_name, ParseContext::FileScan)
        && !info.is_season_pack
        && !info.is_complete_pack
    {
        let parsed_s = match resolve_season_for_series(&info, Path::new(path), settings, absolute) {
            ResolvedSeason::Season(season, _) => {
                // Normalise "01" → "1" so `format_episode_display` remains the one
                // place that zero-pads a season label for display.
                parse_season_num(&season)
                    .map(|n| n.to_string())
                    .unwrap_or(season)
            }
            ResolvedSeason::Unneeded => return (String::new(), String::new(), None),
        };
        let parsed_e = if info.episodes.len() > 1 {
            let first = info.episodes.first().copied().unwrap_or(1);
            let last = info.episodes.last().copied().unwrap_or(1);
            format!("{}-{}", first, last)
        } else {
            info.episodes.first().copied().unwrap_or(1).to_string()
        };
        return (parsed_s, parsed_e, info.part_number);
    }

    (String::new(), String::new(), None)
}

pub fn format_episode_display(parsed_s: &str, parsed_e: &str, parsed_part: Option<u32>) -> String {
    let parsed_e_display = if parsed_e.contains('-') {
        parsed_e.to_string()
    } else {
        format!("{:02}", parsed_e.parse::<i32>().unwrap_or(0))
    };
    let parsed_s_display = if parsed_s.len() == 1 {
        format!("0{}", parsed_s)
    } else {
        parsed_s.to_string()
    };
    if let Some(part) = parsed_part {
        format!("S{}E{} pt{}", parsed_s_display, parsed_e_display, part)
    } else {
        format!("S{}E{}", parsed_s_display, parsed_e_display)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jumbie_shared::mapping::{SeasonOverride, SeriesSettings};

    fn settings_with_alias(season: &str, alias: &str) -> SeriesSettings {
        let mut settings = SeriesSettings::default();
        settings.season.insert(
            season.to_string(),
            SeasonOverride {
                season: season.to_string(),
                aliases: vec![alias.to_string()],
                ..Default::default()
            },
        );
        settings
    }

    #[test]
    fn parses_sidecar_with_trailing_tokens() {
        // Language/flag tokens after the episode don't need stripping: the shared
        // parser ignores them, and the auxiliary extension is stripped for us.
        let s = SeriesSettings::default();
        assert_eq!(
            parse_season_episode_from_filename("Show - S01E01.en", &s, false),
            ("1".to_string(), "1".to_string(), None)
        );
        assert_eq!(
            parse_season_episode_from_filename("Show - S01E01", &s, false),
            ("1".to_string(), "1".to_string(), None)
        );
        assert_eq!(
            parse_season_episode_from_filename("Show - S01E02.en.srt", &s, false),
            ("1".to_string(), "2".to_string(), None)
        );
    }

    #[test]
    fn parses_nfo_sidecar() {
        let s = SeriesSettings::default();
        assert_eq!(
            parse_season_episode_from_filename("Show.S01E03.forced.nfo", &s, false),
            ("1".to_string(), "3".to_string(), None)
        );
    }

    #[test]
    fn season_alias_wins_over_filename_season() {
        // "Whisker Mini" aliases season 0; the parsed S03 is ignored, matching
        // the backend scanner.
        let s = settings_with_alias("0", "Whisker Mini");
        assert_eq!(
            parse_season_episode_from_filename("Whisker Mini Anime S03E01.mkv", &s, false),
            ("0".to_string(), "1".to_string(), None)
        );
    }

    #[test]
    fn season_less_filename_defaults_to_season_one() {
        // `parse_filename` stamps the default season (1) for a bare-number file,
        // so the resolver reports it as-is — identical to the scanner.
        let s = SeriesSettings::default();
        assert_eq!(
            parse_season_episode_from_filename("Series Name - 05.mkv", &s, false),
            ("1".to_string(), "5".to_string(), None)
        );
    }

    #[test]
    fn ambiguous_season_alias_leaves_the_file_unassigned() {
        let mut s = SeriesSettings::default();
        for season in ["0", "2"] {
            s.season.insert(
                season.to_string(),
                SeasonOverride {
                    season: season.to_string(),
                    aliases: vec!["Whisker Mini".to_string()],
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            parse_season_episode_from_filename("Whisker Mini Anime - 01.mkv", &s, false),
            (String::new(), String::new(), None)
        );
    }

    #[test]
    fn absolute_mode_ignores_season_aliases() {
        let mut s = SeriesSettings::default();
        s.season_absolute.insert(
            "1".to_string(),
            SeasonOverride {
                season: "1".to_string(),
                aliases: vec!["Whisker Mini".to_string()],
                ..Default::default()
            },
        );
        assert_eq!(
            parse_season_episode_from_filename("Whisker Mini Anime - 01.mkv", &s, true),
            ("1".to_string(), "1".to_string(), None)
        );
    }
}
