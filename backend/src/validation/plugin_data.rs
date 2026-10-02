//! Validation for metadata types flowing FROM plugins TO the backend.
//!
//! Scalar field limits are defined in `jumbie_shared::validation::fields` and re-used
//! here via the validators named in the table below.
//!
//! | Field | SSoT validator | Limit |
//! |-------|---------------|-------|
//! | `EpisodeMetadata.season` | `validate_season_number` | 0..10000 |
//! | `EpisodeMetadata.episode` | `validate_episode_number` | 1..10000 |
//! | `EpisodeMetadata.runtime` | `validate_episode_runtime` | 0..86400s |
//! | `EpisodeMetadata.image_url` | `validate_url` | http/https |
//! | `SeasonMetadata.season` | `validate_season_number` | 0..10000 |
//! | `SeriesMetadataInfo.name` | `validate_title` → [`MAX_TITLE_LENGTH`] | 0..10000 |
//! | `SeriesMetadataInfo.image_url` | `validate_url` | http/https |
//!
//! [`MAX_TITLE_LENGTH`]: jumbie_shared::validation::fields::MAX_TITLE_LENGTH

use std::collections::HashSet;

use jumbie_shared::validation::fields::ValidationError;
use jumbie_shared::validation::fields::{
    MAX_EPISODE, MAX_ID_LENGTH, validate_episode_number, validate_season_number, validate_url,
};
use jumbie_shared::validation::plugin_data::{Validate, validate_result};

use crate::plugins::metadata::{
    EpisodeMetadata, SeasonMetadata, SeriesMetadata, SeriesMetadataInfo,
};

/// Max episode runtime in seconds (24 hours). Separate from `validate_runtime`
/// (minutes) because `EpisodeMetadata.runtime` is in seconds (TVDB/TVMaze format).
pub const MAX_EPISODE_RUNTIME_SECS: i32 = 86_400;

impl Validate for EpisodeMetadata {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        if self.unique_id.trim().is_empty() {
            errors.push(ValidationError(
                "EpisodeMetadata unique_id is empty".to_string(),
            ));
        }
        if self.unique_id.len() > MAX_ID_LENGTH {
            errors.push(ValidationError(format!(
                "EpisodeMetadata unique_id too long ({} chars, max {})",
                self.unique_id.len(),
                MAX_ID_LENGTH
            )));
        }
        if let Err(e) = validate_season_number(&self.season.to_string()) {
            errors.push(e);
        }
        if let Err(e) = validate_episode_number(self.episode) {
            errors.push(e);
        }
        // Empty titles are allowed (proactive enrichment fills them later); only
        // length and control characters are checked when a title is present.
        if !self.title.is_empty() {
            use jumbie_shared::validation::fields::validate_title;
            if let Err(e) = validate_title(&self.title) {
                errors.push(e);
            }
        }
        if let Some(r) = self.runtime
            && !(0..=MAX_EPISODE_RUNTIME_SECS).contains(&r)
        {
            errors.push(ValidationError(format!(
                "EpisodeMetadata runtime out of range: {} (expected 0..{}s)",
                r, MAX_EPISODE_RUNTIME_SECS
            )));
        }
        // TVDB/TVMaze return absolute URLs, so relative paths are rejected.
        if let Some(ref url) = self.image_url
            && let Err(e) = validate_url(url)
        {
            errors.push(e);
        }

        validate_result(errors)
    }
}

impl Validate for SeasonMetadata {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        if let Err(e) = validate_season_number(&self.season.to_string()) {
            errors.push(e);
        }
        if self.episode_count < 0 {
            errors.push(ValidationError(format!(
                "SeasonMetadata episode_count is negative: {}",
                self.episode_count
            )));
        }
        if self.episode_count > MAX_EPISODE {
            errors.push(ValidationError(format!(
                "SeasonMetadata episode_count too large: {} (max {})",
                self.episode_count, MAX_EPISODE
            )));
        }

        validate_result(errors)
    }
}

impl Validate for SeriesMetadata {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        for (i, ep) in self.episodes.iter().enumerate() {
            if let Err(ep_errors) = ep.validate() {
                for err in ep_errors {
                    errors.push(ValidationError(format!("Episode [{}]: {}", i, err.0)));
                }
            }
        }
        for (i, season) in self.seasons.iter().enumerate() {
            if let Err(s_errors) = season.validate() {
                for err in s_errors {
                    errors.push(ValidationError(format!("Season [{}]: {}", i, err.0)));
                }
            }
        }
        // Duplicate season numbers indicate a provider bug and are rejected.
        let mut seen_seasons = HashSet::new();
        for season in &self.seasons {
            if !seen_seasons.insert(season.season) {
                errors.push(ValidationError(format!(
                    "SeriesMetadata duplicate season number: {}",
                    season.season
                )));
            }
        }

        validate_result(errors)
    }
}

impl Validate for SeriesMetadataInfo {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        // A provider may omit a name; only validate length and control characters
        // when a name is present.
        if !self.name.is_empty() {
            use jumbie_shared::validation::fields::validate_title;
            if let Err(e) = validate_title(&self.name) {
                errors.push(e);
            }
        }
        if let Some(ref url) = self.image_url
            && let Err(e) = validate_url(url)
        {
            errors.push(e);
        }
        // Aliases are best-effort: accept any structure the provider returns. Empty
        // entries are harmless downstream (the alias merge skips empty strings).

        validate_result(errors)
    }
}
