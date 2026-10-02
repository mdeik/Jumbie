// Source plugin modules — each implements `PluginInstance` directly.
//
// - `nyaa`: specialized source for Nyaa.si (anime torrents) with category filtering
// - `rss`: full RSS 2.0 parser supporting common torrent feed extensions
// - `generic`: a minimal placeholder source for documentation/testing

pub mod generic;
pub mod nyaa;
pub mod rss;
