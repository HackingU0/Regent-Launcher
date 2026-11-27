//! Game discovery (Steam / GOG / Xbox) and launch command. UI-free; roots are injected.
use crate::mods::vdf_paths;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Steam,
    Gog,
    Xbox,
}

#[derive(Debug)]
pub struct GameDef {
    /// Key used in playsets.json.
    pub id: &'static str,
    /// Also the install folder name on every channel and the Documents folder name.
    pub title: &'static str,
    pub steam_app: u32,
    /// Curated default wallpaper (full image URL); None falls back to the theme background.
    pub wallpaper: Option<&'static str>,
}

pub const GAMES: [GameDef; 4] = [
    GameDef {
        id: "stellaris",
        title: "Stellaris",
        steam_app: 281990,
        wallpaper: Some("https://w.wallhaven.cc/full/2k/wallhaven-2koyqx.png"),
    },
    GameDef {
        id: "eu4",
        title: "Europa Universalis IV",
        steam_app: 236850,
        wallpaper: None,
    },
    GameDef {
        id: "hoi4",
        title: "Hearts of Iron IV",
        steam_app: 394360,
        wallpaper: None,
    },
    GameDef {
        id: "ck3",
        title: "Crusader Kings III",
        steam_app: 1158310,
        wallpaper: Some("https://w.wallhaven.cc/full/o3/wallhaven-o3qrmm.jpg"),
    },
];

#[derive(Clone, Debug, Default)]
pub struct Roots {
    pub home: PathBuf,
    pub steam_roots: Vec<PathBuf>,
    pub gog_roots: Vec<PathBuf>,
    pub xbox_roots: Vec<PathBuf>,
    pub documents: PathBuf,
    /// App data dir; our store lives under it.
    pub data: PathBuf,
}

impl Roots {
    /// The only place with per-OS path rules.
    pub fn system() -> Roots {
        let home = dirs::home_dir().unwrap_or_default();
        let mut steam_roots = vec![];
        if cfg!(target_os = "macos") {
            steam_roots.push(home.join("Library/Application Support/Steam"));
        } else if cfg!(target_os = "linux") {
            steam_roots.push(home.join(".local/share/Steam"));
            steam_roots.push(home.join(".steam/steam"));
        } else if cfg!(windows) {
            steam_roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        }
        // ponytail: drive letters only, no registry lookup; add winreg if users report misses.
        let drives = |sub: &str| -> Vec<PathBuf> {
            if !cfg!(windows) {
                return vec![];
            }
            ('C'..='Z')
                .map(|d| PathBuf::from(format!("{d}:\\{sub}")))
                .filter(|p| p.is_dir())
                .collect()
        };
        let mut gog_roots = drives("GOG Games");
        gog_roots.extend(drives(r"Program Files (x86)\GOG Galaxy\Games"));
        Roots {
            documents: dirs::document_dir().unwrap_or_else(|| home.join("Documents")),
            data: dirs::data_dir().unwrap_or_else(|| home.clone()),
            xbox_roots: drives("XboxGames"),
            steam_roots,
            gog_roots,
            home,
        }
    }
    /// Synthetic roots rooted under `base`, used by tests and `REGENT_ROOT` fixture mode.
    pub fn under(base: &Path) -> Roots {
        Roots {
            home: base.join("home"),
            documents: base.join("docs"),
            data: base.join("data"),
            steam_roots: vec![base.join("steam")],
            gog_roots: vec![base.join("gog")],
            xbox_roots: vec![base.join("xbox")],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Install {
    pub def: &'static GameDef,
    pub channel: Channel,
    /// Game content root (dlc/, checksum_manifest.txt).
    pub dir: PathBuf,
    /// Folder holding launcher-settings.json; same as `dir` unless `dlcPath` says otherwise.
    pub launcher_dir: PathBuf,
    /// Theme settings JSON and in-game settings layout, both under `launcher_dir`.
    pub theme_file: PathBuf,
    pub settings_layout: PathBuf,
    pub workshop: Option<PathBuf>,
    /// Display version string from launcher-settings.json `version`, e.g. "Corona Borealis v4.6.0 (3d1a)".
    pub version: Option<String>,
    /// None means "unknown", so no version-mismatch highlight.
    pub raw_version: Option<String>,
    /// Documents/Paradox Interactive/<game>; holds mod/ and dlc_load.json.
    pub data_dir: PathBuf,
    pub exe: PathBuf,
    pub exe_args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LauncherSettings {
    version: Option<String>,
    raw_version: Option<String>,
    game_data_path: Option<String>,
    exe_path: String,
    #[serde(default)]
    exe_args: Vec<String>,
    dlc_path: Option<String>,
    theme_file: Option<String>,
    ingame_settings_layout_path: Option<String>,
}

/// Lexically drop `.` and resolve `..`; no filesystem access (canonicalize adds `\\?\` on Windows).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

pub fn expand_data_path(raw: &str, roots: &Roots) -> PathBuf {
    let raw = raw.trim();
    let raw = raw.replace('\\', "/");
    let strip = |p: &str| raw.strip_prefix(p).map(str::to_owned);
    if let Some(r) = strip("~/Documents/").or_else(|| strip("%USER_DOCUMENTS%/")) {
        roots.documents.join(r)
    } else if let Some(r) = strip("$LINUX_DATA_HOME/") {
        roots.home.join(".local/share").join(r)
    } else if let Some(r) = strip("~/") {
        roots.home.join(r)
    } else {
        PathBuf::from(raw)
    }
}

fn load_install(
    def: &'static GameDef,
    channel: Channel,
    root: PathBuf,
    workshop: Option<PathBuf>,
    roots: &Roots,
) -> Option<Install> {
    // Stellaris keeps launcher-settings.json at the root, CK3 under launcher/.
    let (launcher_dir, text) = [root.clone(), root.join("launcher")]
        .into_iter()
        .find_map(|d| {
            let text = std::fs::read_to_string(d.join("launcher-settings.json")).ok()?;
            Some((normalize(&d), text))
        })?;
    let s: LauncherSettings = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let data_dir = match &s.game_data_path {
        Some(p) => expand_data_path(p, roots),
        None => roots.documents.join("Paradox Interactive").join(def.title),
    };
    let resolve = |rel: &str| normalize(&launcher_dir.join(rel));
    let inside = |rel: Option<&str>, default: &str| {
        rel.map(resolve)
            .filter(|p| p.starts_with(&launcher_dir) && p != &launcher_dir)
            .unwrap_or_else(|| resolve(default))
    };
    Some(Install {
        def,
        channel,
        exe: resolve(&s.exe_path),
        exe_args: s.exe_args,
        version: s.version,
        raw_version: s.raw_version,
        workshop: workshop.filter(|w| w.is_dir()),
        data_dir,
        dir: resolve(s.dlc_path.as_deref().unwrap_or("")),
        theme_file: inside(s.theme_file.as_deref(), "assets/theme-settings.json"),
        settings_layout: inside(
            s.ingame_settings_layout_path.as_deref(),
            "settings-layout.json",
        ),
        launcher_dir,
    })
}

fn steam_libs(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    if let Ok(t) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        for p in vdf_paths(&t) {
            if !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

fn find(def: &'static GameDef, roots: &Roots) -> Option<Install> {
    for root in &roots.steam_roots {
        for lib in steam_libs(root) {
            let apps = lib.join("steamapps");
            let workshop = apps
                .join("workshop/content")
                .join(def.steam_app.to_string());
            let dir = apps.join("common").join(def.title);
            if let Some(i) = load_install(def, Channel::Steam, dir, Some(workshop), roots) {
                return Some(i);
            }
        }
    }
    let gog = roots.gog_roots.iter().map(|r| r.join(def.title));
    let gog = gog.filter_map(|d| load_install(def, Channel::Gog, d, None, roots));
    let xbox = roots
        .xbox_roots
        .iter()
        .map(|r| r.join(def.title).join("Content"));
    let xbox = xbox.filter_map(|d| load_install(def, Channel::Xbox, d, None, roots));
    gog.chain(xbox).next()
}

/// One install per game; Steam beats GOG beats Xbox.
pub fn discover(roots: &Roots) -> Vec<Install> {
    GAMES.iter().filter_map(|d| find(d, roots)).collect()
}

/// Quote-aware split: `-a "b c"` -> ["-a", "b c"].
pub fn split_args(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut has) = (vec![], String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' => (quoted, has) = (!quoted, true),
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

pub fn launch_command(install: &Install, extra: &str) -> Command {
    let mut c = Command::new(&install.exe);
    c.args(&install.exe_args).args(split_args(extra));
    // Next to the .app bundle (else next to the binary): Stellaris' root, CK3's binaries/.
    let bundle = install
        .exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"));
    if let Some(cwd) = bundle.unwrap_or(&install.exe).parent() {
        c.current_dir(cwd);
    }
    c
}

#[cfg(test)]
pub(crate) fn tmp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    static N: AtomicUsize = AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "regent-test-{}-{}-{tag}",
        std::process::id(),
        N.fetch_add(1, Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[cfg(test)]
pub(crate) fn put(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = r#"{ "gameId": "stellaris", "version": "Cepheus", "rawVersion": "v4.6.0",
        "gameDataPath": "~/Documents/Paradox Interactive/Stellaris",
        "exePath": "./stellaris.app/Contents/MacOS/stellaris", "exeArgs": ["-gdpr-compliant"]   }  "#;

    fn roots(base: &Path) -> Roots {
        Roots {
            home: base.join("home"),
            documents: base.join("docs"),
            data: base.join("data"),
            steam_roots: vec![base.join("steam")],
            gog_roots: vec![base.join("gog")],
            xbox_roots: vec![base.join("xbox")],
        }
    }

    #[test]
    fn steam_moved_library_and_workshop() {
        let b = tmp_dir("steam");
        let lib2 = b.join("lib2");
        put(
            &b.join("steam/steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\"0\" {{ \"path\" \"{}\" }}\n\"1\" {{ \"path\" \"{}\" \"apps\" {{ \"281990\" \"1\" }} }}\n}}",
                b.join("steam").display(),
                lib2.display()
            ),
        );
        put(
            &lib2.join("steamapps/common/Stellaris/launcher-settings.json"),
            SETTINGS,
        );
        std::fs::create_dir_all(lib2.join("steamapps/workshop/content/281990")).unwrap();
        let r = roots(&b);
        let found = discover(&r);
        assert_eq!(found.len(), 1);
        let i = &found[0];
        assert_eq!((i.def.id, i.channel), ("stellaris", Channel::Steam));
        assert_eq!(i.raw_version.as_deref(), Some("v4.6.0"));
        assert_eq!(i.data_dir, b.join("docs/Paradox Interactive/Stellaris"));
        assert!(i.workshop.is_some());
        assert!(i.exe.ends_with("stellaris.app/Contents/MacOS/stellaris"));
    }

    #[test]
    fn gog_xbox_and_steam_priority() {
        let b = tmp_dir("gogxbox");
        let r = roots(&b);
        put(
            &b.join("gog/Hearts of Iron IV/launcher-settings.json"),
            SETTINGS,
        );
        put(
            &b.join("xbox/Hearts of Iron IV/Content/launcher-settings.json"),
            SETTINGS,
        );
        put(
            &b.join("xbox/Europa Universalis IV/Content/launcher-settings.json"),
            SETTINGS,
        );
        let found = discover(&r);
        let by = |id| found.iter().find(|i| i.def.id == id).unwrap().channel;
        assert_eq!(by("hoi4"), Channel::Gog);
        assert_eq!(by("eu4"), Channel::Xbox);
        put(
            &b.join("steam/steamapps/common/Hearts of Iron IV/launcher-settings.json"),
            SETTINGS,
        );
        let found = discover(&r);
        assert_eq!(
            found.iter().find(|i| i.def.id == "hoi4").unwrap().channel,
            Channel::Steam
        );
        // folder without launcher-settings.json does not count
        std::fs::create_dir_all(b.join("gog/Stellaris")).unwrap();
        let found = discover(&r);
        assert!(!found.iter().any(|i| i.def.id == "stellaris"));
    }

    #[test]
    fn minimal_settings_and_defaults() {
        let b = tmp_dir("minimal");
        put(
            &b.join("steam/steamapps/common/Stellaris/launcher-settings.json"),
            r#"{"exePath":"s.exe"}"#,
        );
        let i = &discover(&roots(&b))[0];
        assert!(i.raw_version.is_none() && i.exe_args.is_empty());
        assert_eq!(i.data_dir, b.join("docs/Paradox Interactive/Stellaris"));
    }

    #[test]
    fn data_path_forms() {
        let r = roots(Path::new("/b"));
        let docs = PathBuf::from("/b/docs/Paradox Interactive/X");
        assert_eq!(
            expand_data_path("~/Documents/Paradox Interactive/X", &r),
            docs
        );
        assert_eq!(
            expand_data_path("%USER_DOCUMENTS%/Paradox Interactive/X", &r),
            docs
        );
        assert_eq!(
            expand_data_path("%USER_DOCUMENTS%\\Paradox Interactive\\X", &r),
            docs
        );
        assert_eq!(
            expand_data_path("$LINUX_DATA_HOME/Paradox Interactive/X", &r),
            PathBuf::from("/b/home/.local/share/Paradox Interactive/X")
        );
        assert_eq!(expand_data_path("/abs/x", &r), PathBuf::from("/abs/x"));
    }

    #[test]
    fn args_and_command() {
        assert_eq!(
            split_args(r#"-debug_mode "-a b" ""  c"#),
            ["-debug_mode", "-a b", "", "c"]
        );
        assert!(split_args("   ").is_empty());
        let b = tmp_dir("cmd");
        put(
            &b.join("steam/steamapps/common/Stellaris/launcher-settings.json"),
            SETTINGS,
        );
        let i = &discover(&roots(&b))[0];
        let c = launch_command(i, "-debug_mode \"x y\"");
        assert_eq!(c.get_program(), i.exe.as_os_str());
        let args: Vec<_> = c.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["-gdpr-compliant", "-debug_mode", "x y"]);
        assert_eq!(c.get_current_dir(), Some(i.dir.as_path()));
    }

    #[test]
    fn ck3_layout_launcher_subfolder() {
        let b = tmp_dir("ck3");
        let root = b.join("steam/steamapps/common/Crusader Kings III");
        put(
            &root.join("launcher/launcher-settings.json"),
            r#"{ "gameId": "ck3", "exePath": "../binaries/ck3.app/Contents/MacOS/ck3",
                "dlcPath": "../game", "ingameSettingsLayoutPath": "settings-layout.json",
                "themeFile": "./assets/theme-settings.json" }"#,
        );
        let i = &discover(&roots(&b))[0];
        assert_eq!(i.def.id, "ck3");
        assert_eq!(i.dir, root.join("game"));
        assert_eq!(i.launcher_dir, root.join("launcher"));
        assert_eq!(i.exe, root.join("binaries/ck3.app/Contents/MacOS/ck3"));
        assert_eq!(
            i.theme_file,
            root.join("launcher/assets/theme-settings.json")
        );
        assert_eq!(
            i.settings_layout,
            root.join("launcher/settings-layout.json")
        );
        assert_eq!(
            launch_command(i, "").get_current_dir(),
            Some(root.join("binaries").as_path())
        );
    }

    #[test]
    fn theme_and_layout_paths_cannot_leave_launcher_dir() {
        let b = tmp_dir("escape");
        put(
            &b.join("steam/steamapps/common/Stellaris/launcher-settings.json"),
            r#"{"exePath":"s","themeFile":"../x.json","ingameSettingsLayoutPath":"/etc/hosts"}"#,
        );
        let i = &discover(&roots(&b))[0];
        assert_eq!(
            i.theme_file,
            i.launcher_dir.join("assets/theme-settings.json")
        );
        assert_eq!(
            i.settings_layout,
            i.launcher_dir.join("settings-layout.json")
        );
    }
}
