// Registration: every internal plugin self-registers via `register_full` on the
// InternalPluginRegistry. Each plugin's `register()` lives in the plugin's own file
// so the factory, schema, and info are defined together.

pub async fn register_all() {
    crate::plugins::downloaders::qbittorrent::QBittorrentClient::register().await;
    crate::plugins::sources::nyaa::NyaaSource::register().await;
    crate::plugins::sources::rss::BasicRssSource::register().await;
    crate::plugins::notifiers::discord::DiscordNotifier::register().await;
    crate::plugins::metadata::tvmaze::TvMazePlugin::register().await;
    crate::plugins::metadata::tvdb::TvDbPlugin::register().await;
}
