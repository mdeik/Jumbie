use std::path::Path;

fn main() {
    // ── Declare custom cfg flags ──────────────────────────────────────────
    // Suppresses `unexpected_cfgs` warning for cfg(ffmpeg_installed) which
    // is set dynamically based on whether ffmpeg is on PATH.
    println!("cargo:rustc-check-cfg=cfg(ffmpeg_installed)");

    // ── macOS: embed Info.plist into binary section ───────────────────────
    // This makes the binary carry bundle metadata so tools like `codesign`
    // and `productbuild` can inspect it. The actual terminal-suppression
    // requires the binary to be inside a .app bundle (see macos-release.sh).
    //
    // The committed plist is a version-less template: the placeholders are
    // stamped with CARGO_PKG_VERSION here (single source of truth: Cargo.toml),
    // and the release workflow stamps the .app bundle's Contents/Info.plist
    // the same way at packaging time.
    #[cfg(all(target_os = "macos", feature = "desktop"))]
    {
        let plist_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("macos/Info.plist");
        println!("cargo:rerun-if-changed={}", plist_path.display());

        let template =
            std::fs::read_to_string(&plist_path).expect("failed to read macos/Info.plist template");
        let stamped = stamp_plist_version(&template, env!("CARGO_PKG_VERSION"));

        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
        let out_plist = Path::new(&out_dir).join("Info.plist");
        std::fs::write(&out_plist, stamped).expect("failed to write stamped Info.plist");
        println!(
            "cargo:rustc-link-arg-bin=jumbie=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            out_plist.display()
        );
    }

    // ── Windows: embed icon resource into the exe ─────────────────────────
    // Embeds windows/jumbie.ico (generated from frontend/icons/icon-512.png
    // by .github/actions/generate-installer-assets) so Explorer, shortcuts and
    // the taskbar show the real icon. Skipped when the .ico is absent so plain
    // dev builds still compile without running the generator first.
    #[cfg(target_os = "windows")]
    embed_windows_icon();

    // ── Build metadata (git commit + build date) ──────────────────────────────
    // `cargo:rustc-env` values are part of cargo's build fingerprint: whenever
    // a value changes between runs, the whole crate is recompiled. Emitting a
    // fresh wall-clock timestamp on every script run therefore turned any
    // script rerun (frontend edit, git commit, branch switch, PATH change, ...)
    // into a full rebuild of this ~50k-line crate — the main reason incremental
    // `cargo build` felt slow.
    //
    // So dynamic metadata is only embedded for release builds, where the Docker
    // image / published binary wants an accurate commit + timestamp. Dev/test
    // builds get fixed placeholders that never change, keeping incremental
    // rebuilds truly incremental.
    let is_release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);

    if is_release {
        // git commit hash
        let git_commit = std::env::var("SOURCE_VERSION")
            .ok()
            .or_else(|| {
                std::process::Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            String::from_utf8(o.stdout)
                                .ok()
                                .map(|s| s.trim().to_string())
                        } else {
                            None
                        }
                    })
            })
            .unwrap_or_else(|| "unknown".to_string());
        println!("cargo:rustc-env=JUMBIE_GIT_COMMIT={}", git_commit);

        // build date (ISO 8601)
        let build_date = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .map(|secs| {
                use chrono::{TimeZone, Utc};
                let dt = Utc.timestamp_opt(secs, 0).unwrap();
                dt.to_rfc3339()
            })
            .unwrap_or_else(|| {
                use chrono::Utc;
                Utc::now().to_rfc3339()
            });
        println!("cargo:rustc-env=JUMBIE_BUILD_DATE={}", build_date);

        // Re-run the script when HEAD moves — only matters for release builds,
        // where the embedded commit hash must track the source.
        println!("cargo:rerun-if-changed=../../.git/HEAD");
    } else {
        println!("cargo:rustc-env=JUMBIE_GIT_COMMIT=dev");
        println!("cargo:rustc-env=JUMBIE_BUILD_DATE=dev");
    }

    // ── Build & embed frontend ────────────────────────────────────────────────
    // `rust-embed` reads `#[folder = "../frontend/dist"]` at compile time,
    // so we must ensure the directory exists with content before compilation
    // proceeds. Build.rs runs before macro expansion, so this is the right spot.
    //
    // Strategy:
    //   1. If `../frontend/dist/` already has content → reuse it (re-build
    //      only if frontend sources changed — checked via rerun-if-changed).
    //   2. If `trunk` CLI is available → run `trunk build --release`.
    //   3. Otherwise → create a minimal placeholder so the binary still compiles.
    //      The user will see a warning and can either install trunk or use
    //      the API-only mode.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("backend should be in workspace root");
    let frontend_dir = workspace_root.join("frontend");
    let dist_dir = frontend_dir.join("dist");

    // Tell cargo to re-run if frontend sources change (only if they exist)
    let frontend_src = frontend_dir.join("src");
    if frontend_src.exists() {
        println!("cargo:rerun-if-changed={}", frontend_src.display());
    }
    let frontend_index = frontend_dir.join("index.html");
    if frontend_index.exists() {
        println!("cargo:rerun-if-changed={}", frontend_index.display());
    }
    let frontend_cargo = frontend_dir.join("Cargo.toml");
    if frontend_cargo.exists() {
        println!("cargo:rerun-if-changed={}", frontend_cargo.display());
    }
    println!("cargo:rerun-if-changed={}", dist_dir.display());

    // Check if the dist already exists and has content
    let dist_ready = dist_dir.exists()
        && dist_dir.join("index.html").exists()
        && dist_dir
            .join("index.html")
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false);

    if !dist_ready {
        // Try building with `trunk` CLI
        let trunk_check = std::process::Command::new("trunk")
            .arg("--version")
            .output();

        if let Ok(ref output) = trunk_check {
            if output.status.success() {
                eprintln!("build.rs: Building frontend with trunk...");
                // ── WHY remove API_BASE_URL ────────────────────────────────
                // When the frontend is embedded in the backend binary it is
                // always served from the same origin as the API.  Using
                // `window.location.origin` at runtime is correct.  Leaving
                // `API_BASE_URL` set would hardcode the developer's dev-server
                // port (typically 3000) into the WASM, breaking e2e tests
                // (which use port 3001) and any deployment on a non-default
                // port.  See also `detect_base_url` in `api_client.rs`.
                let status = std::process::Command::new("trunk")
                    .args(["build", "--release"])
                    .current_dir(&frontend_dir)
                    .env_remove("API_BASE_URL")
                    .status()
                    .expect("build.rs: failed to execute trunk build");

                if !status.success() {
                    panic!(
                        "build.rs: trunk build failed (exit code: {:?})",
                        status.code()
                    );
                }
                eprintln!("build.rs: Frontend build complete.");
            } else {
                create_placeholder_dist(&dist_dir, "trunk is installed but returned an error");
            }
        } else {
            create_placeholder_dist(
                &dist_dir,
                "trunk CLI not found — install trunk to build the frontend",
            );
        }
    }

    // Check that the dist now has at least index.html (rust-embed needs it)
    if !dist_dir.join("index.html").exists() {
        panic!(
            "build.rs: frontend dist directory does not contain index.html at {}",
            dist_dir.display()
        );
    }

    // ── FFmpeg detection ─────────────────────────────────────────────────────
    // Used by tests that need real media-info extraction via ffprobe.
    // When ffmpeg is not on PATH, those tests are marked `#[ignore]` so the
    // test runner shows them as "ignored" rather than silently passing or failing.
    let ffmpeg_ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ffmpeg_ok {
        println!("cargo:rustc-cfg=ffmpeg_installed");
    }

    // ── Re-run triggers ───────────────────────────────────────────────────────
    // Note: SOURCE_VERSION / SOURCE_DATE_EPOCH / PATH / frontend-dir triggers
    // rerun this script, but in dev builds the script now emits stable output
    // (see above), so a rerun is a cheap no-op instead of a full rebuild.
    println!("cargo:rerun-if-env-changed=SOURCE_VERSION");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=PATH");
}

/// Replace the version placeholders in the macOS Info.plist template.
///
/// Both `CFBundleShortVersionString` and `CFBundleVersion` use
/// `<string>0.0.0</string>` as the template placeholder; anything else in the
/// template is left untouched. The version value itself only ever comes from
/// Cargo.toml (`CARGO_PKG_VERSION`), so the plist never hardcodes it.
///
/// Only compiled (and used) on macOS desktop builds — see `main()`.
#[cfg(all(target_os = "macos", feature = "desktop"))]
fn stamp_plist_version(template: &str, version: &str) -> String {
    let placeholder = "<string>0.0.0</string>";
    let count = template.matches(placeholder).count();
    assert!(
        count == 2,
        "macos/Info.plist template must contain exactly 2 version placeholders \
         (CFBundleShortVersionString + CFBundleVersion), found {count}"
    );
    template.replace(placeholder, &format!("<string>{version}</string>"))
}

/// Embed windows/jumbie.ico as the exe icon (RT_ICON/RT_GROUP_ICON).
///
/// The .ico is derived at build time from `frontend/icons/icon-512.png` by the
/// `.github/actions/generate-installer-assets` action (CI) or
/// `scripts/generate-installer-assets.ps1` (local). The generated file is not
/// committed — the PNG is the single source of truth.
#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    let ico_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("windows/jumbie.ico");
    println!("cargo:rerun-if-changed={}", ico_src.display());

    if !ico_src.exists() {
        println!(
            "cargo:warning=windows/jumbie.ico not found — skipping exe icon embedding. \
             Run scripts/generate-installer-assets.ps1 or the generate-installer-assets CI action."
        );
        return;
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");

    // Copy the .ico next to the generated .rc so the resource compiler finds it
    // by its relative name.
    let ico_dst = Path::new(&out_dir).join("jumbie.ico");
    std::fs::copy(&ico_src, &ico_dst).expect("failed to copy jumbie.ico to OUT_DIR");

    let rc_path = Path::new(&out_dir).join("jumbie.rc");
    std::fs::write(&rc_path, windows_rc_source()).expect("failed to write jumbie.rc");

    // Compile & embed the .res into the exe; .manifest_optional() adds the
    // default application manifest (common controls v6) since jumbie.rc
    // defines none.
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .expect("failed to embed Windows icon resource");
}

/// The .rc source: exe icon + VERSIONINFO (shown in Explorer → Properties →
/// Details). Version comes from Cargo.toml at build-script compile time, so
/// the exe version can never drift from the manifest.
#[cfg(target_os = "windows")]
fn windows_rc_source() -> String {
    let (major, minor, patch, build) = windows_version_parts(env!("CARGO_PKG_VERSION"));

    format!(
        r#"MAINICON ICON "jumbie.ico"

1 VERSIONINFO
 FILEVERSION {major},{minor},{patch},{build}
 PRODUCTVERSION {major},{minor},{patch},{build}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Matthew Deik"
            VALUE "FileDescription", "Series organizer and media manager"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "jumbie"
            VALUE "LegalCopyright", "Copyright (C) Matthew Deik"
            VALUE "OriginalFilename", "Jumbie.exe"
            VALUE "ProductName", "Jumbie"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// Split "0.4.7[-suffix][+meta]" into up to four u16 segments (missing ones
/// default to 0), matching VERSIONINFO's fixed 4-field layout.
#[cfg(target_os = "windows")]
fn windows_version_parts(version: &str) -> (u16, u16, u16, u16) {
    let core = version
        .split('+')
        .next()
        .unwrap_or(version)
        .split('-')
        .next()
        .unwrap_or(version);
    let mut segments = core.split('.');
    let major = segments.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = segments.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = segments.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let build = segments.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch, build)
}

/// Create a minimal placeholder frontend so `rust-embed` has something to
/// embed and the binary compiles without the real frontend built.
fn create_placeholder_dist(dist_dir: &Path, reason: &str) {
    eprintln!("build.rs: WARNING — {}", reason);
    eprintln!("build.rs: Creating placeholder frontend. Rebuild after installing");
    eprintln!("build.rs: trunk to get the real UI.");

    std::fs::create_dir_all(dist_dir).expect("build.rs: failed to create dist directory");

    let placeholder = r#"<!DOCTYPE html>
<html>
<head><title>Jumbie — Frontend Not Built</title></head>
<body>
<h1>Frontend Not Available</h1>
<p>The frontend WASM app was not compiled into this binary.</p>
<p>Install <code>trunk</code> and rebuild to include the frontend.</p>
</body>
</html>"#;

    std::fs::write(dist_dir.join("index.html"), placeholder)
        .expect("build.rs: failed to write placeholder index.html");
}
