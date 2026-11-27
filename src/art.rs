//! Game art: official theme logo/background, cached wallpapers and Steam previews, and every
//! network fetch. UI-free.
use crate::game::Install;
use crate::mods;
use crate::playset::stage_atomic_bytes;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig};

/// Cap for any served or downloaded image.
pub(crate) const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_API_BYTES: u64 = 8 * 1024 * 1024;
const MAX_THEME_BYTES: u64 = 1024 * 1024;
const EXTS: [&str; 3] = ["png", "jpg", "webp"];

/// Image type from magic bytes; the only types we ever cache.
fn image_ext(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else {
        None
    }
}

// ---------------------------------------------------------------- cache

/// `<dir>/<stem>.<ext>` for whichever image extension exists inside `app_dir`.
fn cached(app_dir: &Path, dir: &Path, stem: &str) -> Option<PathBuf> {
    EXTS.iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| mods::contained_file(app_dir, p).is_some())
}

/// Cache writes stay inside `app_dir`: create `dir`, then refuse if any directory from `app_dir`
/// down to it is a symlink (a planted link could otherwise point writes or deletes at game files).
fn real_cache_dir(app_dir: &Path, dir: &Path) -> std::io::Result<()> {
    let rel = dir
        .strip_prefix(app_dir)
        .map_err(|_| std::io::Error::other("cache folder is outside the app data folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut p = app_dir.to_path_buf();
    for c in rel.components() {
        p.push(c);
        if std::fs::symlink_metadata(&p)?.file_type().is_symlink() {
            return Err(std::io::Error::other("cache folder is a symlink"));
        }
    }
    Ok(())
}

fn remove_image(app_dir: &Path, dir: &Path, stem: &str, keep: Option<&str>) -> std::io::Result<()> {
    real_cache_dir(app_dir, dir)?;
    for ext in EXTS.iter().filter(|e| Some(**e) != keep) {
        match std::fs::remove_file(dir.join(format!("{stem}.{ext}"))) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Validate by magic bytes, then replace `<dir>/<stem>.*`. The new file is renamed into place
/// before the other extensions are deleted, so a failure never leaves the slot empty.
fn store_image(app_dir: &Path, dir: &Path, stem: &str, bytes: &[u8]) -> Result<(), String> {
    let ext = image_ext(bytes).ok_or("That is not a PNG, JPEG or WebP image")?;
    let target = dir.join(format!("{stem}.{ext}"));
    let save = || -> anyhow::Result<()> {
        real_cache_dir(app_dir, dir)?;
        let tmp = stage_atomic_bytes(&target, bytes, None)?;
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(remove_image(app_dir, dir, stem, Some(ext))?)
    };
    save().map_err(|e| format!("Could not save the image: {e:#}"))
}

// ---------------------------------------------------------------- theme art

/// Logo or background named by the install's theme-settings.json (relative to that file),
/// else the conventional file name next to it. Always inside `launcher_dir`, no symlinks.
fn theme_art(install: &Install, section: &str, key: &str, fallback: &str) -> Option<PathBuf> {
    let root = &install.launcher_dir;
    let dir = install.theme_file.parent()?;
    let declared =
        mods::read_contained_file(root, &install.theme_file, MAX_THEME_BYTES).and_then(|bytes| {
            let json = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
            let v: Value = serde_json::from_slice(json).ok()?;
            v[section][key].as_str().map(str::to_owned)
        });
    declared
        .as_deref()
        .into_iter()
        .chain([fallback])
        .map(|name| dir.join(name))
        .find(|p| mods::contained_file(root, p).is_some())
}

// ---------------------------------------------------------------- wallpapers

fn wallpaper_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("wallpapers")
}

// Wallpaper and preview keys come from `install.def.id`, never from a raw request string.
fn wallpaper_stem(install: &Install, slot: &str) -> String {
    format!("{}.{slot}", install.def.id)
}

pub(crate) fn custom_wallpaper(app_dir: &Path, install: &Install) -> Option<PathBuf> {
    cached(
        app_dir,
        &wallpaper_dir(app_dir),
        &wallpaper_stem(install, "custom"),
    )
}

pub(crate) fn default_wallpaper(app_dir: &Path, install: &Install) -> Option<PathBuf> {
    cached(
        app_dir,
        &wallpaper_dir(app_dir),
        &wallpaper_stem(install, "default"),
    )
}

pub(crate) fn set_custom_wallpaper(
    app_dir: &Path,
    install: &Install,
    bytes: &[u8],
) -> Result<(), String> {
    store_image(
        app_dir,
        &wallpaper_dir(app_dir),
        &wallpaper_stem(install, "custom"),
        bytes,
    )
}

pub(crate) fn reset_custom_wallpaper(app_dir: &Path, install: &Install) -> std::io::Result<()> {
    remove_image(
        app_dir,
        &wallpaper_dir(app_dir),
        &wallpaper_stem(install, "custom"),
        None,
    )
}

/// Read a user-picked file, refusing anything that is not a regular file within the size cap.
pub(crate) fn read_picked_file(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let unreadable = |e: std::io::Error| format!("Could not read that file: {e}");
    // Checked before open: opening a FIFO would block.
    if !std::fs::metadata(path).map_err(unreadable)?.is_file() {
        return Err("That is not a file".into());
    }
    let mut bytes = vec![];
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("Image is larger than 32 MB".into());
    }
    Ok(bytes)
}

/// `(containment root, path)` behind `thumb://<game>/art/<which>`.
pub(crate) fn target(app_dir: &Path, install: &Install, which: &str) -> Option<(PathBuf, PathBuf)> {
    let theme = |path: Option<PathBuf>| Some((install.launcher_dir.clone(), path?));
    let own = |path: PathBuf| (app_dir.to_path_buf(), path);
    match which {
        "logo" => theme(theme_art(install, "logo", "imagePath", "game-logo.png")),
        "background" => theme(theme_art(
            install,
            "background",
            "backgroundImage",
            "app-background.png",
        )),
        "wallpaper" => custom_wallpaper(app_dir, install)
            .or_else(|| default_wallpaper(app_dir, install))
            .map(own)
            .or_else(|| self::target(app_dir, install, "background")),
        _ => None,
    }
}

// ---------------------------------------------------------------- Steam workshop previews

/// Ids come from descriptor text; only plain numbers may become file names or request fields.
pub(crate) fn is_workshop_id(id: &str) -> bool {
    (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

fn preview_dir(app_dir: &Path, game: &str) -> PathBuf {
    app_dir.join("mod-thumbs").join(game)
}

pub(crate) fn steam_preview(app_dir: &Path, install: &Install, remote_id: &str) -> Option<PathBuf> {
    if !is_workshop_id(remote_id) {
        return None;
    }
    cached(app_dir, &preview_dir(app_dir, install.def.id), remote_id)
}

/// `(id, preview url)` pairs from a GetPublishedFileDetails response. Only ids we asked for
/// and https urls are kept.
fn parse_previews(json: &[u8], asked: &[String]) -> Vec<(String, String)> {
    let v: Value = serde_json::from_slice(json).unwrap_or_default();
    v["response"]["publishedfiledetails"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let id = d["publishedfileid"].as_str()?;
            let url = d["preview_url"].as_str()?;
            (asked.iter().any(|a| a == id) && url.starts_with("https://"))
                .then(|| (id.to_owned(), url.to_owned()))
        })
        .collect()
}

/// Download previews for `ids` into the cache, 100 per API call. Failures are silent;
/// `saved` runs after each batch that wrote at least one file.
pub(crate) fn fetch_previews(app_dir: &Path, game: &str, ids: &[String], saved: impl Fn()) {
    const API: &str =
        "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
    let (agent, dir) = (agent(), preview_dir(app_dir, game));
    for batch in ids.chunks(100) {
        let mut form = vec![("itemcount".to_string(), batch.len().to_string())];
        form.extend(
            batch
                .iter()
                .enumerate()
                .map(|(i, id)| (format!("publishedfileids[{i}]"), id.clone())),
        );
        let Ok(json) = agent.post(API).send_form(form).and_then(|mut r| {
            r.body_mut()
                .with_config()
                .limit(MAX_API_BYTES)
                .read_to_vec()
        }) else {
            continue;
        };
        let mut wrote = false;
        for (id, url) in parse_previews(&json, batch) {
            if let Ok(bytes) = get(&agent, &url, MAX_PREVIEW_BYTES) {
                wrote |= store_image(app_dir, &dir, &id, &bytes).is_ok();
            }
        }
        if wrote {
            saved();
        }
    }
}

// ---------------------------------------------------------------- HTTP

/// https only (redirects included), OS trust store, short timeout.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(3)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent("RegentLauncher")
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, ureq::Error> {
    let mut response = agent.get(url).call()?;
    response.body_mut().with_config().limit(limit).read_to_vec()
}

#[derive(Debug, PartialEq)]
pub(crate) enum Source {
    /// A wallhaven wallpaper id, resolved through its API.
    Wallhaven(String),
    Direct(String),
}

fn is_wallhaven_id(s: &str) -> bool {
    s.len() == 6
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A bare wallhaven id, a `https://wallhaven.cc/w/<id>` page link, or any other https url.
pub(crate) fn parse_wallpaper_url(input: &str) -> Result<Source, &'static str> {
    let url = input.trim();
    if is_wallhaven_id(url) {
        return Ok(Source::Wallhaven(url.into()));
    }
    let rest = url
        .split_at_checked(8)
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("https://"))
        .map(|(_, rest)| rest)
        .ok_or("Only https links are supported")?;
    let (host, path) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    if host.is_empty() {
        return Err("That is not a valid link");
    }
    let wallhaven = ["wallhaven.cc", "www.wallhaven.cc"]
        .iter()
        .any(|h| host.eq_ignore_ascii_case(h));
    let path = path.split(['?', '#']).next().unwrap_or_default();
    match path.strip_prefix("/w/").filter(|_| wallhaven) {
        Some(id) => {
            let id = id.trim_end_matches('/');
            is_wallhaven_id(id)
                .then(|| Source::Wallhaven(id.into()))
                .ok_or("That is not a valid Wallhaven wallpaper link")
        }
        None => Ok(Source::Direct(url.into())),
    }
}

/// `data.path` of a wallhaven `/api/v1/w/<id>` response, if it is https.
fn wallhaven_image_url(json: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(json).ok()?;
    let path = v["data"]["path"].as_str()?;
    path.starts_with("https://").then(|| path.to_owned())
}

/// Image bytes for a wallpaper source; the error is a short message for the user.
pub(crate) fn download_wallpaper(source: &Source) -> Result<Vec<u8>, String> {
    let message = |e: ureq::Error| match e {
        ureq::Error::StatusCode(code) => format!("Download failed (HTTP {code})"),
        ureq::Error::BodyExceedsLimit(_) => "Image is larger than 32 MB".into(),
        _ => "Could not download the image. Check the link and your connection.".into(),
    };
    let agent = agent();
    let url = match source {
        Source::Direct(url) => url.clone(),
        Source::Wallhaven(id) => {
            let api = format!("https://wallhaven.cc/api/v1/w/{id}");
            let json = get(&agent, &api, MAX_API_BYTES).map_err(message)?;
            wallhaven_image_url(&json).ok_or("Wallhaven did not return an image link")?
        }
    };
    get(&agent, &url, MAX_IMAGE_BYTES).map_err(message)
}

/// Download and cache a game's curated wallpaper. False on any failure.
pub(crate) fn fetch_default_wallpaper(app_dir: &Path, install: &Install) -> bool {
    let Some(url) = install.def.wallpaper else {
        return false;
    };
    download_wallpaper(&Source::Direct(url.into())).is_ok_and(|bytes| {
        store_image(
            app_dir,
            &wallpaper_dir(app_dir),
            &wallpaper_stem(install, "default"),
            &bytes,
        )
        .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{self, Roots, put, tmp_dir};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    const JPG: &[u8] = b"\xff\xd8\xff\xe0rest";
    const WEBP: &[u8] = b"RIFF\x00\x00\x00\x00WEBPrest";

    fn install(tag: &str) -> (PathBuf, Install) {
        let b = tmp_dir(tag);
        put(
            &b.join("steam/steamapps/common/Crusader Kings III/launcher/launcher-settings.json"),
            r#"{"exePath":"../binaries/ck3","dlcPath":"../game","themeFile":"./assets/theme-settings.json"}"#,
        );
        let i = game::discover(&Roots::under(&b)).remove(0);
        (b, i)
    }

    #[test]
    fn theme_art_follows_theme_file_then_falls_back_and_stays_contained() {
        let (b, i) = install("art-theme");
        let app = b.join("app");
        let assets = i.launcher_dir.join("assets");
        let path = |which| target(&app, &i, which).map(|(_, p)| p);
        assert_eq!(path("logo"), None);
        put(&assets.join("game-logo.png"), "fallback logo");
        put(&assets.join("app-background.png"), "fallback bg");
        assert_eq!(path("logo"), Some(assets.join("game-logo.png")));

        put(
            &i.theme_file,
            "\u{feff}{\"logo\":{\"imagePath\":\"logo.svg\"},\"background\":{\"backgroundImage\":\"bg.png\"}}",
        );
        // declared but missing -> conventional name
        assert_eq!(path("logo"), Some(assets.join("game-logo.png")));
        put(&assets.join("logo.svg"), "<svg/>");
        put(&assets.join("bg.png"), "bg");
        assert_eq!(path("logo"), Some(assets.join("logo.svg")));
        assert_eq!(path("background"), Some(assets.join("bg.png")));
        assert_eq!(path("wallpaper"), Some(assets.join("bg.png")));

        // a declared name that escapes is ignored
        put(&b.join("secret.png"), "secret");
        for evil in [
            "../../../../../../secret.png".to_string(),
            b.join("secret.png").display().to_string(),
        ] {
            put(
                &i.theme_file,
                &serde_json::json!({"logo": {"imagePath": evil}}).to_string(),
            );
            assert_eq!(path("logo"), Some(assets.join("game-logo.png")), "{evil}");
        }
        put(&i.theme_file, "not json");
        assert_eq!(path("logo"), Some(assets.join("game-logo.png")));
        assert_eq!(path("other"), None);
    }

    #[test]
    fn wallpaper_precedence_and_one_file_per_slot() {
        let (b, i) = install("art-wall");
        let app = b.join("app");
        let dir = wallpaper_dir(&app);
        let current = || target(&app, &i, "wallpaper").map(|(_, p)| p);
        assert_eq!(current(), None);
        store_image(&app, &dir, "ck3.default", PNG).unwrap();
        assert_eq!(current(), Some(dir.join("ck3.default.png")));

        set_custom_wallpaper(&app, &i, JPG).unwrap();
        assert_eq!(current(), Some(dir.join("ck3.custom.jpg")));
        set_custom_wallpaper(&app, &i, WEBP).unwrap();
        assert_eq!(current(), Some(dir.join("ck3.custom.webp")));
        assert!(!dir.join("ck3.custom.jpg").exists());

        // rejected bytes keep the previous wallpaper
        for bad in [&b"<svg onload=x>"[..], b"GIF89a", b"RIFF\0\0\0\0WAVE", b""] {
            assert!(set_custom_wallpaper(&app, &i, bad).is_err());
        }
        assert_eq!(std::fs::read(current().unwrap()).unwrap(), WEBP);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

        reset_custom_wallpaper(&app, &i).unwrap();
        reset_custom_wallpaper(&app, &i).unwrap();
        assert_eq!(current(), Some(dir.join("ck3.default.png")));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_folder_is_never_written_or_cleared() {
        let (b, i) = install("art-symlink");
        let app = b.join("app");
        let game_files = b.join("game-files");
        std::fs::create_dir_all(&game_files).unwrap();
        std::fs::write(game_files.join("ck3.custom.png"), PNG).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::os::unix::fs::symlink(&game_files, wallpaper_dir(&app)).unwrap();
        assert!(set_custom_wallpaper(&app, &i, JPG).is_err());
        assert!(reset_custom_wallpaper(&app, &i).is_err());
        assert_eq!(std::fs::read_dir(&game_files).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(game_files.join("ck3.custom.png")).unwrap(),
            PNG
        );
    }

    #[test]
    fn magic_bytes() {
        assert_eq!(
            [PNG, JPG, WEBP].map(image_ext),
            [Some("png"), Some("jpg"), Some("webp")]
        );
        assert_eq!(image_ext(b"RIFF"), None);
        assert_eq!(image_ext(b"<html>"), None);
    }

    #[test]
    fn picked_file_is_capped_and_must_be_a_file() {
        let b = tmp_dir("art-pick");
        put(&b.join("ok"), "x");
        assert_eq!(read_picked_file(&b.join("ok")).unwrap(), b"x");
        assert!(read_picked_file(&b).is_err());
        let big = std::fs::File::create(b.join("big")).unwrap();
        big.set_len(MAX_IMAGE_BYTES + 1).unwrap();
        assert_eq!(
            read_picked_file(&b.join("big")).unwrap_err(),
            "Image is larger than 32 MB"
        );
    }

    #[test]
    fn wallpaper_url_forms() {
        let id = |s: &str| Ok(Source::Wallhaven(s.into()));
        for ok in [
            "o3qrmm",
            " https://wallhaven.cc/w/o3qrmm ",
            "https://wallhaven.cc/w/o3qrmm/",
            "HTTPS://www.Wallhaven.cc/w/o3qrmm?ref=x#top",
        ] {
            assert_eq!(parse_wallpaper_url(ok), id("o3qrmm"), "{ok}");
        }
        for direct in [
            "https://w.wallhaven.cc/full/o3/wallhaven-o3qrmm.jpg",
            "https://example.com/w/o3qrmm",
            "https://wallhaven.cc.evil.test/w/o3qrmm",
        ] {
            assert_eq!(
                parse_wallpaper_url(direct),
                Ok(Source::Direct(direct.into())),
                "{direct}"
            );
        }
        for bad in [
            "",
            "o3qrm",
            "O3QRMM",
            "http://wallhaven.cc/w/o3qrmm",
            "file:///etc/passwd",
            "wallhaven.cc/w/o3qrmm",
            "https://",
            "https:/é€é€",
            "https://wallhaven.cc/w/",
            "https://wallhaven.cc/w/../../x",
            "https://wallhaven.cc/w/o3qrmmm",
        ] {
            assert!(parse_wallpaper_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn api_responses_keep_only_https_and_requested_ids() {
        assert_eq!(
            wallhaven_image_url(br#"{"data":{"path":"https://w.wallhaven.cc/full/a.jpg"}}"#),
            Some("https://w.wallhaven.cc/full/a.jpg".into())
        );
        for bad in [
            &br#"{"data":{"path":"http://w.wallhaven.cc/a.jpg"}}"#[..],
            br#"{"error":"Nothing here"}"#,
            b"<html>",
        ] {
            assert_eq!(wallhaven_image_url(bad), None);
        }

        let json = br#"{"response":{"result":1,"resultcount":4,"publishedfiledetails":[
            {"publishedfileid":"111","result":1,"preview_url":"https://images.steamusercontent.com/ugc/a/"},
            {"publishedfileid":"222","result":9},
            {"publishedfileid":"333","result":1,"preview_url":"http://insecure/x.png"},
            {"publishedfileid":"../../evil","result":1,"preview_url":"https://x/y.png"},
            {"publishedfileid":"444","result":1,"preview_url":""}]}}"#;
        let asked = ["111", "222", "333", "444"].map(String::from);
        assert_eq!(
            parse_previews(json, &asked),
            [(
                "111".to_string(),
                "https://images.steamusercontent.com/ugc/a/".to_string()
            )]
        );
        assert!(parse_previews(b"garbage", &asked).is_empty());
        assert!(is_workshop_id("2216917595"));
        for bad in ["", "1/2", "..", "12a", "123456789012345678901"] {
            assert!(!is_workshop_id(bad), "{bad}");
        }
    }
}
