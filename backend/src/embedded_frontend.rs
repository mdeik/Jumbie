// Embeds the compiled frontend (`frontend/dist/`) into the binary via rust-embed,
// so a single binary serves both API and UI. There is no configurable disk
// override — frontend deployment happens exclusively through this embedding.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../frontend/dist"]
struct FrontendAssets;

/// Serve the embedded frontend assets.
///
/// Returns `None` if the path does not exist in the embedded assets and
/// there is no `index.html` fallback — the caller should return a 404.
pub fn serve(path: &str) -> Option<EmbeddedResponse> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() || path == "index.html" {
        "index.html"
    } else {
        path
    };

    if let Some(file) = FrontendAssets::get(path) {
        return Some(build_response(path, file));
    }

    // SPA fallback: unmatched paths serve index.html for the client-side router.
    if let Some(file) = FrontendAssets::get("index.html") {
        return Some(build_response("index.html", file));
    }

    None
}

pub fn is_available() -> bool {
    FrontendAssets::get("index.html").is_some()
}

pub struct EmbeddedResponse {
    pub body: Vec<u8>,
    pub mime: &'static str,
    pub cache_control: &'static str,
}

fn build_response(path: &str, file: rust_embed::EmbeddedFile) -> EmbeddedResponse {
    let mime = mime_type(path);
    let cache_control = cache_policy(path);
    EmbeddedResponse {
        body: file.data.to_vec(),
        mime,
        cache_control,
    }
}

/// Determine the MIME type from the file extension.
fn mime_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}

/// Content-hashed assets (e.g. `jumbie_frontend-<hash>.js`, `style-<hash>.css`)
/// get immutable long-lived caching; everything else uses `no-cache` so the
/// browser always revalidates.
fn cache_policy(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.contains("frontend-") || name.starts_with("style-") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}
