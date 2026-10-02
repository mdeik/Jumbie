use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, instrument, trace};

use crate::models::media::{MediaEntry, ReleaseCandidate};
use crate::organizer::ContentOrganizer;
use jumbie_shared::parsing::CustomParseResult;
use jumbie_shared::plugin::Capability;
use jumbie_shared::types::MappingRule;

impl ContentOrganizer {
    #[instrument(skip(self))]
    pub async fn process_sources(&self, plugin_id: Option<&str>) -> Result<()> {
        // Plugin identity is already recorded on the `process_sources` span.

        // Hold the plugin_manager read lock only while collecting plugins, then release
        // it before spawning tasks so no lock is held across the concurrent fetch's
        // .await points (each task owns its plugin object by value).
        let plugins = {
            let pm = self.plugin_manager.read().await;
            match plugin_id {
                Some(pid) => pm
                    .get_plugin_with_capability(pid, Capability::FeedProvider)
                    .into_iter()
                    .collect::<Vec<_>>(),
                None => pm.get_plugins_by_capability(Capability::FeedProvider),
            }
        };

        // Parallel fetch
        // join_all so slow feeds don't block fast ones; the trade-off is all feed data
        // arriving in memory at once, acceptable since feed entries are small.
        let mut tasks = Vec::new();
        for plugin in plugins {
            let plugin_instance_id = plugin.instance_id().to_string();
            let instance_id = plugin_instance_id.clone();

            tasks.push(async move {
                tracing::debug!("Fetching entries from plugin {}", plugin_instance_id);
                let result = crate::plugins::bridge::sources::fetch_entries(plugin.as_ref())
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()));
                (instance_id, result)
            });
        }

        let results = futures::future::join_all(tasks).await;

        // Load the full mapping set once and thread it through both identification phases.
        let all_mappings = self.db.get_all_series_mappings().await?;

        // Gate: one combined matcher answering "does this title match ANY series'
        // name/alias/pattern?". A miss skips the entry, saving Phase 1 and Phase 2.
        // It is a superset of Phase 1 — false positives are fine, false negatives
        // impossible. The matcher also carries each series' compiled Phase 1
        // patterns, so both phases share one compilation.
        let matcher = crate::source_processor::SeriesMatcher::build(&all_mappings);

        let mut candidates = Vec::new();
        for (instance_id, res) in results {
            match res {
                Ok(feed_entries) => {
                    for entry in &feed_entries {
                        if crate::task::is_shutdown_requested(
                            &self.shutdown_token,
                            "source processing mid-entries",
                        ) {
                            break;
                        }

                        // Fast pre-filter: skip entries matching no series (replaces N
                        // pattern attempts).
                        if !matcher.is_match(&entry.title) {
                            trace!("Gate: no series matches '{}', skipping", entry.title);
                            continue;
                        }

                        trace!("Processing entry: {}", entry.title);
                        if let Ok(candidates_for_entry) = self
                            .process_entry_with_mappings(
                                entry,
                                &instance_id,
                                &all_mappings,
                                &matcher,
                                None,
                            )
                            .await
                        {
                            for candidate in candidates_for_entry {
                                candidates.push(candidate);
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("Failed to fetch from source: {}", e),
            }
        }

        if candidates.is_empty() {
            debug!("No valid candidates found");
            return Ok(());
        }

        // Winner selection is a batch operation over ALL candidates, so two feeds
        // can't independently download the same episode — select_winners sees the
        // union and picks the single best across sources.
        let winners = self.select_winners(candidates).await?;
        debug!("Selected {} winners", winners.len());

        // Collapse winners sharing a magnet/URL into one with multi_targets so the file
        // is downloaded once and copied to each target at organize.
        let winners = self.merge_shared_downloads(winners).await?;

        for winner in winners {
            if crate::task::is_shutdown_requested(
                &self.shutdown_token,
                "source processing mid-winners",
            ) {
                break;
            }
            self.download_winner(&winner).await?;
        }

        Ok(())
    }

    /// Entry point for per-entry processing with pre-loaded mappings.
    ///
    /// Callers that have already loaded `all_mappings` (e.g. `process_sources`)
    /// should use this directly to avoid a redundant DB load + deserialization.
    pub(crate) async fn process_entry_with_mappings(
        &self,
        entry: &MediaEntry,
        instance_id: &str,
        all_mappings: &HashMap<String, MappingRule>,
        matcher: &crate::source_processor::SeriesMatcher,
        required_season: Option<i32>,
    ) -> Result<Vec<ReleaseCandidate>> {
        let title = &entry.title;

        // SSoT: effective numbering-mode default, read once for identification.
        let global_absolute = self.db_config().await.general.absolute_numbering;

        // Phase 1: try each tracked series' patterns (user-defined or auto-generated)
        // against the raw title, identifying the series without parse_filename.
        //
        // # Outcomes
        // - Extraction match → use extracted (season, episode) directly
        // - Filter-only match → run parse_filename for values only
        // - No match → fall through to Phase 2
        if let Some((mapping_base, maybe_extracted)) =
            matcher.identify(all_mappings, title, instance_id, global_absolute)
        {
            let info = if let Some(CustomParseResult::Extracted(extracted)) = &maybe_extracted {
                trace!(
                    "Phase 1: pattern identified series '{}' with extraction: season={:?}, episode={}",
                    mapping_base.target_title,
                    extracted.seasons.first(),
                    extracted.episodes.first().copied().unwrap_or(0)
                );
                (**extracted).clone()
            } else {
                // Filter-only match — pattern identified the series but extracted no
                // episode/season; run the title parser for values only (series_key
                // ignored). A season-scoped search additionally verifies the season.
                match crate::search::parse_search_result_title(title, required_season) {
                    Some(i) => i,
                    None => {
                        trace!(
                            "Phase 1: pattern matched but title parse returned None for '{}'",
                            title
                        );
                        return Ok(vec![]);
                    }
                }
            };

            return self
                .process_entry_inner(title, info, mapping_base, instance_id, entry, true)
                .await;
        }

        // Phase 2: title-parser fallback, reached only when no pattern matched.
        let info = match crate::search::parse_search_result_title(title, required_season) {
            Some(i) => i,
            None => {
                trace!("Phase 2: title parse returned None for: {}", title);
                return Ok(vec![]);
            }
        };

        let mapping_base =
            match crate::source_processor::lookup_mapping_in(&info.series_key, all_mappings) {
                Some((_id, m)) => m,
                None => {
                    trace!(
                        "Phase 2: lookup_mapping_in returned None for series_key: {}",
                        info.series_key
                    );
                    return Ok(vec![]);
                }
            };

        self.process_entry_inner(title, info, mapping_base, instance_id, entry, false)
            .await
    }
}
