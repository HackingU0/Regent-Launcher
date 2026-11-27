#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use percent_encoding::percent_decode_str;
use regent_launcher::app::{
    AppError, AppResult, At, Backend, CrashListResponse, CrashReport, FieldEdit, GameArt,
    GameStateDto, GameView, GamesResponse, LaunchInfo, OpenTarget, RawEdit, RefreshEvent,
    SavePlayOutcome, SavesResponse, SettingsApplyResponse, SettingsView, ShareExportResponse,
    ShareImportResponse, ShareInspectResponse, Summary,
};
use regent_launcher::game::Roots;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tauri::{Emitter, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;

type SharedBackend = Arc<Backend>;

async fn run_blocking<T, F>(backend: State<'_, SharedBackend>, f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(SharedBackend) -> AppResult<T> + Send + 'static,
{
    let b = Arc::clone(backend.inner());
    tauri::async_runtime::spawn_blocking(move || f(b))
        .await
        .map_err(|e| AppError::failed(format!("worker task failed: {e}")))?
}

#[tauri::command]
async fn games(backend: State<'_, SharedBackend>) -> AppResult<GamesResponse> {
    run_blocking(backend, |b| b.games()).await
}

#[tauri::command]
async fn load(
    backend: State<'_, SharedBackend>,
    game: String,
    rescan: bool,
) -> AppResult<GameView> {
    run_blocking(backend, move |b| b.load(&game, rescan)).await
}

#[tauri::command]
async fn summary(backend: State<'_, SharedBackend>, game: String) -> AppResult<Summary> {
    run_blocking(backend, move |b| b.summary(&game)).await
}

#[tauri::command]
async fn conflict_files(
    backend: State<'_, SharedBackend>,
    game: String,
    winner: String,
    loser: String,
) -> AppResult<Vec<String>> {
    run_blocking(backend, move |b| b.conflict_files(&game, &winner, &loser)).await
}

#[tauri::command]
async fn set_mods_enabled(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    ids: Vec<String>,
    on: bool,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.set_mods_enabled(&game, &at, &ids, on)).await
}

#[tauri::command]
async fn move_mod(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    id: String,
    before_id: Option<String>,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| {
        b.move_mod(&game, &at, &id, before_id.as_deref())
    })
    .await
}

#[tauri::command]
async fn apply_order(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.apply_order(&game, &at)).await
}

#[tauri::command]
async fn set_dlcs_enabled(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    ids: Vec<String>,
    on: bool,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.set_dlcs_enabled(&game, &at, &ids, on)).await
}

#[tauri::command]
async fn playset_new(
    backend: State<'_, SharedBackend>,
    game: String,
    name: String,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.playset_new(&game, &name)).await
}

#[tauri::command]
async fn playset_copy(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    name: Option<String>,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| {
        b.playset_copy(&game, &at, name.as_deref())
    })
    .await
}

#[tauri::command]
async fn playset_rename(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    name: String,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.playset_rename(&game, &at, &name)).await
}

#[tauri::command]
async fn playset_use(
    backend: State<'_, SharedBackend>,
    game: String,
    index: usize,
    name: String,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.playset_use(&game, index, &name)).await
}

#[tauri::command]
async fn set_launch_args(
    backend: State<'_, SharedBackend>,
    game: String,
    args: String,
) -> AppResult<GameStateDto> {
    run_blocking(backend, move |b| b.set_launch_args(&game, &args)).await
}

#[tauri::command]
async fn launch(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    extra: Vec<String>,
) -> AppResult<LaunchInfo> {
    run_blocking(backend, move |b| b.launch(&game, &at, &extra)).await
}

#[tauri::command]
async fn settings_get(backend: State<'_, SharedBackend>, game: String) -> AppResult<SettingsView> {
    run_blocking(backend, move |b| b.settings_get(&game)).await
}

#[tauri::command]
async fn settings_apply(
    backend: State<'_, SharedBackend>,
    game: String,
    rev: String,
    edits: Vec<FieldEdit>,
    raw: Vec<RawEdit>,
) -> AppResult<SettingsApplyResponse> {
    run_blocking(backend, move |b| {
        b.settings_apply(&game, &rev, &edits, &raw)
    })
    .await
}

#[tauri::command]
async fn settings_rollback(
    backend: State<'_, SharedBackend>,
    game: String,
    id: u64,
) -> AppResult<SettingsView> {
    run_blocking(backend, move |b| b.settings_rollback(&game, id)).await
}

#[tauri::command]
async fn saves_list(backend: State<'_, SharedBackend>, game: String) -> AppResult<SavesResponse> {
    run_blocking(backend, move |b| b.saves_list(&game)).await
}

#[tauri::command]
async fn saves_bind(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    rel: String,
) -> AppResult<SavesResponse> {
    run_blocking(backend, move |b| b.saves_bind(&game, &at, &rel)).await
}

#[tauri::command]
async fn saves_play(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
    rel: String,
    force: bool,
    expect: Option<String>,
) -> AppResult<SavePlayOutcome> {
    run_blocking(backend, move |b| {
        b.saves_play(&game, &at, &rel, force, expect.as_deref())
    })
    .await
}

#[tauri::command]
async fn crash_list(
    backend: State<'_, SharedBackend>,
    game: String,
) -> AppResult<CrashListResponse> {
    run_blocking(backend, move |b| b.crash_list(&game)).await
}

#[tauri::command]
async fn crash_diagnose(
    backend: State<'_, SharedBackend>,
    game: String,
    name: Option<String>,
) -> AppResult<CrashReport> {
    run_blocking(backend, move |b| b.crash_diagnose(&game, name.as_deref())).await
}

#[tauri::command]
async fn share_export(
    backend: State<'_, SharedBackend>,
    game: String,
    at: At,
) -> AppResult<ShareExportResponse> {
    run_blocking(backend, move |b| b.share_export(&game, &at)).await
}

#[tauri::command]
async fn share_inspect(
    backend: State<'_, SharedBackend>,
    game: String,
    text: String,
) -> AppResult<ShareInspectResponse> {
    run_blocking(backend, move |b| b.share_inspect(&game, &text)).await
}

#[tauri::command]
async fn share_import(
    backend: State<'_, SharedBackend>,
    game: String,
    text: String,
) -> AppResult<ShareImportResponse> {
    run_blocking(backend, move |b| b.share_import(&game, &text)).await
}

#[tauri::command]
async fn open(backend: State<'_, SharedBackend>, target: OpenTarget) -> AppResult<()> {
    run_blocking(backend, move |b| b.open(target)).await
}

#[tauri::command]
async fn wallpaper_set_url(
    backend: State<'_, SharedBackend>,
    game: String,
    url: String,
) -> AppResult<GameArt> {
    run_blocking(backend, move |b| b.wallpaper_set_url(&game, &url)).await
}

/// None when the user cancels the picker.
#[tauri::command]
async fn wallpaper_pick_file(
    window: tauri::Window,
    backend: State<'_, SharedBackend>,
    game: String,
) -> AppResult<Option<GameArt>> {
    run_blocking(backend, move |b| {
        let picked = window
            .dialog()
            .file()
            .set_parent(&window)
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .blocking_pick_file();
        let Some(file) = picked else {
            return Ok(None);
        };
        let path = file
            .into_path()
            .map_err(|e| AppError::failed(format!("Could not read that file: {e}")))?;
        b.wallpaper_set_file(&game, &path).map(Some)
    })
    .await
}

#[tauri::command]
async fn wallpaper_reset(backend: State<'_, SharedBackend>, game: String) -> AppResult<GameArt> {
    run_blocking(backend, move |b| b.wallpaper_reset(&game)).await
}

// ---------------------------------------------------------------- thumb:// protocol

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

fn not_found() -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(404)
        .header("Content-Type", "text/plain")
        .header("Cache-Control", "no-cache")
        .header("X-Content-Type-Options", "nosniff")
        .body(Vec::new())
        .expect("static 404 response")
}

// ponytail: no resized thumbnail cache; add one if scrolling 1000 rows janks.
fn thumb_response(backend: &Backend, raw_path: &str) -> tauri::http::Response<Vec<u8>> {
    let Ok(decoded) = percent_decode_str(raw_path).decode_utf8() else {
        return not_found();
    };
    let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
    let mut parts = trimmed.splitn(3, '/');
    let (Some(game), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        return not_found();
    };
    if game.is_empty() || kind.is_empty() || id.is_empty() {
        return not_found();
    }
    let Some((path, bytes)) = backend.read_thumb(game, kind, id) else {
        return not_found();
    };
    let mut response = tauri::http::Response::builder()
        .status(200)
        .header("X-Content-Type-Options", "nosniff")
        // Art changes when the user picks a wallpaper; mod and DLC images rarely do.
        .header(
            "Cache-Control",
            if kind == "art" {
                "no-cache"
            } else {
                "max-age=3600"
            },
        );
    let svg_logo = kind == "art"
        && id == "logo"
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    response = if svg_logo {
        // An SVG is a document: no scripts, no external loads, only its own inline styles.
        response.header("Content-Type", "image/svg+xml").header(
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'",
        )
    } else {
        response.header("Content-Type", content_type(&path))
    };
    response.body(bytes).unwrap_or_else(|_| not_found())
}

fn main() {
    // Fixture mode is a dev tool; release builds always use the real roots.
    let fixture_root =
        std::env::var_os("REGENT_ROOT").filter(|v| cfg!(debug_assertions) && !v.is_empty());
    let (roots, fixture) = match fixture_root {
        Some(v) => (Roots::under(Path::new(&v)), true),
        None => (Roots::system(), false),
    };

    let handle_slot: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let emit_slot = Arc::clone(&handle_slot);
    let backend: SharedBackend = Arc::new(Backend::with_emitter(
        roots,
        fixture,
        Arc::new(move |ev| {
            if let Some(h) = emit_slot.get() {
                let _ = h.emit("regent:refresh", ev);
            }
        }),
    ));
    let thumb_backend = Arc::clone(&backend);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(backend)
        .setup(move |app| {
            let _ = handle_slot.set(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event {
                let _ = window.emit("regent:refresh", RefreshEvent::focus());
            }
        })
        .register_asynchronous_uri_scheme_protocol("thumb", move |_ctx, request, responder| {
            let b = Arc::clone(&thumb_backend);
            tauri::async_runtime::spawn_blocking(move || {
                let resp = thumb_response(&b, request.uri().path());
                responder.respond(resp);
            });
        })
        .invoke_handler(tauri::generate_handler![
            games,
            load,
            summary,
            conflict_files,
            set_mods_enabled,
            move_mod,
            apply_order,
            set_dlcs_enabled,
            playset_new,
            playset_copy,
            playset_rename,
            playset_use,
            set_launch_args,
            launch,
            settings_get,
            settings_apply,
            settings_rollback,
            saves_list,
            saves_bind,
            saves_play,
            crash_list,
            crash_diagnose,
            share_export,
            share_inspect,
            share_import,
            open,
            wallpaper_set_url,
            wallpaper_pick_file,
            wallpaper_reset,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Regent Launcher");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "regent-app-test-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn put(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn svg_is_served_only_as_the_art_logo_and_headers_differ_by_kind() {
        let base = tmp_dir("thumb-svg");
        let g = base.join("steam/steamapps/common/Crusader Kings III");
        put(
            &g.join("launcher/launcher-settings.json"),
            br#"{"exePath":"../binaries/ck3","dlcPath":"../game"}"#,
        );
        put(
            &g.join("launcher/assets/theme-settings.json"),
            br#"{"logo":{"imagePath":"logo.svg"},"background":{"backgroundImage":"bg.svg"}}"#,
        );
        put(&g.join("launcher/assets/logo.svg"), b"<svg/>");
        put(&g.join("launcher/assets/bg.svg"), b"<svg/>");
        put(
            &g.join("game/dlc/d/d.dlc"),
            b"name=\"D\"\nthumbnail=\"t.svg\"\n",
        );
        put(&g.join("game/dlc/d/t.svg"), b"<svg/>");
        let backend = Backend::new(Roots::under(&base), true);
        backend.load("ck3", false).unwrap();

        let logo = thumb_response(&backend, "/ck3/art/logo");
        assert_eq!(logo.status(), 200);
        assert_eq!(logo.headers()["Content-Type"], "image/svg+xml");
        assert_eq!(
            logo.headers()["Content-Security-Policy"],
            "default-src 'none'; style-src 'unsafe-inline'"
        );
        assert_eq!(logo.headers()["Cache-Control"], "no-cache");
        assert_eq!(logo.headers()["X-Content-Type-Options"], "nosniff");

        for (path, cache) in [
            ("/ck3/art/background", "no-cache"),
            ("/ck3/art/wallpaper", "no-cache"),
            ("/ck3/dlc/dlc%2Fd%2Fd.dlc", "max-age=3600"),
        ] {
            let r = thumb_response(&backend, path);
            assert_eq!(r.status(), 200, "{path}");
            assert_eq!(r.headers()["Content-Type"], "application/octet-stream");
            assert_eq!(r.headers()["Cache-Control"], cache, "{path}");
            assert_eq!(r.headers()["X-Content-Type-Options"], "nosniff");
        }
    }

    #[test]
    fn thumb_decodes_encoded_ids_serves_art_and_404s_unknown_and_traversal() {
        let base = tmp_dir("thumb");
        let g = base.join("steam/steamapps/common/Stellaris");
        put(
            &g.join("launcher-settings.json"),
            br#"{"gameId":"stellaris","rawVersion":"v4.6.0","exePath":"/usr/bin/true"}"#,
        );
        put(&g.join("assets/app-background.png"), b"BG_PNG");
        put(&g.join("assets/game-logo.png"), b"LOGO_PNG");
        put(
            &g.join("dlc/dlc014_utopia/dlc014.dlc"),
            b"name=\"Utopia\"\narchive=\"dlc/dlc014_utopia/dlc014.zip\"\nthumbnail=\"thumbnail.png\"\n",
        );
        put(&g.join("dlc/dlc014_utopia/dlc014.zip"), b"z");
        put(&g.join("dlc/dlc014_utopia/thumbnail.png"), b"DLC_PNG");

        let ws = base.join("steam/steamapps/workshop/content/281990/1623423360");
        put(
            &ws.join("descriptor.mod"),
            b"name=\"Workshop Mod\"\npicture=\"cover.jpg\"\n",
        );
        put(&ws.join("cover.jpg"), b"MOD_JPG");
        put(&base.join("secret.png"), b"SECRET");

        let backend = Backend::new(Roots::under(&base), true);
        backend.load("stellaris", false).unwrap();

        // Percent-encoded slash in workshop mod id
        let r = thumb_response(&backend, "/stellaris/mod/mod%2Fugc_1623423360.mod");
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), b"MOD_JPG");
        assert_eq!(r.headers()["Content-Type"], "image/jpeg");
        assert_eq!(r.headers()["Cache-Control"], "max-age=3600");

        // Percent-encoded slashes in DLC id (what convertFileSrc produces for the whole path)
        let r = thumb_response(
            &backend,
            "/stellaris%2Fdlc%2Fdlc%2Fdlc014_utopia%2Fdlc014.dlc",
        );
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), b"DLC_PNG");
        assert_eq!(r.headers()["Content-Type"], "image/png");

        // Game art background & logo
        let bg = thumb_response(&backend, "/stellaris/art/background");
        assert_eq!(
            (bg.status().as_u16(), bg.body().as_slice()),
            (200, b"BG_PNG".as_slice())
        );
        let logo = thumb_response(&backend, "/stellaris/art/logo");
        assert_eq!(
            (logo.status().as_u16(), logo.body().as_slice()),
            (200, b"LOGO_PNG".as_slice())
        );

        // Unknown keys, traversal, and invalid kinds -> 404
        for bad in [
            "/stellaris/mod/mod%2Fnope.mod",
            "/stellaris/mod/..",
            "/stellaris/mod/%2e%2e%2Fsecret.png",
            "/stellaris/art/..%2Fsecret.png",
            "/stellaris/art/other",
            "/stellaris/other/background",
            "/unknown/art/background",
            "/only-two/parts",
            "/",
        ] {
            assert_eq!(thumb_response(&backend, bad).status(), 404, "{bad}");
        }

        // R7: Post-scan symlink swap and symlinked art/thumbs must return 404 at open time.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            // Swap valid mod thumbnail with a symlink pointing outside the mod dir.
            std::fs::remove_file(ws.join("cover.jpg")).unwrap();
            symlink(base.join("secret.png"), ws.join("cover.jpg")).unwrap();
            assert_eq!(
                thumb_response(&backend, "/stellaris/mod/mod%2Fugc_1623423360.mod").status(),
                404,
                "post-scan swapped mod thumb symlink must 404"
            );

            // Swap game art logo with a symlink pointing outside install dir.
            std::fs::remove_file(g.join("assets/game-logo.png")).unwrap();
            symlink(base.join("secret.png"), g.join("assets/game-logo.png")).unwrap();
            assert_eq!(
                thumb_response(&backend, "/stellaris/art/logo").status(),
                404,
                "symlinked game art must 404"
            );

            // Swap DLC thumbnail with a symlink pointing outside DLC dir.
            std::fs::remove_file(g.join("dlc/dlc014_utopia/thumbnail.png")).unwrap();
            symlink(
                base.join("secret.png"),
                g.join("dlc/dlc014_utopia/thumbnail.png"),
            )
            .unwrap();
            assert_eq!(
                thumb_response(
                    &backend,
                    "/stellaris%2Fdlc%2Fdlc%2Fdlc014_utopia%2Fdlc014.dlc",
                )
                .status(),
                404,
                "symlinked DLC thumb must 404"
            );
        }
    }
}
