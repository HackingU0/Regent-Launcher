//! settings.txt / pdx_settings.txt: edit one value by splicing its byte span (everything else
//! stays byte-identical), field table over both files, backups and rollback. UI-free.
use crate::game::{Install, Roots};
use crate::mods::{Tok, tokens};
use crate::playset::{app_dir, lock_app, stage_atomic_bytes, write_atomic_bytes};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fs::{self, Permissions};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------- text

/// Byte span of a scalar value inside the text.
#[derive(Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub quoted: bool,
}

/// First scalar at the nested key path (quoted and unquoted keys are the same key).
pub fn locate(text: &str, path: &[&str]) -> Option<Span> {
    let (toks, _) = tokens(text);
    let mut stack: Vec<&str> = vec![];
    let mut i = 0;
    while i < toks.len() {
        let next = |n: usize| toks.get(i + n).map(|t| &t.0);
        match (&toks[i].0, next(1), toks.get(i + 2)) {
            (Tok::Atom(k) | Tok::Str(k), Some(Tok::Eq), Some((v, r))) => {
                match v {
                    Tok::Open => {
                        stack.push(k);
                        i += 3;
                        continue;
                    }
                    Tok::Atom(_) | Tok::Str(_) => {
                        if stack.len() + 1 == path.len()
                            && stack.iter().zip(path).all(|(a, b)| a == b)
                            && k == path[stack.len()]
                        {
                            return Some(Span {
                                range: r.clone(),
                                quoted: matches!(v, Tok::Str(_)),
                            });
                        }
                        i += 3;
                        continue;
                    }
                    _ => {}
                }
                i += 2;
                continue;
            }
            (Tok::Open, ..) => stack.push(""),
            (Tok::Close, ..) => {
                stack.pop();
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The value without its quotes.
pub fn get(text: &str, path: &[&str]) -> Option<String> {
    let s = locate(text, path)?;
    let t = &text[s.range];
    let t = t.strip_prefix('"').unwrap_or(t);
    Some(t.strip_suffix('"').unwrap_or(t).to_string())
}

/// Replace one value, keeping quoting; floats are written with 6 decimals like the game does.
/// The rest of the text is returned byte for byte.
pub fn set(text: &str, path: &[&str], value: &str) -> Result<String> {
    let s = locate(text, path).with_context(|| format!("no such setting: {}", path.join(".")))?;
    let old = &text[s.range.clone()];
    let new = if s.quoted {
        ensure!(
            !value.contains(['"', '\\', '\n', '\r']),
            "a quoted value can't contain quotes, backslashes or line breaks"
        );
        format!("\"{value}\"")
    } else {
        ensure!(
            !value.is_empty()
                && !value
                    .chars()
                    .any(|c| c.is_whitespace() || "={}\"#".contains(c)),
            "'{value}' is not a valid value (empty, spaces and = {{ }} \" # are not allowed)"
        );
        if old.contains('.') && old.parse::<f64>().is_ok() {
            let v = value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .with_context(|| format!("'{value}' is not a number"))?;
            format!("{v:.6}")
        } else {
            value.to_string()
        }
    };
    Ok(format!(
        "{}{new}{}",
        &text[..s.range.start],
        &text[s.range.end..]
    ))
}

/// Balanced quotes and braces: what the raw editor must satisfy before a write.
pub fn check(text: &str) -> Result<()> {
    let (toks, unterminated) = tokens(text);
    ensure!(!unterminated, "unclosed quote");
    let line = |at: usize| text[..at].matches('\n').count() + 1;
    let mut depth = 0usize;
    for (t, r) in &toks {
        match t {
            Tok::Open => depth += 1,
            Tok::Close => {
                depth = depth
                    .checked_sub(1)
                    .with_context(|| format!("unmatched }} on line {}", line(r.start)))?
            }
            _ => {}
        }
    }
    ensure!(depth == 0, "{depth} unclosed {{");
    Ok(())
}

/// Text for a raw editor (the editor works with LF).
pub fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Edited editor text back to the line endings the original file used.
pub fn restore_eol(orig: &str, edited: &str) -> String {
    if orig.contains("\r\n") {
        to_lf(edited).replace('\n', "\r\n")
    } else {
        edited.to_string()
    }
}

// ---------------------------------------------------------------- files

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum File {
    Game,
    Pdx,
}

impl File {
    pub const ALL: [File; 2] = [File::Game, File::Pdx];

    pub fn name(self) -> &'static str {
        match self {
            File::Game => "settings.txt",
            File::Pdx => "pdx_settings.txt",
        }
    }
}

pub fn path(install: &Install, f: File) -> PathBuf {
    install.data_dir.join(f.name())
}

/// The text of both files; None = the file does not exist.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Files {
    pub game: Option<String>,
    pub pdx: Option<String>,
}

impl Files {
    pub fn load(install: &Install) -> Result<Files> {
        let read = |f: File| -> Result<Option<String>> {
            let p = path(install, f);
            match fs::read_to_string(&p) {
                Ok(s) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => bail!("{}: {e}", p.display()),
            }
        };
        Ok(Files {
            game: read(File::Game)?,
            pdx: read(File::Pdx)?,
        })
    }

    pub fn get(&self, f: File) -> Option<&String> {
        match f {
            File::Game => self.game.as_ref(),
            File::Pdx => self.pdx.as_ref(),
        }
    }

    pub fn slot(&mut self, f: File) -> &mut Option<String> {
        match f {
            File::Game => &mut self.game,
            File::Pdx => &mut self.pdx,
        }
    }
}

/// One value to splice into one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub file: File,
    pub path: Vec<String>,
    pub value: String,
}

/// Apply edits in memory.
pub fn edit(files: &Files, edits: &[Edit]) -> Result<Files> {
    let mut out = files.clone();
    for e in edits {
        let text = out
            .slot(e.file)
            .as_mut()
            .with_context(|| format!("{} does not exist", e.file.name()))?;
        let path: Vec<&str> = e.path.iter().map(String::as_str).collect();
        *text = set(text, &path, &e.value).with_context(|| e.file.name())?;
    }
    Ok(out)
}

// ---------------------------------------------------------------- fields

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Toggle,
    Slider {
        min: f64,
        max: f64,
        step: f64,
    },
    Int {
        max: u32,
    },
    /// "WxH"
    Resolution,
    /// fullscreen | borderless_fullscreen | windowed
    Mode,
    Language,
}

type Paths = &'static [&'static [&'static str]];

/// One setting as the user sees it; `game` / `pdx` are its key paths in each file.
pub struct Field {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    game: Paths,
    pdx: Paths,
}

const VOL: Kind = Kind::Slider {
    min: 0.0,
    max: 100.0,
    step: 1.0,
};

pub const FIELDS: [Field; 11] = [
    Field {
        id: "resolution",
        label: "Resolution",
        kind: Kind::Resolution,
        game: &[&["graphics", "size", "x"], &["graphics", "size", "y"]],
        pdx: &[
            &["Graphics", "fullscreen_resolution", "value"],
            &["Graphics", "windowed_resolution", "value"],
        ],
    },
    Field {
        id: "display-mode",
        label: "Display mode",
        kind: Kind::Mode,
        game: &[&["graphics", "fullScreen"], &["graphics", "borderless"]],
        pdx: &[&["Graphics", "display_mode", "value"]],
    },
    Field {
        id: "vsync",
        label: "VSync",
        kind: Kind::Toggle,
        game: &[&["graphics", "vsync"]],
        pdx: &[&["Graphics", "vsync", "enabled"]],
    },
    Field {
        id: "ui-scale",
        label: "UI scale",
        kind: Kind::Slider {
            min: 0.5,
            max: 2.0,
            step: 0.05,
        },
        game: &[&["graphics", "gui_scale"]],
        pdx: &[],
    },
    Field {
        id: "language",
        label: "Language",
        kind: Kind::Language,
        game: &[&["language"]],
        pdx: &[&["System", "language", "value"]],
    },
    Field {
        id: "autosave",
        label: "Autosave",
        kind: Kind::Int { max: 99 },
        game: &[&["autosave"]],
        pdx: &[],
    },
    Field {
        id: "master-volume",
        label: "Master volume",
        kind: VOL,
        game: &[&["master_volume"]],
        pdx: &[],
    },
    Field {
        id: "music-volume",
        label: "Music volume",
        kind: VOL,
        game: &[&["music_volume"]],
        pdx: &[],
    },
    Field {
        id: "sound-fx-volume",
        label: "Sound FX volume",
        kind: VOL,
        game: &[&["sound_fx_volume"]],
        pdx: &[],
    },
    Field {
        id: "ambient-volume",
        label: "Ambient volume",
        kind: VOL,
        game: &[&["ambient_volume"]],
        pdx: &[],
    },
    Field {
        id: "voice-volume",
        label: "Voice volume",
        kind: VOL,
        game: &[&["voice_volume"]],
        pdx: &[],
    },
];

/// (label, value) of the display modes.
pub const MODES: [(&str, &str); 3] = [
    ("Fullscreen", "fullscreen"),
    ("Borderless fullscreen", "borderless_fullscreen"),
    ("Windowed", "windowed"),
];

pub fn field(name: &str) -> Option<&'static Field> {
    let n = name.to_lowercase().replace('_', "-");
    FIELDS.iter().find(|f| f.id == n)
}

/// What a field currently holds.
#[derive(Debug, PartialEq)]
pub struct Reading {
    pub value: String,
    /// The other file disagrees.
    pub note: Option<String>,
}

impl Field {
    fn paths(&self, f: File) -> Paths {
        match f {
            File::Game => self.game,
            File::Pdx => self.pdx,
        }
    }

    /// The value in one file; None when the file or one of the keys is missing.
    fn read_in(&self, files: &Files, f: File) -> Option<String> {
        let text = files.get(f)?;
        let p = self.paths(f);
        // pdx resolution also lists windowed_resolution, which is optional.
        let need = if f == File::Pdx {
            p.len().min(1)
        } else {
            p.len()
        };
        if need == 0 {
            return None;
        }
        let v: Vec<String> = p[..need]
            .iter()
            .map(|k| get(text, k))
            .collect::<Option<_>>()?;
        Some(match (self.kind, f) {
            (Kind::Resolution, File::Game) => format!("{}x{}", v[0], v[1]),
            (Kind::Mode, File::Game) => match (v[0] == "yes", v[1] == "yes") {
                (true, _) => "fullscreen",
                (_, true) => "borderless_fullscreen",
                _ => "windowed",
            }
            .into(),
            _ => v[0].clone(),
        })
    }

    /// pdx_settings.txt first, then settings.txt. None = in neither file, so the field is hidden.
    pub fn read(&self, files: &Files) -> Option<Reading> {
        let found: Vec<(File, String)> = [File::Pdx, File::Game]
            .into_iter()
            .filter_map(|f| self.read_in(files, f).map(|v| (f, v)))
            .collect();
        let value = found.first()?.1.clone();
        let note = found
            .get(1)
            .filter(|(_, o)| !self.same(o, &value))
            .map(|(f, o)| format!("{} says {o}", f.name()));
        Some(Reading { value, note })
    }

    /// Equal values (numbers compare as numbers).
    pub fn same(&self, a: &str, b: &str) -> bool {
        match (self.kind, a.parse::<f64>(), b.parse::<f64>()) {
            (Kind::Slider { .. } | Kind::Int { .. }, Ok(x), Ok(y)) => (x - y).abs() < 1e-4,
            _ => a == b,
        }
    }

    /// Check user input and return it in canonical form.
    pub fn validate(&self, input: &str) -> Result<String> {
        let s = input.trim();
        Ok(match self.kind {
            Kind::Toggle => match s.to_lowercase().as_str() {
                "yes" | "on" | "true" | "1" => "yes",
                "no" | "off" | "false" | "0" => "no",
                _ => bail!("expected yes or no"),
            }
            .into(),
            Kind::Slider { min, max, .. } => {
                let v: f64 = s
                    .parse()
                    .ok()
                    .filter(|v: &f64| v.is_finite())
                    .with_context(|| format!("'{s}' is not a number"))?;
                ensure!((min..=max).contains(&v), "must be between {min} and {max}");
                v.to_string()
            }
            Kind::Int { max } => {
                let v: u32 = s
                    .parse()
                    .with_context(|| format!("'{s}' is not a whole number"))?;
                ensure!(v <= max, "must be between 0 and {max}");
                v.to_string()
            }
            Kind::Resolution => {
                let dim = |t: &str| t.parse::<u32>().ok().filter(|n| (1..=16384).contains(n));
                match s.to_lowercase().split_once('x') {
                    Some((w, h)) => match (dim(w), dim(h)) {
                        (Some(w), Some(h)) => format!("{w}x{h}"),
                        _ => bail!("expected WIDTHxHEIGHT, like 1920x1080"),
                    },
                    None => bail!("expected WIDTHxHEIGHT, like 1920x1080"),
                }
            }
            Kind::Mode => {
                ensure!(
                    MODES.iter().any(|m| m.1 == s),
                    "expected fullscreen, borderless_fullscreen or windowed"
                );
                s.to_string()
            }
            Kind::Language => {
                ensure!(!s.is_empty(), "empty language");
                s.to_string()
            }
        })
    }

    /// The edits that set this field in every file that has its keys.
    pub fn plan(&self, input: &str, files: &Files) -> Result<Vec<Edit>> {
        let v = self.validate(input)?;
        let mut out = vec![];
        for f in File::ALL {
            let Some(text) = files.get(f) else { continue };
            let p = self.paths(f);
            let vals: Vec<String> = match (self.kind, f) {
                (Kind::Resolution, File::Game) => {
                    let (w, h) = v.split_once('x').unwrap_or_default();
                    vec![w.into(), h.into()]
                }
                (Kind::Mode, File::Game) => match v.as_str() {
                    "fullscreen" => vec!["yes".into(), "no".into()],
                    "borderless_fullscreen" => vec!["no".into(), "yes".into()],
                    _ => vec!["no".into(), "no".into()],
                },
                _ => vec![v.clone(); p.len()],
            };
            for (k, val) in p.iter().zip(vals) {
                if locate(text, k).is_some() {
                    out.push(Edit {
                        file: f,
                        path: k.iter().map(|s| s.to_string()).collect(),
                        value: val,
                    });
                }
            }
        }
        ensure!(!out.is_empty(), "{} is not in your settings files", self.id);
        Ok(out)
    }
}

/// (label, value) of the languages the game offers, from the install's settings layout JSON.
/// Empty when the file is missing or has none.
pub fn language_options(install: &Install) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(&install.settings_layout) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return vec![];
    };
    let s = |v: &serde_json::Value, k: &str| v[k].as_str().map(str::to_string);
    v.as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| c["settings"].as_array().into_iter().flatten())
        .filter(|s| s["name"] == "language")
        .flat_map(|s| s["options"].as_array().into_iter().flatten())
        .filter_map(|o| Some((s(o, "label")?, s(o, "value")?)))
        .collect()
}

// ---------------------------------------------------------------- backups

const KEEP: usize = 20;
const MANIFEST_NAME: &str = "manifest.json";

static BAK_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FileSnapMeta {
    present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct BackupManifest {
    game: FileSnapMeta,
    pdx: FileSnapMeta,
}

impl BackupManifest {
    fn entry(&self, f: File) -> &FileSnapMeta {
        match f {
            File::Game => &self.game,
            File::Pdx => &self.pdx,
        }
    }
    fn entry_mut(&mut self, f: File) -> &mut FileSnapMeta {
        match f {
            File::Game => &mut self.game,
            File::Pdx => &mut self.pdx,
        }
    }
}

pub struct Backup {
    /// Unix milliseconds of creation; also the directory name.
    pub id: u64,
    pub dir: PathBuf,
    pub files: Vec<File>,
}

fn backup_root(install: &Install, roots: &Roots) -> PathBuf {
    app_dir(roots).join("backups").join(install.def.id)
}

fn read_manifest(dir: &Path) -> Option<BackupManifest> {
    let text = fs::read_to_string(dir.join(MANIFEST_NAME)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Newest first. Ignores hidden staging directories (`.tmp-*`).
pub fn backups(install: &Install, roots: &Roots) -> Vec<Backup> {
    let Ok(rd) = fs::read_dir(backup_root(install, roots)) else {
        return vec![];
    };
    let mut v: Vec<Backup> = rd
        .flatten()
        .filter_map(|e| {
            let id: u64 = e.file_name().to_str()?.parse().ok()?;
            let dir = e.path();
            if !dir.is_dir() {
                return None;
            }
            if let Some(m) = read_manifest(&dir) {
                let files: Vec<File> = File::ALL
                    .into_iter()
                    .filter(|&f| m.entry(f).present && dir.join(f.name()).is_file())
                    .collect();
                Some(Backup { id, dir, files })
            } else {
                let files: Vec<File> = File::ALL
                    .into_iter()
                    .filter(|f| dir.join(f.name()).is_file())
                    .collect();
                (!files.is_empty()).then_some(Backup { id, dir, files })
            }
        })
        .collect();
    v.sort_by_key(|b| std::cmp::Reverse(b.id));
    v
}

/// Commit a staged backup directory into `<root>/<ms>`, moving to the next millisecond if taken.
fn commit_dir(root: &Path, staging: &Path, mut ms: u64) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    loop {
        let d = root.join(ms.to_string());
        if d.exists() {
            ms += 1;
            continue;
        }
        match fs::rename(staging, &d) {
            Ok(()) => return Ok(d),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                ms += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Create `<root>/<ms>`, moving to the next millisecond when the name is taken.
#[cfg(test)]
fn new_dir(root: &Path, mut ms: u64) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    loop {
        let d = root.join(ms.to_string());
        match fs::create_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
            Err(e) => return Err(e.into()),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(unix)]
fn file_mode(m: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(m.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn file_mode(_m: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn perms_from_mode(mode: Option<u32>) -> Option<Permissions> {
    use std::os::unix::fs::PermissionsExt;
    mode.map(Permissions::from_mode)
}

#[cfg(not(unix))]
fn perms_from_mode(_mode: Option<u32>) -> Option<Permissions> {
    None
}

/// Snapshot both settings files (bytes, presence, and Unix mode) into a temp dir, write
/// `manifest.json`, then atomically rename into `<backup_root>/<ms>` so an interrupted copy
/// never looks like a valid backup. Drops all but the newest `KEEP`.
fn backup_snapshot(install: &Install, roots: &Roots) -> Result<PathBuf> {
    let root = backup_root(install, roots);
    fs::create_dir_all(&root)?;
    let staging = loop {
        let seq = BAK_SEQ.fetch_add(1, Ordering::Relaxed);
        let d = root.join(format!(".tmp-{}-{seq}", std::process::id()));
        match fs::create_dir(&d) {
            Ok(()) => break d,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let build = (|| -> Result<PathBuf> {
        let mut manifest = BackupManifest::default();
        for f in File::ALL {
            let src = path(install, f);
            match fs::read(&src) {
                Ok(bytes) => {
                    let meta = fs::metadata(&src)
                        .with_context(|| format!("reading metadata of {}", f.name()))?;
                    fs::write(staging.join(f.name()), &bytes)
                        .with_context(|| format!("backing up {}", f.name()))?;
                    *manifest.entry_mut(f) = FileSnapMeta {
                        present: true,
                        mode: file_mode(&meta),
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    *manifest.entry_mut(f) = FileSnapMeta {
                        present: false,
                        mode: None,
                    };
                }
                Err(e) => return Err(e).with_context(|| format!("backing up {}", f.name())),
            }
        }
        fs::write(
            staging.join(MANIFEST_NAME),
            serde_json::to_string_pretty(&manifest)?,
        )
        .context("writing backup manifest")?;
        commit_dir(&root, &staging, now_ms())
    })();
    let dir = match build {
        Ok(d) => d,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    for old in backups(install, roots).iter().skip(KEEP) {
        let _ = fs::remove_dir_all(&old.dir);
    }
    Ok(dir)
}

#[cfg(test)]
fn backup(install: &Install, roots: &Roots, _which: &[File]) -> Result<PathBuf> {
    let _lock = lock_app(roots)?;
    backup_snapshot(install, roots)
}

enum TargetAction {
    Write {
        bytes: Vec<u8>,
        perms: Option<Permissions>,
    },
    Delete,
}

type DiskEntry = (PathBuf, Option<(Vec<u8>, Permissions)>);
type DiskSnap = Vec<DiskEntry>;

fn snapshot_disk(paths: &[PathBuf]) -> Result<DiskSnap> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        match fs::read(p) {
            Ok(bytes) => {
                let perms = fs::metadata(p)?.permissions();
                out.push((p.clone(), Some((bytes, perms))));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => out.push((p.clone(), None)),
            Err(e) => return Err(e).with_context(|| format!("reading {}", p.display())),
        }
    }
    Ok(out)
}

/// Put every snapshotted file back, trying all of them; the error lists each file that failed.
fn restore_disk(snap: &[DiskEntry]) -> std::result::Result<(), String> {
    let mut failed = vec![];
    for (p, orig) in snap.iter().rev() {
        let res = match orig {
            Some((bytes, perms)) => write_atomic_bytes(p, bytes, Some(perms.clone())),
            None => match fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
        };
        if let Err(e) = res {
            failed.push(format!("{} ({e})", p.display()));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join(", "))
    }
}

#[cfg(test)]
type CommitHook = Option<(usize, Box<dyn Fn()>)>;

#[cfg(test)]
thread_local! {
    /// Test-only: fail the commit step with this index (after running the closure).
    static COMMIT_HOOK: std::cell::RefCell<CommitHook> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn injected_commit_failure(i: usize) -> Option<std::io::Error> {
    COMMIT_HOOK.with(|h| match &*h.borrow() {
        Some((at, f)) if *at == i => {
            f();
            Some(std::io::Error::other("injected commit failure"))
        }
        _ => None,
    })
}

/// All-or-nothing multi-file commit: stage all outputs first, commit each rename/delete in order,
/// and restore every already-committed file if any step fails.
fn apply_all_or_nothing(ops: Vec<(File, PathBuf, TargetAction)>, backup_dir: &Path) -> Result<()> {
    let paths: Vec<PathBuf> = ops.iter().map(|(_, p, _)| p.clone()).collect();
    let orig = snapshot_disk(&paths)?;
    let mut staged: Vec<(File, PathBuf, Option<PathBuf>)> = Vec::with_capacity(ops.len());
    for (f, dst, act) in ops {
        match act {
            TargetAction::Write { bytes, perms } => match stage_atomic_bytes(&dst, &bytes, perms) {
                Ok(tmp) => staged.push((f, dst, Some(tmp))),
                Err(e) => {
                    for (_, _, t) in &staged {
                        if let Some(t) = t {
                            let _ = fs::remove_file(t);
                        }
                    }
                    return Err(e).with_context(|| {
                        format!(
                            "staging {} (backup is in {})",
                            f.name(),
                            backup_dir.display()
                        )
                    });
                }
            },
            TargetAction::Delete => staged.push((f, dst, None)),
        }
    }
    for (i, (f, dst, tmp_opt)) in staged.iter().enumerate() {
        #[cfg(test)]
        let injected = injected_commit_failure(i);
        #[cfg(not(test))]
        let injected: Option<std::io::Error> = None;
        let res = match injected {
            Some(e) => Err(e),
            None => match tmp_opt {
                Some(tmp) => fs::rename(tmp, dst),
                None => match fs::remove_file(dst) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e),
                },
            },
        };
        if let Err(e) = res {
            for (_, _, t) in &staged[i..] {
                if let Some(t) = t {
                    let _ = fs::remove_file(t);
                }
            }
            let restore_note = match restore_disk(&orig[..i]) {
                Ok(()) => String::new(),
                Err(files) => format!(
                    "; could not restore the previous contents of: {files}, copy them back from the backup"
                ),
            };
            return Err(e).with_context(|| {
                format!(
                    "writing {} (backup is in {}){restore_note}",
                    f.name(),
                    backup_dir.display()
                )
            });
        }
    }
    Ok(())
}

/// Write the files that differ between `old` (what was loaded) and `new`: check, back up both
/// files atomically, then replace all-or-nothing. Returns the backup dir, None when nothing changed.
pub fn write(
    install: &Install,
    roots: &Roots,
    old: &Files,
    new: &Files,
) -> Result<Option<PathBuf>> {
    let changed: Vec<File> = File::ALL
        .into_iter()
        .filter(|&f| new.get(f).is_some() && new.get(f) != old.get(f))
        .collect();
    if changed.is_empty() {
        return Ok(None);
    }
    for &f in &changed {
        check(new.get(f).unwrap_or(&String::new())).with_context(|| f.name())?;
    }
    let _lock = lock_app(roots)?;
    ensure!(
        &Files::load(install)? == old,
        "the settings files changed on disk (the game rewrites them on exit); reload and try again"
    );
    let dir = backup_snapshot(install, roots)?;
    let ops = changed
        .into_iter()
        .map(|f| {
            let text = new.get(f).map_or("", |s| s);
            (
                f,
                path(install, f),
                TargetAction::Write {
                    bytes: text.as_bytes().to_vec(),
                    perms: None,
                },
            )
        })
        .collect();
    apply_all_or_nothing(ops, &dir)?;
    Ok(Some(dir))
}

fn restore_backup(install: &Install, roots: &Roots, b: Backup) -> Result<(Backup, PathBuf)> {
    let manifest = read_manifest(&b.dir);
    let mut ops: Vec<(File, PathBuf, TargetAction)> = vec![];
    match manifest {
        Some(m) => {
            for f in File::ALL {
                let entry = m.entry(f);
                if entry.present {
                    let bytes = fs::read(b.dir.join(f.name()))
                        .with_context(|| format!("reading backup {}", f.name()))?;
                    ops.push((
                        f,
                        path(install, f),
                        TargetAction::Write {
                            bytes,
                            perms: perms_from_mode(entry.mode),
                        },
                    ));
                } else {
                    ops.push((f, path(install, f), TargetAction::Delete));
                }
            }
        }
        None => {
            for &f in &b.files {
                let bytes = fs::read(b.dir.join(f.name()))
                    .with_context(|| format!("reading backup {}", f.name()))?;
                ops.push((
                    f,
                    path(install, f),
                    TargetAction::Write { bytes, perms: None },
                ));
            }
        }
    }
    let undo = backup_snapshot(install, roots)?;
    apply_all_or_nothing(ops, &undo)?;
    Ok((b, undo))
}

/// Restore the backup with exact `id` (resolved once under the cross-process lock).
/// Returns `Ok(None)` if no backup with that id exists.
pub fn rollback_by_id(
    install: &Install,
    roots: &Roots,
    id: u64,
) -> Result<Option<(Backup, PathBuf)>> {
    let _lock = lock_app(roots)?;
    let Some(b) = backups(install, roots).into_iter().find(|b| b.id == id) else {
        return Ok(None);
    };
    restore_backup(install, roots, b).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Channel, GAMES, put, tmp_dir};

    const GAME: &str = "force_pow2_textures=no\nlanguage=\"l_simp_chinese\"\ngraphics=\n{\n\tsize=\n\t{\n\t\tx=2048\n\t\ty=1152\n\t}\n\n\tgui_scale=1.000000\n\trefreshRate=120\n\tfullScreen=no\n\tborderless=yes\n\tvsync=yes\n}\nmaster_volume=100.000000\ninput_type=0\ninput_type=0\nautosave=4\nbloom=\n{\n\tquality=2\n}\ntransfer_speed=\"fast\"\n";
    const PDX: &str = "\"Graphics\"=\n{\n\t\"display_mode\"=\n\t{\n\t\tvalue=\"borderless_fullscreen\"\n\t\tversion=0\n\t}\n\t\"vsync\"=\n\t{\n\t\tenabled=yes\n\t\tversion=0\n\t}\n\t\"fullscreen_resolution\"=\n\t{\n\t\tvalue=\"2048x1152\"\n\t\tversion=0\n\t}\n}\n\"System\"=\n{\n\t\"language\"=\n\t{\n\t\tvalue=\"l_simp_chinese\"\n\t\tversion=0\n\t}\n}\n";

    fn crlf(s: &str) -> String {
        s.replace('\n', "\r\n")
    }

    fn install(base: &Path) -> (Install, Roots) {
        let data_dir = base.join("docs/Stellaris");
        fs::create_dir_all(&data_dir).unwrap();
        let i = Install {
            def: &GAMES[0],
            channel: Channel::Steam,
            dir: base.join("game"),
            launcher_dir: base.join("game"),
            theme_file: base.join("game/assets/theme-settings.json"),
            settings_layout: base.join("game/settings-layout.json"),
            workshop: None,
            version: None,
            raw_version: None,
            data_dir,
            exe: PathBuf::new(),
            exe_args: vec![],
        };
        let roots = Roots {
            data: base.join("data"),
            ..Default::default()
        };
        (i, roots)
    }

    fn files(i: &Install) -> Files {
        Files::load(i).unwrap()
    }

    #[test]
    fn locate_nested_duplicates_and_quoted_keys() {
        let g = crlf(GAME);
        assert_eq!(get(&g, &["graphics", "size", "x"]).unwrap(), "2048");
        assert_eq!(get(&g, &["language"]).unwrap(), "l_simp_chinese");
        assert_eq!(get(&g, &["bloom", "quality"]).unwrap(), "2");
        assert_eq!(get(&g, &["size", "x"]), None);
        assert_eq!(get(&g, &["graphics", "nope"]), None);
        let s = locate(&g, &["input_type"]).unwrap();
        assert_eq!(s.range.start, g.find("input_type=0").unwrap() + 11);
        let p = crlf(PDX);
        assert_eq!(get(&p, &["Graphics", "vsync", "enabled"]).unwrap(), "yes");
        assert!(locate(&p, &["System", "language", "value"]).unwrap().quoted);
        // a BOM keeps offsets valid
        let b = format!("\u{feff}{g}");
        assert_eq!(get(&b, &["graphics", "size", "y"]).unwrap(), "1152");
    }

    #[test]
    fn set_changes_only_the_value_bytes() {
        let cases: &[(&str, &[&str], &str, &str)] = &[
            (GAME, &["graphics", "size", "x"], "1920", "1920"),
            (GAME, &["graphics", "gui_scale"], "1.25", "1.250000"),
            (GAME, &["master_volume"], "75", "75.000000"),
            (GAME, &["language"], "l_german", "\"l_german\""),
            (GAME, &["input_type"], "1", "1"),
            (
                PDX,
                &["Graphics", "display_mode", "value"],
                "windowed",
                "\"windowed\"",
            ),
            (PDX, &["Graphics", "vsync", "enabled"], "no", "no"),
        ];
        for (base, path, v, written) in cases {
            for text in [base.to_string(), crlf(base)] {
                let s = locate(&text, path).unwrap();
                let out = set(&text, path, v).unwrap();
                assert_eq!(&out[..s.range.start], &text[..s.range.start]);
                assert_eq!(&out[s.range.start + written.len()..], &text[s.range.end..]);
                assert_eq!(&out[s.range.start..s.range.start + written.len()], *written);
                assert_eq!(out.len() + s.range.len(), text.len() + written.len());
            }
        }
        // duplicate key: only the first one changes
        let out = set(GAME, &["input_type"], "1").unwrap();
        assert_eq!(out.matches("input_type=1").count(), 1);
        assert_eq!(out.matches("input_type=0").count(), 1);
        assert!(out.find("input_type=1").unwrap() < out.find("input_type=0").unwrap());
        // no op set is byte identical
        assert_eq!(set(&crlf(GAME), &["autosave"], "4").unwrap(), crlf(GAME));
    }

    #[test]
    fn set_rejections() {
        for v in ["", "a b", "a=b", "{", "}", "a\"b", "a#b", "x\ny", " "] {
            assert!(set(GAME, &["autosave"], v).is_err(), "unquoted {v:?}");
        }
        for v in ["a\"b", "a\\b", "a\nb", "a\rb"] {
            assert!(set(GAME, &["language"], v).is_err(), "quoted {v:?}");
        }
        for v in ["abc", "", "nan", "inf"] {
            assert!(set(GAME, &["master_volume"], v).is_err(), "float {v:?}");
        }
        assert!(set(GAME, &["no", "such"], "1").is_err());
        // a quoted value may hold spaces, an unquoted int may be anything the rules allow
        assert!(set(GAME, &["language"], "a b").is_ok());
        assert!(set(GAME, &["autosave"], "abc").is_ok());
    }

    #[test]
    fn check_balanced() {
        assert!(check(GAME).is_ok() && check(PDX).is_ok() && check("").is_ok());
        assert!(check(&crlf(GAME)).is_ok());
        assert!(check("a=\"b").unwrap_err().to_string().contains("quote"));
        assert!(
            check("a={ b=1")
                .unwrap_err()
                .to_string()
                .contains("unclosed")
        );
        let e = check("a=1\n}").unwrap_err().to_string();
        assert!(e.contains("line 2"), "{e}");
        // braces and quotes in comments don't count
        assert!(check("# \" {\na=1").is_ok());
        assert!(check(&GAME.replace("\"fast\"", "\"fast")).is_err());
    }

    #[test]
    fn eol_round_trip() {
        let g = crlf(GAME);
        assert_eq!(restore_eol(&g, &to_lf(&g)), g);
        assert_eq!(restore_eol(GAME, "a\nb"), "a\nb");
        assert_eq!(restore_eol(&g, "a\r\nb\nc"), "a\r\nb\r\nc");
    }

    #[test]
    fn fields_read_and_note_mismatch() {
        let f = Files {
            game: Some(crlf(GAME)),
            pdx: Some(crlf(PDX)),
        };
        let r = |id: &str| field(id).unwrap().read(&f);
        assert_eq!(r("resolution").unwrap().value, "2048x1152");
        assert_eq!(r("display-mode").unwrap().value, "borderless_fullscreen");
        assert_eq!(r("vsync").unwrap().value, "yes");
        assert_eq!(r("language").unwrap().note, None);
        assert_eq!(r("autosave").unwrap().value, "4");
        assert_eq!(r("master_volume").unwrap().value, "100.000000");
        let mut g = f.clone();
        g.game = g.game.map(|t| t.replace("fullScreen=no", "fullScreen=yes"));
        let m = field("display-mode").unwrap().read(&g).unwrap();
        assert_eq!(m.value, "borderless_fullscreen");
        assert_eq!(m.note.unwrap(), "settings.txt says fullscreen");
        // only one file: still shown, from that file
        let only = Files {
            game: Some(GAME.into()),
            pdx: None,
        };
        assert_eq!(
            field("resolution").unwrap().read(&only).unwrap().value,
            "2048x1152"
        );
        assert!(
            field("resolution")
                .unwrap()
                .read(&Files::default())
                .is_none()
        );
    }

    #[test]
    fn plan_edits_every_file_with_the_key() {
        let f = Files {
            game: Some(GAME.into()),
            pdx: Some(PDX.into()),
        };
        let out = |id: &str, v: &str| {
            let e = field(id).unwrap().plan(v, &f).unwrap();
            edit(&f, &e).unwrap()
        };
        let r = out("resolution", "1920X1080");
        assert!(r.game.unwrap().contains("x=1920") && r.pdx.unwrap().contains("\"1920x1080\""));
        let w = out("display-mode", "windowed");
        let g = w.game.unwrap();
        assert!(g.contains("fullScreen=no") && g.contains("borderless=no"));
        assert!(w.pdx.unwrap().contains("\"windowed\""));
        let fs_ = out("display-mode", "fullscreen").game.unwrap();
        assert!(fs_.contains("fullScreen=yes") && fs_.contains("borderless=no"));
        assert!(out("vsync", "off").pdx.unwrap().contains("enabled=no"));
        assert!(
            out("ui-scale", "1.5")
                .game
                .unwrap()
                .contains("gui_scale=1.500000")
        );
        // windowed_resolution is only touched when present
        let with = Files {
            game: None,
            pdx: Some(PDX.replace(
                "\"vsync\"",
                "\"windowed_resolution\"={ value=\"800x600\" }\n\"vsync\"",
            )),
        };
        let e = field("resolution")
            .unwrap()
            .plan("1280x720", &with)
            .unwrap();
        assert_eq!(e.len(), 2);
        // a key in only one file: only that file
        let one = Files {
            game: Some(GAME.into()),
            pdx: Some(PDX.replace("\"vsync\"", "\"vsync_x\"")),
        };
        let e = field("vsync").unwrap().plan("no", &one).unwrap();
        assert!(e.iter().all(|e| e.file == File::Game) && e.len() == 1);
        // bad input
        for (id, v) in [
            ("resolution", "big"),
            ("resolution", "0x5"),
            ("vsync", "maybe"),
            ("master-volume", "101"),
            ("ui-scale", "0.1"),
            ("autosave", "-1"),
            ("display-mode", "full"),
        ] {
            assert!(field(id).unwrap().plan(v, &f).is_err(), "{id} {v}");
        }
        assert!(
            field("autosave")
                .unwrap()
                .plan("2", &Files::default())
                .is_err()
        );
    }

    #[test]
    fn backup_names_do_not_collide() {
        let b = tmp_dir("set-names");
        let root = b.join("x");
        let a = new_dir(&root, 1000).unwrap();
        let c = new_dir(&root, 1000).unwrap();
        let d = new_dir(&root, 1000).unwrap();
        assert_eq!(
            [a, c, d].map(|p| p.file_name().unwrap().to_string_lossy().into_owned()),
            ["1000", "1001", "1002"]
        );
    }

    #[test]
    fn write_backs_up_and_rolls_back_undoably() {
        let b = tmp_dir("set-write");
        let (i, roots) = install(&b);
        let (g0, p0) = (crlf(GAME), crlf(PDX));
        put(&path(&i, File::Game), &g0);
        put(&path(&i, File::Pdx), &p0);
        let old = files(&i);
        let new = edit(&old, &field("vsync").unwrap().plan("no", &old).unwrap()).unwrap();
        let dir = write(&i, &roots, &old, &new).unwrap().unwrap();
        // originals are in the backup byte for byte; live files carry the change, CRLF intact
        assert_eq!(fs::read_to_string(dir.join("settings.txt")).unwrap(), g0);
        assert_eq!(
            fs::read_to_string(dir.join("pdx_settings.txt")).unwrap(),
            p0
        );
        let (g1, p1) = (
            fs::read_to_string(path(&i, File::Game)).unwrap(),
            fs::read_to_string(path(&i, File::Pdx)).unwrap(),
        );
        assert_eq!(g1, g0.replace("vsync=yes", "vsync=no"));
        assert_eq!(p1, p0.replace("enabled=yes", "enabled=no"));
        // nothing to change: no new backup
        assert!(write(&i, &roots, &files(&i), &files(&i)).unwrap().is_none());
        assert_eq!(backups(&i, &roots).len(), 1);
        // rollback restores, and is itself backed up
        let (r, _) = rollback_by_id(&i, &roots, backups(&i, &roots)[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(r.files, vec![File::Game, File::Pdx]);
        assert_eq!(fs::read_to_string(path(&i, File::Game)).unwrap(), g0);
        assert_eq!(fs::read_to_string(path(&i, File::Pdx)).unwrap(), p0);
        assert_eq!(backups(&i, &roots).len(), 2);
        // roll back the rollback
        rollback_by_id(&i, &roots, backups(&i, &roots)[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read_to_string(path(&i, File::Game)).unwrap(), g1);
        assert_eq!(fs::read_to_string(path(&i, File::Pdx)).unwrap(), p1);
        assert!(rollback_by_id(&i, &roots, u64::MAX).unwrap().is_none());
        // stale: the game rewrote the file after we loaded it
        let stale = files(&i);
        fs::write(path(&i, File::Game), "x=1\n").unwrap();
        let again = edit(
            &stale,
            &field("vsync").unwrap().plan("yes", &stale).unwrap(),
        )
        .unwrap();
        assert!(write(&i, &roots, &stale, &again).is_err());
        assert_eq!(fs::read_to_string(path(&i, File::Game)).unwrap(), "x=1\n");
    }

    #[test]
    fn only_existing_files_with_the_key_are_written() {
        let b = tmp_dir("set-only");
        let (i, roots) = install(&b);
        // settings.txt only
        put(&path(&i, File::Game), GAME);
        let old = files(&i);
        let new = edit(
            &old,
            &field("language").unwrap().plan("l_german", &old).unwrap(),
        )
        .unwrap();
        let dir = write(&i, &roots, &old, &new).unwrap().unwrap();
        assert!(!path(&i, File::Pdx).exists());
        assert!(dir.join("settings.txt").is_file() && !dir.join("pdx_settings.txt").exists());
        // pdx without the key stays byte identical on disk; backup captures consistent snapshot of both
        let pdx = PDX.replace("\"vsync\"", "\"vsync_x\"");
        put(&path(&i, File::Pdx), &pdx);
        let old = files(&i);
        let new = edit(&old, &field("vsync").unwrap().plan("no", &old).unwrap()).unwrap();
        let dir = write(&i, &roots, &old, &new).unwrap().unwrap();
        assert_eq!(fs::read_to_string(path(&i, File::Pdx)).unwrap(), pdx);
        assert!(dir.join("pdx_settings.txt").is_file() && dir.join(MANIFEST_NAME).is_file());
        // a broken text is refused before anything is written
        let n = backups(&i, &roots).len();
        let mut bad = files(&i);
        bad.game = bad.game.map(|t| format!("{t}\nx=\"oops"));
        assert!(write(&i, &roots, &files(&i), &bad).is_err());
        assert_eq!(backups(&i, &roots).len(), n);
    }

    #[cfg(unix)]
    #[test]
    fn permissions_survive_write_and_rollback() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp_dir("set-perm");
        let (i, roots) = install(&b);
        let p = path(&i, File::Game);
        put(&p, GAME);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        let old = files(&i);
        let new = edit(&old, &field("autosave").unwrap().plan("2", &old).unwrap()).unwrap();
        write(&i, &roots, &old, &new).unwrap();
        let mode = || fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o600);
        rollback_by_id(&i, &roots, backups(&i, &roots)[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(mode(), 0o600);
    }

    #[test]
    fn keeps_the_newest_twenty() {
        let b = tmp_dir("set-prune");
        let (i, roots) = install(&b);
        put(&path(&i, File::Game), GAME);
        for n in 0..25u64 {
            let d = backup_root(&i, &roots).join((1000 + n).to_string());
            put(&d.join("settings.txt"), "a=1");
        }
        backup(&i, &roots, &[File::Game]).unwrap();
        let ids: Vec<u64> = backups(&i, &roots).iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), KEEP);
        assert_eq!(ids.last(), Some(&1006));
    }

    #[test]
    fn language_options_from_layout() {
        let b = tmp_dir("set-lang");
        let (i, _) = install(&b);
        assert!(language_options(&i).is_empty());
        put(
            &i.settings_layout,
            r#"[{"category":"Graphics","settings":[{"name":"vsync"}]},
                {"category":"System","settings":[{"name":"language","options":[
                 {"label":"English","value":"l_english"},{"label":"中文","value":"l_simp_chinese"}]}]}]"#,
        );
        assert_eq!(
            language_options(&i),
            [
                ("English".into(), "l_english".into()),
                ("中文".into(), "l_simp_chinese".into())
            ]
        );
    }

    #[test]
    fn atomic_backup_ignores_staging_and_two_file_apply_restores_on_second_failure() {
        let b = tmp_dir("set-atomic");
        let (i, roots) = install(&b);
        put(&path(&i, File::Game), GAME);
        put(&path(&i, File::Pdx), PDX);
        // Incomplete staging directory in backup_root is never listed as a valid backup.
        let staging = backup_root(&i, &roots).join(".tmp-123-0");
        put(&staging.join("settings.txt"), GAME);
        assert!(backups(&i, &roots).is_empty());

        // Inject a failure into the SECOND commit step. The hook records what the first target
        // holds at that moment, proving the first rename really had been committed.
        let orig_game = fs::read_to_string(path(&i, File::Game)).unwrap();
        let orig_pdx = fs::read_to_string(path(&i, File::Pdx)).unwrap();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let (seen2, game_path) = (seen.clone(), path(&i, File::Game));
        COMMIT_HOOK.with(|h| {
            *h.borrow_mut() = Some((
                1,
                Box::new(move || *seen2.borrow_mut() = fs::read_to_string(&game_path).unwrap()),
            ))
        });
        let ops = || {
            vec![
                (
                    File::Game,
                    path(&i, File::Game),
                    TargetAction::Write {
                        bytes: b"changed=1\n".to_vec(),
                        perms: None,
                    },
                ),
                (
                    File::Pdx,
                    path(&i, File::Pdx),
                    TargetAction::Write {
                        bytes: b"changed=2\n".to_vec(),
                        perms: None,
                    },
                ),
            ]
        };
        let err = apply_all_or_nothing(ops(), &b);
        COMMIT_HOOK.with(|h| *h.borrow_mut() = None);
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("injected commit failure"), "{msg}");
        assert_eq!(*seen.borrow(), "changed=1\n", "first commit must have run");
        assert_eq!(
            fs::read_to_string(path(&i, File::Game)).unwrap(),
            orig_game,
            "first file must be restored if second commit fails"
        );
        assert_eq!(fs::read_to_string(path(&i, File::Pdx)).unwrap(), orig_pdx);
    }

    #[cfg(unix)]
    #[test]
    fn failed_restore_after_second_commit_failure_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp_dir("set-restore-fail");
        let (i, _) = install(&b);
        put(&path(&i, File::Game), GAME);
        put(&path(&i, File::Pdx), PDX);
        let (gp, pp) = (path(&i, File::Game), path(&i, File::Pdx));
        let lock_dir = gp.parent().unwrap().to_path_buf();
        // Failing commit #1 also makes the first file's directory read-only: restoring it cannot work.
        COMMIT_HOOK.with(|h| {
            let d = lock_dir.clone();
            *h.borrow_mut() = Some((
                1,
                Box::new(move || {
                    fs::set_permissions(&d, Permissions::from_mode(0o555)).unwrap();
                }),
            ))
        });
        let res = apply_all_or_nothing(
            vec![
                (
                    File::Game,
                    gp.clone(),
                    TargetAction::Write {
                        bytes: b"changed=1\n".to_vec(),
                        perms: None,
                    },
                ),
                (
                    File::Pdx,
                    pp,
                    TargetAction::Write {
                        bytes: b"changed=2\n".to_vec(),
                        perms: None,
                    },
                ),
            ],
            Path::new("/the/backup"),
        );
        COMMIT_HOOK.with(|h| *h.borrow_mut() = None);
        fs::set_permissions(&lock_dir, Permissions::from_mode(0o755)).unwrap();
        let msg = format!("{:#}", res.unwrap_err());
        assert!(
            msg.contains("could not restore")
                && msg.contains("settings.txt")
                && msg.contains("/the/backup"),
            "{msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rollback_tracks_both_files_absence_and_mode() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp_dir("set-absence");
        let (i, roots) = install(&b);
        let (gp, pp) = (path(&i, File::Game), path(&i, File::Pdx));
        put(&gp, GAME);
        put(&pp, PDX);
        fs::set_permissions(&pp, Permissions::from_mode(0o600)).unwrap();

        // Snapshot 1: both exist (vsync=yes, pdx mode 0o600).
        let old = files(&i);
        let new = edit(&old, &field("autosave").unwrap().plan("2", &old).unwrap()).unwrap();
        write(&i, &roots, &old, &new).unwrap();
        let snap1_id = backups(&i, &roots)[0].id;

        // Now delete pdx_settings.txt and edit settings.txt again.
        fs::remove_file(&pp).unwrap();
        let old2 = files(&i);
        let new2 = edit(&old2, &field("autosave").unwrap().plan("3", &old2).unwrap()).unwrap();
        write(&i, &roots, &old2, &new2).unwrap();

        // Roll back to snap1 -> recreates pdx_settings.txt with mode 0o600, and creates an undo backup recording pdx absent.
        let (_, undo_dir) = rollback_by_id(&i, &roots, snap1_id).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&pp).unwrap(), PDX);
        assert_eq!(
            fs::metadata(&pp).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let undo_id: u64 = undo_dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();

        // Roll back to undo backup -> deletes recreated pdx_settings.txt!
        rollback_by_id(&i, &roots, undo_id).unwrap().unwrap();
        assert!(
            !pp.exists(),
            "rolling back to state where pdx_settings.txt was absent must remove it"
        );
        assert!(fs::read_to_string(&gp).unwrap().contains("autosave=3"));
    }
}
