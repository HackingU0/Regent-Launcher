//! Saves and which playset each was played with. UI-free; roots are injected.
//!
//! Binding rule: a save is bound to the current dlc_load.json iff it was written after that file
//! and after the previous sweep. The first sweep binds nothing, so every pre-existing save stays
//! "unknown" until the user binds it by hand or plays it again.
use crate::game::{Install, Roots};
use crate::mods::{Dlc, Mod, Value, contained_file, parse_pdx};
use crate::playset::{
    DlcLoad, Entry, GameState, Playset, app_dir, dlc_load, lock_app, read_dlc_load, write_atomic,
};
use crate::zip;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub name: String,
    /// In-game date.
    pub date: String,
    pub version: String,
}

#[derive(Clone, Debug)]
pub struct Save {
    /// Key in saves.json: path under `save games/` (Steam Cloud copies get a `steam:<user>/` prefix).
    pub rel: String,
    pub path: PathBuf,
    /// Unix milliseconds.
    pub mtime: u64,
    pub meta: Option<Meta>,
}

impl Save {
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Empire name from the save's meta, else the file stem.
    pub fn title(&self) -> String {
        match &self.meta {
            Some(m) if !m.name.is_empty() => m.name.clone(),
            _ => self.stem(),
        }
    }
}

/// A save plus the dlc_load.json it was played with, if known.
#[derive(Clone)]
pub struct Bound {
    pub save: Save,
    pub snap: Option<DlcLoad>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Binding {
    Unknown,
    /// Name of the playset whose dlc_load equals the snapshot.
    Playset(String),
    /// A snapshot no playset matches; the number of mods in it.
    Custom(usize),
}

impl std::fmt::Display for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Binding::Unknown => f.write_str("unknown"),
            Binding::Playset(n) => f.write_str(n),
            Binding::Custom(n) => write!(f, "custom ({n} mods)"),
        }
    }
}

// ---------------------------------------------------------------- scanning

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn read_dir_checked(p: &Path) -> Result<Option<Vec<PathBuf>>, std::io::Error> {
    match std::fs::read_dir(p) {
        Ok(rd) => {
            let mut v = vec![];
            for e in rd {
                v.push(e?.path());
            }
            v.sort();
            Ok(Some(v))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// (directory, rel prefix) of every place saves live: Documents and Steam Cloud, plus the
/// prefixes whose parent could not be read (I/O error, not just absent).
fn save_roots_checked(install: &Install, roots: &Roots) -> (Vec<(PathBuf, String)>, Vec<String>) {
    let mut v = vec![(install.data_dir.join("save games"), String::new())];
    let mut failed = vec![];
    for steam in &roots.steam_roots {
        match read_dir_checked(&steam.join("userdata")) {
            Ok(Some(users)) => {
                for user in users.into_iter().filter(|u| u.is_dir()) {
                    let dir = user
                        .join(install.def.steam_app.to_string())
                        .join("remote/save games");
                    let who = user.file_name().unwrap_or_default().to_string_lossy();
                    v.push((dir, format!("steam:{who}/")));
                }
            }
            Ok(None) => {}
            Err(_) => failed.push("steam:".to_string()),
        }
    }
    (v, failed)
}

fn is_save(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["sav", "eu4", "hoi4", "ck3"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

/// Saves at depth 1 and 2 (`save games/<empire>/<date>.sav`), newest first, plus prefixes whose
/// directory could not be read (non-NotFound I/O error) and prefixes whose root is absent
/// (unmounted or never created: their recorded bindings must be kept, not pruned).
fn scan_checked(install: &Install, roots: &Roots) -> (Vec<Save>, Vec<String>, Vec<String>) {
    let mut out = vec![];
    let mut missing = vec![];
    let (roots_list, mut failed) = save_roots_checked(install, roots);
    // ponytail: any absent Steam userdata keeps every `steam:` binding; refine per library if it matters.
    if roots
        .steam_roots
        .iter()
        .any(|s| !s.join("userdata").is_dir())
    {
        missing.push("steam:".to_string());
    }
    for (root, prefix) in roots_list {
        let entries = match read_dir_checked(&root) {
            Ok(Some(v)) => v,
            Ok(None) => {
                missing.push(prefix);
                continue;
            }
            Err(_) => {
                failed.push(prefix);
                continue;
            }
        };
        let mut files = vec![];
        for p in entries {
            if p.is_dir() {
                match read_dir_checked(&p) {
                    Ok(Some(sub)) => files.extend(sub),
                    Ok(None) => {}
                    Err(_) => {
                        let sub_rel = p
                            .strip_prefix(&root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .replace('\\', "/");
                        failed.push(format!("{prefix}{sub_rel}/"));
                    }
                }
            } else {
                files.push(p);
            }
        }
        for p in files.into_iter().filter(|p| is_save(p)) {
            let rel_s = p
                .strip_prefix(&root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let full_rel = format!("{prefix}{rel_s}");
            match std::fs::metadata(&p) {
                Ok(m) if m.is_file() => match m.modified() {
                    Ok(mtime) => out.push(Save {
                        rel: full_rel,
                        path: p,
                        mtime: ms(mtime),
                        meta: None,
                    }),
                    Err(_) => failed.push(full_rel),
                },
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(_) => failed.push(full_rel),
            }
        }
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.rel.cmp(&b.rel)));
    (out, failed, missing)
}

/// Whether the save key `rel` lives under the root identified by `prefix` ("" = Documents).
fn under_prefix(rel: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        !rel.starts_with("steam:")
    } else {
        rel.starts_with(prefix)
    }
}

/// Saves at depth 1 and 2 (`save games/<empire>/<date>.sav`), newest first. No meta.
pub fn scan(install: &Install, roots: &Roots) -> Vec<Save> {
    scan_checked(install, roots).0
}

/// Locate a single save by its `rel` key without scanning the save collection or reading zip meta.
/// Validates relative path depth (1..=2), save extension, non-symlink and root containment.
pub fn find_one(install: &Install, roots: &Roots, rel: &str) -> Option<Save> {
    let (root, inner) = if let Some(rest) = rel.strip_prefix("steam:") {
        let (who, tail) = rest.split_once('/')?;
        let mut who_c = Path::new(who).components();
        if !matches!(
            (who_c.next(), who_c.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return None;
        }
        let dir = roots.steam_roots.iter().find_map(|steam| {
            let d = steam
                .join("userdata")
                .join(who)
                .join(install.def.steam_app.to_string())
                .join("remote/save games");
            d.is_dir().then_some(d)
        })?;
        (dir, tail)
    } else {
        (install.data_dir.join("save games"), rel)
    };
    let comps: Vec<_> = Path::new(inner).components().collect();
    if !(1..=2).contains(&comps.len()) || !comps.iter().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let path = root.join(inner);
    if !is_save(&path) {
        return None;
    }
    contained_file(&root, &path)?;
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
    let mtime = ms(meta.modified().ok()?);
    Some(Save {
        rel: rel.to_string(),
        path,
        mtime,
        meta: None,
    })
}

/// `meta` of a zip save; None for text saves or anything unreadable.
pub fn read_meta(path: &Path) -> Option<Meta> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 2];
    f.read_exact(&mut head).ok()?;
    if &head != b"PK" {
        return None;
    }
    f.seek(SeekFrom::Start(0)).ok()?;
    let es = zip::entries(&mut f).ok()?;
    let e = es.iter().find(|e| e.name == "meta")?;
    let text = String::from_utf8_lossy(&zip::read(&mut f, e, 64 * 1024).ok()?).into_owned();
    let mut m = Meta::default();
    for (k, v) in parse_pdx(&text) {
        let Value::Scalar(v) = v else { continue };
        match k.as_str() {
            "name" => m.name = v,
            "date" => m.date = v,
            "version" => m.version = v,
            _ => {}
        }
    }
    Some(m)
}

/// Every save with its meta, newest first.
pub fn list(install: &Install, roots: &Roots) -> Vec<Save> {
    let mut v = scan(install, roots);
    for s in &mut v {
        s.meta = read_meta(&s.path);
    }
    v
}

// ---------------------------------------------------------------- saves.json

#[derive(Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    games: BTreeMap<String, Log>,
}

#[derive(Default, Serialize, Deserialize)]
struct Log {
    /// Unix ms of the last sweep.
    #[serde(default)]
    swept_at: u64,
    #[serde(default)]
    snapshots: Vec<DlcLoad>,
    #[serde(default)]
    saves: BTreeMap<String, Rec>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Rec {
    mtime: u64,
    snap: Option<usize>,
}

fn book_path(roots: &Roots) -> PathBuf {
    app_dir(roots).join("saves.json")
}

impl Book {
    /// Read-only: never renames or writes. Corrupt or unreadable books return an empty book plus a warning.
    fn peek(path: &Path) -> (Book, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Book>(&text) {
                Ok(book) => (book, None),
                Err(e) => (
                    Book::default(),
                    Some(format!(
                        "{} is corrupt ({e}); save bindings unavailable",
                        path.display()
                    )),
                ),
            },
            Err(e) if e.kind() == ErrorKind::NotFound => (Book::default(), None),
            Err(e) => (
                Book::default(),
                Some(format!(
                    "Could not read {} ({e}); save bindings unavailable",
                    path.display()
                )),
            ),
        }
    }

    /// Strict load for mutations: missing -> empty; corrupt or unreadable -> error (never renames).
    fn load(path: &Path) -> Result<Book> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Book::default()),
            Err(e) => return Err(e.into()),
        };
        serde_json::from_str(&text)
            .with_context(|| format!("{} is corrupt; refusing to overwrite it", path.display()))
    }

    fn store(&self, path: &Path) -> Result<()> {
        write_atomic(path, &serde_json::to_string_pretty(self)?)
    }
}

/// Read-only check that `saves.json` is missing or parses cleanly (never writes or locks).
pub fn check_book_readable(roots: &Roots) -> Result<()> {
    Book::load(&book_path(roots)).map(|_| ())
}

fn intern(snaps: &mut Vec<DlcLoad>, s: &DlcLoad) -> usize {
    snaps.iter().position(|x| x == s).unwrap_or_else(|| {
        snaps.push(s.clone());
        snaps.len() - 1
    })
}

/// Record every save and bind the ones written since the last sweep and the last dlc_load.json
/// write to the current dlc_load.json. Prunes deleted saves (except under roots that failed to scan).
pub fn sweep(install: &Install, roots: &Roots) -> Result<()> {
    let _lock = lock_app(roots)?;
    let path = book_path(roots);
    let mut book = Book::load(&path)?;
    let id = install.def.id;
    let first = !book.games.contains_key(id);
    let log = book.games.entry(id.into()).or_default();
    // Capture start watermark before scanning so saves written mid-scan are not missed next sweep.
    let started_at = ms(SystemTime::now());
    let written = std::fs::metadata(install.data_dir.join("dlc_load.json"))
        .and_then(|m| m.modified())
        .ok()
        .map(ms);
    let current = read_dlc_load(&install.data_dir);
    let (scanned, failed_prefixes, missing_prefixes) = scan_checked(install, roots);
    // An unreadable location is not "no saves": recording now would advance the watermark past
    // saves we could not see, so they could never be bound. Nothing is written.
    if !failed_prefixes.is_empty() {
        let shown: Vec<String> = failed_prefixes
            .iter()
            .map(|p| {
                if p.is_empty() {
                    "save games".into()
                } else {
                    p.clone()
                }
            })
            .collect();
        bail!(
            "could not read save location(s): {}; fix permissions or the mount and try again",
            shown.join(", ")
        );
    }
    let mut recs = BTreeMap::new();
    for s in scanned {
        let snap = match log.saves.get(&s.rel) {
            Some(r) if r.mtime == s.mtime => r.snap,
            _ => match (&current, written) {
                (Some(c), Some(w)) if !first && s.mtime > w && s.mtime > log.swept_at => {
                    Some(intern(&mut log.snapshots, c))
                }
                _ => None,
            },
        };
        recs.insert(
            s.rel,
            Rec {
                mtime: s.mtime,
                snap,
            },
        );
    }
    // Roots that are absent (not unreadable) keep their bindings: the drive may just be unmounted.
    for (rel, rec) in &log.saves {
        if !recs.contains_key(rel) && missing_prefixes.iter().any(|p| under_prefix(rel, p)) {
            recs.insert(rel.clone(), rec.clone());
        }
    }
    log.saves = recs;
    log.swept_at = started_at.max(log.swept_at);
    book.store(&path)
}

/// Saves with their recorded bindings, newest first, plus any corrupt-book warning.
/// Strictly read-only: never writes or renames `saves.json`.
pub fn bound_peek(install: &Install, roots: &Roots) -> (Vec<Bound>, Option<String>) {
    let (book, warning) = Book::peek(&book_path(roots));
    let log = book.games.get(install.def.id);
    let rows = list(install, roots)
        .into_iter()
        .map(|save| {
            let snap = log
                .and_then(|l| l.saves.get(&save.rel).filter(|r| r.mtime == save.mtime))
                .and_then(|r| r.snap)
                .and_then(|i| log?.snapshots.get(i).cloned());
            Bound { save, snap }
        })
        .collect();
    (rows, warning)
}

/// Look up the recorded binding for a single already-located save using `Book::peek` (read-only).
pub fn bound_one_peek(install: &Install, roots: &Roots, save: Save) -> (Bound, Option<String>) {
    let (book, warning) = Book::peek(&book_path(roots));
    let log = book.games.get(install.def.id);
    let snap = log
        .and_then(|l| l.saves.get(&save.rel).filter(|r| r.mtime == save.mtime))
        .and_then(|r| r.snap)
        .and_then(|i| log?.snapshots.get(i).cloned());
    (Bound { save, snap }, warning)
}

/// Hand-fix a save's binding to `snap` (the current dlc_load).
pub fn bind(install: &Install, roots: &Roots, rel: &str, snap: &DlcLoad) -> Result<()> {
    let _lock = lock_app(roots)?;
    sweep(install, roots)?;
    let path = book_path(roots);
    let mut book = Book::load(&path)?;
    let log = book.games.entry(install.def.id.into()).or_default();
    let Some(r) = log.saves.get(rel).cloned() else {
        bail!("no such save: {rel}");
    };
    let i = intern(&mut log.snapshots, snap);
    log.saves.insert(rel.into(), Rec { snap: Some(i), ..r });
    book.store(&path)
}

// ---------------------------------------------------------------- playing

/// Playsets matching `snap`: the active one first.
fn matching(g: &GameState, snap: &DlcLoad, mods: &[Mod], dlcs: &[Dlc]) -> Option<usize> {
    let hit = |i: &usize| dlc_load(&g.playsets[*i], mods, dlcs) == *snap;
    let active = g.active.min(g.playsets.len().saturating_sub(1));
    (g.playsets.len() > active && hit(&active))
        .then_some(active)
        .or_else(|| (0..g.playsets.len()).find(hit))
}

pub fn binding(b: &Bound, g: &GameState, mods: &[Mod], dlcs: &[Dlc]) -> Binding {
    let Some(snap) = &b.snap else {
        return Binding::Unknown;
    };
    match matching(g, snap, mods, dlcs) {
        Some(i) => Binding::Playset(g.playsets[i].name.clone()),
        None => Binding::Custom(snap.enabled_mods.len()),
    }
}

pub enum Plan {
    /// Switch to this playset.
    Use(usize),
    /// Create this playset from the snapshot; the ids in the snapshot that are not installed.
    Make(Playset, Vec<String>),
    /// No snapshot: launch with whatever playset is current.
    Unknown,
}

impl Plan {
    /// Playing needs an explicit yes (`--force`, GUI confirm).
    pub fn risky(&self) -> bool {
        match self {
            Plan::Use(_) => false,
            Plan::Make(_, missing) => !missing.is_empty(),
            Plan::Unknown => true,
        }
    }
}

pub fn plan(b: &Bound, g: &GameState, mods: &[Mod], dlcs: &[Dlc]) -> Plan {
    let Some(snap) = &b.snap else {
        return Plan::Unknown;
    };
    if let Some(i) = matching(g, snap, mods, dlcs) {
        return Plan::Use(i);
    }
    let (have, missing): (Vec<_>, Vec<_>) = snap
        .enabled_mods
        .iter()
        .partition(|id| mods.iter().any(|m| &m.id == *id));
    let mut p = Playset::new(&format!("For {}", b.save.stem()));
    p.mods = have
        .into_iter()
        .map(|id| Entry {
            id: id.clone(),
            enabled: true,
        })
        .collect();
    p.disabled_dlcs = snap.disabled_dlcs.clone();
    p.reconcile(mods);
    Plan::Make(p, missing.into_iter().cloned().collect())
}

/// Make the plan's playset active. A forced `Make` that was already created earlier is reused.
pub fn adopt(g: &mut GameState, plan: Plan, mods: &[Mod], dlcs: &[Dlc]) {
    match plan {
        Plan::Use(i) => g.active = i,
        Plan::Unknown => {}
        Plan::Make(p, _) => {
            let want = dlc_load(&p, mods, dlcs);
            let same = g
                .playsets
                .iter()
                .position(|q| q.name == p.name && dlc_load(q, mods, dlcs) == want);
            match same {
                Some(i) => g.active = i,
                None => g.add(p),
            }
        }
    }
}

/// `-continuelastsave` only when the game's continue_game.json names exactly one save among
/// `all` (every save of every root) and that save is `save`. Anything ambiguous launches plain.
pub fn continue_arg(install: &Install, save: &Save, all: &[Save]) -> Option<&'static str> {
    let text = std::fs::read_to_string(install.data_dir.join("continue_game.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let obj = v.as_object()?;
    let raw = ["filename", "save_game", "save"]
        .iter()
        .find_map(|k| obj.get(*k)?.as_str())?
        .trim();
    if raw.is_empty() {
        return None;
    }
    let norm = raw.replace('\\', "/");
    let target = norm
        .strip_prefix("save games/")
        .or_else(|| norm.strip_prefix("Save Games/"))
        .unwrap_or(&norm);
    let is_remote = obj.get("is_remote").and_then(|x| x.as_bool());
    let names = |s: &Save| {
        let steam = s.rel.starts_with("steam:");
        if is_remote.is_some_and(|r| r != steam) {
            return false;
        }
        let local_rel = s
            .rel
            .strip_prefix("steam:")
            .and_then(|r| r.split_once('/').map(|(_, t)| t))
            .unwrap_or(&s.rel);
        if target.contains('/') {
            local_rel.eq_ignore_ascii_case(target)
        } else {
            s.path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(target))
        }
    };
    let mut hits = all.iter().filter(|s| names(s));
    (names(save) && hits.next().is_some_and(|h| h.rel == save.rel) && hits.next().is_none())
        .then_some("-continuelastsave")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::{Channel, GAMES, put, tmp_dir};
    use std::time::Duration;

    /// Stored-method zip of (name, data) pairs.
    pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut cd) = (vec![], vec![]);
        for (name, data) in files {
            let off = out.len() as u32;
            let crc_etc = |v: &mut Vec<u8>| {
                for x in [0u32, data.len() as u32, data.len() as u32] {
                    v.extend(x.to_le_bytes());
                }
            };
            out.extend(b"PK\x03\x04");
            for x in [20u16, 0, 0, 0, 0] {
                out.extend(x.to_le_bytes());
            }
            crc_etc(&mut out);
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend(*data);
            cd.extend(b"PK\x01\x02");
            for x in [20u16, 20, 0, 0, 0, 0] {
                cd.extend(x.to_le_bytes());
            }
            crc_etc(&mut cd);
            cd.extend((name.len() as u16).to_le_bytes());
            cd.extend([0u8; 12]);
            cd.extend(off.to_le_bytes());
            cd.extend(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend(&cd);
        out.extend(b"PK\x05\x06\0\0\0\0");
        for x in [files.len() as u16, files.len() as u16] {
            out.extend(x.to_le_bytes());
        }
        out.extend((cd.len() as u32).to_le_bytes());
        out.extend(cd_off.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    pub(crate) fn sav(name: &str, date: &str) -> Vec<u8> {
        let meta = format!("version=\"Gemini v4.6.0\"\nname=\"{name}\"\ndate=\"{date}\"\n");
        zip_of(&[("meta", meta.as_bytes()), ("gamestate", b"xxxx")])
    }

    pub(crate) fn fixture(tag: &str) -> (Roots, Install, PathBuf) {
        let b = tmp_dir(tag);
        let roots = Roots {
            data: b.join("appdata"),
            steam_roots: vec![b.join("steam")],
            ..Default::default()
        };
        let i = Install {
            def: &GAMES[0],
            channel: Channel::Steam,
            dir: b.join("game"),
            launcher_dir: b.join("game"),
            theme_file: b.join("game/assets/theme-settings.json"),
            settings_layout: b.join("game/settings-layout.json"),
            workshop: None,
            version: None,
            raw_version: None,
            data_dir: b.join("data"),
            exe: PathBuf::from("/usr/bin/true"),
            exe_args: vec![],
        };
        (roots, i, b)
    }

    pub(crate) fn put_save(i: &Install, rel: &str, bytes: &[u8], at: SystemTime) {
        let p = i.data_dir.join("save games").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    pub(crate) fn dl(i: &Install, mods: &[&str], dis: &[&str]) {
        let l = DlcLoad {
            enabled_mods: mods.iter().map(|s| s.to_string()).collect(),
            disabled_dlcs: dis.iter().map(|s| s.to_string()).collect(),
        };
        write_atomic(
            &i.data_dir.join("dlc_load.json"),
            &serde_json::to_string(&l).unwrap(),
        )
        .unwrap();
    }

    fn mod_(id: &str) -> Mod {
        Mod {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        }
    }

    fn dlc_(id: &str) -> Dlc {
        Dlc {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        }
    }

    fn snap_of(i: &Install, roots: &Roots, rel: &str) -> Option<DlcLoad> {
        bound_peek(i, roots)
            .0
            .into_iter()
            .find(|b| b.save.rel == rel)
            .unwrap()
            .snap
    }

    #[test]
    fn meta_from_zip() {
        let (_, i, _) = fixture("meta");
        put_save(
            &i,
            "e/a.sav",
            &sav("Terran", "2230.05.01"),
            SystemTime::now(),
        );
        put_save(&i, "e/text.sav", b"not a zip", SystemTime::now());
        let m = read_meta(&i.data_dir.join("save games/e/a.sav")).unwrap();
        assert_eq!(
            m,
            Meta {
                name: "Terran".into(),
                date: "2230.05.01".into(),
                version: "Gemini v4.6.0".into()
            }
        );
        assert!(read_meta(&i.data_dir.join("save games/e/text.sav")).is_none());
        assert!(read_meta(&i.data_dir.join("save games/e/none.sav")).is_none());
    }

    #[test]
    #[ignore = "reads real Steam install; run with --ignored"]
    fn real_tutorial_meta() {
        let game = crate::game::discover(&Roots::system())
            .into_iter()
            .find(|i| i.def.id == "stellaris")
            .expect("Stellaris install present");
        let p = game.dir.join("curated_save_games/tutorial.sav");
        let m = read_meta(&p).expect("meta of the tutorial save");
        assert!(!m.version.is_empty() && !m.date.is_empty());
    }

    #[test]
    fn binding_rule() {
        let (roots, i, _) = fixture("bind");
        let now = SystemTime::now();
        let ago = |s| now - Duration::from_secs(s);
        let ahead = |s| now + Duration::from_secs(s);
        dl(&i, &["mod/a.mod"], &[]);
        // Saves that exist at the first sweep stay unknown, even if newer than dlc_load.json.
        put_save(&i, "e/old.sav", &sav("E", "2200.01.01"), ahead(5));
        sweep(&i, &roots).unwrap();
        assert!(snap_of(&i, &roots, "e/old.sav").is_none());

        // A save written after a dlc_load.json write binds to it.
        dl(&i, &["mod/a.mod", "mod/b.mod"], &["dlc/x.dlc"]);
        put_save(&i, "e/new.sav", &sav("E", "2201.01.01"), ahead(10));
        // A copy with an old mtime (restored backup, cloud sync) stays unknown.
        put_save(&i, "f/copy.sav", &sav("F", "2100.01.01"), ago(86_400));
        sweep(&i, &roots).unwrap();
        let s = snap_of(&i, &roots, "e/new.sav").unwrap();
        assert_eq!(s.enabled_mods, ["mod/a.mod", "mod/b.mod"]);
        assert_eq!(s.disabled_dlcs, ["dlc/x.dlc"]);
        assert!(snap_of(&i, &roots, "f/copy.sav").is_none());
        assert!(snap_of(&i, &roots, "e/old.sav").is_none());

        // The binding survives later sweeps and a dlc_load.json change.
        dl(&i, &["mod/c.mod"], &[]);
        sweep(&i, &roots).unwrap();
        assert!(snap_of(&i, &roots, "e/new.sav").unwrap() == s);

        // Saved again after the newer dlc_load: rebinds.
        put_save(&i, "e/new.sav", &sav("E", "2201.02.01"), ahead(20));
        sweep(&i, &roots).unwrap();
        assert_eq!(
            snap_of(&i, &roots, "e/new.sav").unwrap().enabled_mods,
            ["mod/c.mod"]
        );

        // Hand binding.
        let cur = read_dlc_load(&i.data_dir).unwrap();
        bind(&i, &roots, "f/copy.sav", &cur).unwrap();
        assert!(snap_of(&i, &roots, "f/copy.sav").unwrap() == cur);
        assert!(bind(&i, &roots, "nope.sav", &cur).is_err());

        // Deleted saves are pruned.
        std::fs::remove_file(i.data_dir.join("save games/f/copy.sav")).unwrap();
        sweep(&i, &roots).unwrap();
        let text = std::fs::read_to_string(book_path(&roots)).unwrap();
        assert!(!text.contains("copy.sav") && text.contains("new.sav"));
    }

    #[test]
    fn no_dlc_load_binds_nothing() {
        let (roots, i, _) = fixture("nodl");
        sweep(&i, &roots).unwrap();
        put_save(
            &i,
            "e/a.sav",
            &sav("E", "d"),
            SystemTime::now() + Duration::from_secs(9),
        );
        sweep(&i, &roots).unwrap();
        assert!(snap_of(&i, &roots, "e/a.sav").is_none());
    }

    #[test]
    fn steam_cloud_saves_listed() {
        let (roots, i, b) = fixture("cloud");
        let p = b.join("steam/userdata/42/281990/remote/save games/e/c.sav");
        put(&p, "x");
        put(&i.data_dir.join("save games/e/d.txt"), "ignored");
        let rels: Vec<_> = list(&i, &roots).into_iter().map(|s| s.rel).collect();
        assert_eq!(rels, ["steam:42/e/c.sav"]);
    }

    #[test]
    fn matching_includes_disabled_dlcs() {
        let mods = [mod_("m1"), mod_("m2")];
        let dlcs = [dlc_("d1")];
        let mut a = Playset::new("A");
        a.reconcile(&mods);
        a.toggle("m1");
        let mut b = a.clone();
        b.name = "B".into();
        b.disabled_dlcs = vec!["d1".into()];
        let g = GameState {
            active: 0,
            playsets: vec![a, b],
            ..Default::default()
        };
        let mk = |dis: &[&str]| Bound {
            save: Save {
                rel: "r.sav".into(),
                path: "r.sav".into(),
                mtime: 0,
                meta: None,
            },
            snap: Some(DlcLoad {
                enabled_mods: vec!["m1".into()],
                disabled_dlcs: dis.iter().map(|s| s.to_string()).collect(),
            }),
        };
        assert_eq!(
            binding(&mk(&[]), &g, &mods, &dlcs),
            Binding::Playset("A".into())
        );
        assert_eq!(
            binding(&mk(&["d1"]), &g, &mods, &dlcs),
            Binding::Playset("B".into())
        );
        let mut other = mk(&[]);
        other.snap.as_mut().unwrap().enabled_mods = vec!["m2".into(), "m1".into()];
        assert_eq!(binding(&other, &g, &mods, &dlcs), Binding::Custom(2));
        let mut none = mk(&[]);
        none.snap = None;
        assert_eq!(binding(&none, &g, &mods, &dlcs), Binding::Unknown);
        assert_eq!(Binding::Custom(2).to_string(), "custom (2 mods)");
    }

    #[test]
    fn plan_and_adopt() {
        let mods = [mod_("m1"), mod_("m2")];
        let mut g = GameState::default();
        g.current().reconcile(&mods);
        let mut b = Bound {
            save: Save {
                rel: "e/s.sav".into(),
                path: "e/s.sav".into(),
                mtime: 0,
                meta: None,
            },
            snap: None,
        };
        assert!(plan(&b, &g, &mods, &[]).risky());

        b.snap = Some(DlcLoad {
            enabled_mods: vec!["m2".into()],
            disabled_dlcs: vec![],
        });
        let p = plan(&b, &g, &mods, &[]);
        assert!(!p.risky());
        adopt(&mut g, p, &mods, &[]);
        assert_eq!((g.active, g.playsets.len()), (1, 2));
        assert_eq!(g.playsets[1].name, "For s");
        assert!(g.playsets[1].mods[0].enabled && !g.playsets[1].mods[1].enabled);
        // Now a playset matches, so no third one.
        let p = plan(&b, &g, &mods, &[]);
        assert!(matches!(p, Plan::Use(1)));

        // A snapshot with a mod that is not installed is risky; adopting twice reuses.
        b.snap.as_mut().unwrap().enabled_mods.push("gone".into());
        for _ in 0..2 {
            let p = plan(&b, &g, &mods, &[]);
            assert!(p.risky());
            adopt(&mut g, p, &mods, &[]);
        }
        assert_eq!(g.playsets.len(), 2, "same name and content is reused");
    }

    #[test]
    fn continue_arg_needs_the_stem() {
        let (_, i, _) = fixture("cont");
        let s = |p: &str| Save {
            rel: p.into(),
            path: PathBuf::from(p),
            mtime: 0,
            meta: None,
        };
        let all: Vec<Save> = [
            "e/autosave_2230.01.01.sav",
            "e/other.sav",
            "emp_a/1.sav",
            "emp_a/2201.01.01.sav",
            "emp_b/2201.01.01.sav",
            "emp_a/autosave.sav",
            "emp_b/autosave.sav",
        ]
        .iter()
        .map(|p| s(p))
        .collect();
        assert!(continue_arg(&i, &s("e/autosave_2230.01.01.sav"), &all).is_none());
        put(
            &i.data_dir.join("continue_game.json"),
            r#"{"title":"E","save_game":"e/autosave_2230.01.01.sav"}"#,
        );
        assert!(continue_arg(&i, &s("e/autosave_2230.01.01.sav"), &all).is_some());
        assert!(continue_arg(&i, &s("e/other.sav"), &all).is_none());
        // Substring stem must never match wrong campaign or title.
        put(
            &i.data_dir.join("continue_game.json"),
            r#"{"title":"Empire 1","save_game":"emp_b/2201.01.01.sav"}"#,
        );
        assert!(continue_arg(&i, &s("emp_a/1.sav"), &all).is_none());
        assert!(continue_arg(&i, &s("emp_a/2201.01.01.sav"), &all).is_none());
        assert!(continue_arg(&i, &s("emp_b/2201.01.01.sav"), &all).is_some());
        // Paradox filename field with "save games/" prefix matches exact relative path.
        put(
            &i.data_dir.join("continue_game.json"),
            r#"{"filename":"save games/emp_a/autosave.sav"}"#,
        );
        assert!(continue_arg(&i, &s("emp_a/autosave.sav"), &all).is_some());
        assert!(continue_arg(&i, &s("emp_b/autosave.sav"), &all).is_none());
        // Basename target shared by two saves in different roots/dirs is ambiguous: plain launch.
        put(
            &i.data_dir.join("continue_game.json"),
            r#"{"save":"autosave.sav"}"#,
        );
        assert!(continue_arg(&i, &s("emp_a/autosave.sav"), &all).is_none());
        assert!(continue_arg(&i, &s("emp_b/autosave.sav"), &all).is_none());
        // Unique basename still works.
        put(
            &i.data_dir.join("continue_game.json"),
            r#"{"save":"other.sav"}"#,
        );
        assert!(continue_arg(&i, &s("e/other.sav"), &all).is_some());
    }

    #[test]
    fn corrupt_book_peek_never_renames_and_sweep_fails_closed() {
        let (roots, i, _) = fixture("corrupt-book");
        let bp = book_path(&roots);
        put(&bp, "{ broken saves");
        let mtime_before = std::fs::metadata(&bp).unwrap().modified().unwrap();

        let (rows, warn) = bound_peek(&i, &roots);
        assert!(rows.is_empty());
        assert!(warn.is_some());
        assert_eq!(std::fs::read_to_string(&bp).unwrap(), "{ broken saves");
        assert_eq!(
            std::fs::metadata(&bp).unwrap().modified().unwrap(),
            mtime_before
        );
        assert!(!bp.with_extension("json.bad").exists());

        // sweep fails closed without modifying corrupt saves.json.
        assert!(sweep(&i, &roots).is_err());
        assert_eq!(std::fs::read_to_string(&bp).unwrap(), "{ broken saves");
    }

    #[cfg(unix)]
    #[test]
    fn stray_file_in_steam_userdata_is_not_a_user() {
        // Finder drops .DS_Store into Steam/userdata; reading `.DS_Store/<app>/…` is ENOTDIR.
        let (roots, i, b) = fixture("userdata-dsstore");
        put(&b.join("steam/userdata/.DS_Store"), "x");
        std::fs::create_dir_all(b.join("steam/userdata/42")).unwrap();
        sweep(&i, &roots).unwrap();
    }

    #[test]
    fn failed_scan_preserves_existing_bindings() {
        use std::os::unix::fs::PermissionsExt;
        let (roots, i, _) = fixture("preserve-bind");
        let now = SystemTime::now();
        sweep(&i, &roots).unwrap();
        dl(&i, &["mod/a.mod"], &[]);
        put_save(
            &i,
            "emp/s1.sav",
            &sav("E", "2201.01.01"),
            now + Duration::from_secs(10),
        );
        sweep(&i, &roots).unwrap();
        assert!(snap_of(&i, &roots, "emp/s1.sav").is_some());

        // An unreadable campaign dir fails the sweep: nothing is written, so the watermark does
        // not advance past saves we could not see and the binding is kept.
        let book_before = std::fs::read(book_path(&roots)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let emp_dir = i.data_dir.join("save games/emp");
        std::fs::set_permissions(&emp_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let res = sweep(&i, &roots);
        std::fs::set_permissions(&emp_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = res.unwrap_err().to_string();
        assert!(err.contains("could not read save location"), "{err}");
        assert_eq!(std::fs::read(book_path(&roots)).unwrap(), book_before);
        assert!(snap_of(&i, &roots, "emp/s1.sav").is_some());
    }

    #[test]
    fn missing_save_root_keeps_its_bindings_but_deleted_saves_are_pruned() {
        let (roots, i, _) = fixture("missing-root");
        let now = SystemTime::now();
        sweep(&i, &roots).unwrap();
        dl(&i, &["mod/a.mod"], &[]);
        put_save(
            &i,
            "emp/s1.sav",
            &sav("E", "2201.01.01"),
            now + Duration::from_secs(10),
        );
        sweep(&i, &roots).unwrap();
        assert!(snap_of(&i, &roots, "emp/s1.sav").is_some());
        // Root vanishes (unmounted): the record stays.
        let root = i.data_dir.join("save games");
        let parked = i.data_dir.join("save games.parked");
        std::fs::rename(&root, &parked).unwrap();
        sweep(&i, &roots).unwrap();
        std::fs::rename(&parked, &root).unwrap();
        assert!(
            snap_of(&i, &roots, "emp/s1.sav").is_some(),
            "binding must survive a missing root"
        );
        // A save deleted from a present root is still pruned.
        std::fs::remove_file(root.join("emp/s1.sav")).unwrap();
        sweep(&i, &roots).unwrap();
        let (book, _) = Book::peek(&book_path(&roots));
        assert!(!book.games["stellaris"].saves.contains_key("emp/s1.sav"));
    }

    #[test]
    fn find_one_validates_path_and_containment() {
        let (roots, i, b) = fixture("find-one");
        put_save(
            &i,
            "emp/ok.sav",
            &sav("Ok", "2200.01.01"),
            SystemTime::now(),
        );
        put(&b.join("data/outside.sav"), "x");
        assert!(find_one(&i, &roots, "emp/ok.sav").is_some());
        assert!(find_one(&i, &roots, "../outside.sav").is_none());
        assert!(find_one(&i, &roots, "a/b/c.sav").is_none());
        assert!(find_one(&i, &roots, "emp/ok.txt").is_none());
    }
}
