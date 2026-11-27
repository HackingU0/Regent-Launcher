//! Playset store (JSON in app data), reconcile, dlc_load.json export and launch. UI-free.
use crate::game::{Install, Roots};
use crate::mods::{Dlc, Mod};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// <app data>/regent-launcher: playsets.json and everything else of ours.
pub fn app_dir(roots: &Roots) -> PathBuf {
    roots.data.join("regent-launcher")
}

/// <app data>/regent-launcher/playsets.json. JSON, not sqlite: tiny, hand-readable, no new crate.
pub fn store_path(roots: &Roots) -> PathBuf {
    app_dir(roots).join("playsets.json")
}

// ---------------------------------------------------------------- cross-process lock

struct InProcLock {
    held: Mutex<bool>,
    cv: Condvar,
}

impl InProcLock {
    fn new() -> Self {
        Self {
            held: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    fn acquire(self: &Arc<Self>) -> InProcGuard {
        let mut h = self.held.lock().unwrap_or_else(|e| e.into_inner());
        while *h {
            h = self.cv.wait(h).unwrap_or_else(|e| e.into_inner());
        }
        *h = true;
        InProcGuard(Arc::clone(self))
    }
}

struct InProcGuard(Arc<InProcLock>);

impl Drop for InProcGuard {
    fn drop(&mut self) {
        let mut h = self.0.held.lock().unwrap_or_else(|e| e.into_inner());
        *h = false;
        self.0.cv.notify_one();
    }
}

thread_local! {
    static HELD_LOCKS: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

fn proc_lock_for(path: &Path) -> Arc<InProcLock> {
    static MAP: OnceLock<Mutex<HashMap<PathBuf, Arc<InProcLock>>>> = OnceLock::new();
    let map = MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        g.entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(InProcLock::new())),
    )
}

/// Exclusive in-process and cross-process lock on `<app_dir>/.lock`.
/// Re-entrant on the same thread; never call from read-only paths (it creates `<app_dir>/.lock`).
pub struct AppLock {
    file: Option<File>,
    path: Option<PathBuf>,
    in_proc: Option<InProcGuard>,
}

impl Drop for AppLock {
    fn drop(&mut self) {
        // Release OS flock before releasing in-process waiters.
        drop(self.file.take());
        if let Some(p) = self.path.take() {
            HELD_LOCKS.with(|h| {
                let mut v = h.borrow_mut();
                if let Some(i) = v.iter().rposition(|x| x == &p) {
                    v.remove(i);
                }
            });
        }
        drop(self.in_proc.take());
    }
}

pub fn lock_app(roots: &Roots) -> Result<AppLock> {
    let dir = app_dir(roots);
    std::fs::create_dir_all(&dir)?;
    let canon_dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let lock_path = canon_dir.join(".lock");
    let already_held = HELD_LOCKS.with(|h| h.borrow().iter().any(|p| p == &lock_path));
    if already_held {
        return Ok(AppLock {
            file: None,
            path: None,
            in_proc: None,
        });
    }
    let in_proc = proc_lock_for(&lock_path).acquire();
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening lockfile {}", lock_path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", lock_path.display()))?;
    HELD_LOCKS.with(|h| h.borrow_mut().push(lock_path.clone()));
    Ok(AppLock {
        file: Some(file),
        path: Some(lock_path),
        in_proc: Some(in_proc),
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playset {
    pub name: String,
    #[serde(default)]
    pub mods: Vec<Entry>,
    /// Installed DLC ids the user turned off.
    #[serde(default)]
    pub disabled_dlcs: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameState {
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub playsets: Vec<Playset>,
    #[serde(default)]
    pub launch_args: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// Set when playsets.json exists but could not be read: never overwrite it.
    #[serde(skip)]
    pub read_only: bool,
    /// Games whose default playset was seeded from an existing, unparsable `dlc_load.json`.
    #[serde(skip)]
    pub bad_dlc_load: HashSet<String>,
    pub games: BTreeMap<String, GameState>,
}

/// The game's own dlc_load.json format.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DlcLoad {
    #[serde(default)]
    pub enabled_mods: Vec<String>,
    #[serde(default)]
    pub disabled_dlcs: Vec<String>,
}

impl Playset {
    pub fn new(name: &str) -> Playset {
        Playset {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn duplicate(&self) -> Playset {
        Playset {
            name: format!("{} copy", self.name),
            ..self.clone()
        }
    }

    /// Empty names are ignored.
    pub fn rename(&mut self, name: &str) {
        if !name.trim().is_empty() {
            self.name = name.trim().into();
        }
    }

    pub fn toggle(&mut self, id: &str) {
        if let Some(e) = self.mods.iter_mut().find(|e| e.id == id) {
            e.enabled = !e.enabled;
        }
    }

    /// `from` takes the position `to` had (lands after `to` when moving down, before it when moving up).
    pub fn move_mod(&mut self, from: &str, to: &str) {
        let (Some(f), Some(t)) = (
            self.mods.iter().position(|e| e.id == from),
            self.mods.iter().position(|e| e.id == to),
        ) else {
            return;
        };
        let e = self.mods.remove(f);
        self.mods.insert(t, e);
    }

    /// Append mods new on disk as disabled. Missing entries stay (hidden by UI, skipped on export).
    pub fn reconcile(&mut self, mods: &[Mod]) {
        let known: HashSet<String> = self.mods.iter().map(|e| e.id.clone()).collect();
        self.mods.extend(
            mods.iter()
                .filter(|m| !known.contains(&m.id))
                .map(|m| Entry {
                    id: m.id.clone(),
                    enabled: false,
                }),
        );
    }

    /// Entries that exist on disk, in load order (what the UI lists).
    pub fn visible<'a>(&'a self, mods: &'a [Mod]) -> Vec<(&'a Entry, &'a Mod)> {
        let by_id: HashMap<&str, &Mod> = mods.iter().map(|m| (m.id.as_str(), m)).collect();
        self.mods
            .iter()
            .filter_map(|e| by_id.get(e.id.as_str()).map(|m| (e, *m)))
            .collect()
    }

    pub fn dlc_enabled(&self, id: &str) -> bool {
        !self.disabled_dlcs.iter().any(|d| d == id)
    }

    pub fn toggle_dlc(&mut self, id: &str) {
        match self.disabled_dlcs.iter().position(|d| d == id) {
            Some(i) => drop(self.disabled_dlcs.remove(i)),
            None => self.disabled_dlcs.push(id.into()),
        }
    }
}

impl GameState {
    pub fn current(&mut self) -> &mut Playset {
        let i = self.active.min(self.playsets.len().saturating_sub(1));
        if self.playsets.is_empty() {
            self.playsets.push(Playset::new("Default"));
        }
        &mut self.playsets[i]
    }

    /// Append and switch to it.
    pub fn add(&mut self, p: Playset) {
        self.playsets.push(p);
        self.active = self.playsets.len() - 1;
    }
}

/// Seed a playset from the game's dlc_load.json. Missing -> empty. Unreadable -> empty plus a
/// warning, leaving the file alone. Returns `(playset, warning, dlc_load_was_corrupt)`.
fn seed(data_dir: &Path) -> (Playset, Option<String>, bool) {
    let path = data_dir.join("dlc_load.json");
    let (l, warning, corrupt) = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<DlcLoad>(&t).map_err(|e| e.to_string()))
    {
        Ok(l) => (l, None, false),
        Err(_) if !path.exists() => (DlcLoad::default(), None, false),
        Err(e) => {
            let w = format!(
                "Could not read {} ({e}); started an empty playset",
                path.display()
            );
            (DlcLoad::default(), Some(w), true)
        }
    };
    let p = Playset {
        name: "Default".into(),
        mods: l
            .enabled_mods
            .into_iter()
            .map(|id| Entry { id, enabled: true })
            .collect(),
        disabled_dlcs: l.disabled_dlcs,
    };
    (p, warning, corrupt)
}

impl Store {
    /// Refusal text for a game whose default playset was seeded from an unparsable dlc_load.json.
    pub fn bad_seed_message(install: &Install) -> String {
        format!(
            "{} exists but could not be parsed; refusing to use an empty seeded playset. Back up or fix dlc_load.json first.",
            install.data_dir.join("dlc_load.json").display()
        )
    }

    /// Never renames or copies anything: a corrupt store comes back `read_only` with a warning.
    /// Used by all read and ordinary mutating commands (mutations fail closed when `read_only`).
    pub fn peek(path: &Path, installs: &[Install]) -> (Store, Option<String>) {
        let mut warning = None;
        let mut store = match std::fs::read_to_string(path).map(|t| serde_json::from_str(&t)) {
            Ok(Ok(s)) => s,
            Ok(Err(_)) => {
                warning = Some(format!(
                    "{} is corrupt; showing defaults, nothing will be saved",
                    path.display()
                ));
                Store {
                    read_only: true,
                    ..Store::default()
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(e) => {
                warning = Some(format!(
                    "Could not read {} ({e}); changes will not be saved",
                    path.display()
                ));
                Store {
                    read_only: true,
                    ..Store::default()
                }
            }
        };
        for i in installs {
            let g = store.games.entry(i.def.id.into()).or_default();
            if g.playsets.is_empty() {
                let (p, w, corrupt_dl) = seed(&i.data_dir);
                if corrupt_dl {
                    store.bad_dlc_load.insert(i.def.id.into());
                }
                g.playsets.push(p);
                warning = warning.or(w);
            }
            g.active = g.active.min(g.playsets.len() - 1);
        }
        (store, warning)
    }

    pub fn game(&mut self, id: &str) -> &mut GameState {
        self.games.entry(id.into()).or_default()
    }

    /// Atomic: write a temp file next to the target, then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        // A playset seeded from an unparsable dlc_load.json is never materialized: it would hide
        // the corruption from the next read and let the next launch replace the original file.
        let mut out = self.clone();
        for id in &self.bad_dlc_load {
            if let Some(g) = out.games.get_mut(id) {
                g.playsets.clear();
                g.active = 0;
            }
        }
        write_atomic(path, &serde_json::to_string_pretty(&out)?)
    }
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create a private unique temp file next to `path`, write `bytes`, and set permissions.
pub(crate) fn stage_atomic_bytes(
    path: &Path,
    bytes: &[u8],
    perms: Option<Permissions>,
) -> Result<PathBuf> {
    let dir = path.parent().context("path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().context("path has no file name")?;
    let pid = std::process::id();
    let (mut file, tmp) = loop {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!(".{}.tmp-{pid}-{seq}", name.to_string_lossy()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => break (f, candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    let write_res = (|| -> Result<()> {
        file.write_all(bytes)?;
        drop(file);
        if let Some(p) = perms {
            let _ = std::fs::set_permissions(&tmp, p);
        } else if let Ok(m) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(tmp)
}

/// Write bytes to a private unique temp file next to `path`, then rename over it.
pub(crate) fn write_atomic_bytes(
    path: &Path,
    bytes: &[u8],
    perms: Option<Permissions>,
) -> Result<()> {
    let tmp = stage_atomic_bytes(path, bytes, perms)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Write a unique temp file next to `path`, then rename over it.
/// An existing target keeps its permissions.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    write_atomic_bytes(path, text.as_bytes(), None)
}

/// What dlc_load.json holds for a playset: enabled installed mods in order, installed DLC ids
/// the user turned off. Never lists anything not installed.
pub fn dlc_load(p: &Playset, mods: &[Mod], dlcs: &[Dlc]) -> DlcLoad {
    let by_id: HashMap<&str, &Mod> = mods.iter().map(|m| (m.id.as_str(), m)).collect();
    DlcLoad {
        enabled_mods: p
            .mods
            .iter()
            .filter(|e| e.enabled)
            .filter_map(|e| by_id.get(e.id.as_str()).map(|m| m.id.clone()))
            .collect(),
        disabled_dlcs: p
            .disabled_dlcs
            .iter()
            .filter(|d| dlcs.iter().any(|x| &x.id == *d))
            .cloned()
            .collect(),
    }
}

/// The game's current dlc_load.json; None when missing or unparsable.
pub fn read_dlc_load(data_dir: &Path) -> Option<DlcLoad> {
    serde_json::from_str(&std::fs::read_to_string(data_dir.join("dlc_load.json")).ok()?).ok()
}

/// Verify the game executable exists (and is executable on Unix) before mutating anything on disk.
pub fn preflight_exe(exe: &Path) -> Result<()> {
    let meta = std::fs::metadata(exe)
        .with_context(|| format!("game executable not found: {}", exe.display()))?;
    if !meta.is_file() {
        bail!("game executable is not a regular file: {}", exe.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            bail!("game executable is not executable: {}", exe.display());
        }
    }
    Ok(())
}

type SavedFile = (PathBuf, Option<(Vec<u8>, Permissions)>);

/// Snapshot of files touched by `prepare_launch` so they can be restored if writing or spawning fails.
pub struct LaunchSnapshot {
    entries: Vec<SavedFile>,
}

impl LaunchSnapshot {
    fn capture(paths: &[PathBuf]) -> Result<Self> {
        let mut entries = Vec::with_capacity(paths.len());
        for p in paths {
            match std::fs::read(p) {
                Ok(bytes) => {
                    let perms = std::fs::metadata(p)?.permissions();
                    entries.push((p.clone(), Some((bytes, perms))));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    entries.push((p.clone(), None));
                }
                Err(e) => return Err(e).with_context(|| format!("reading {}", p.display())),
            }
        }
        Ok(Self { entries })
    }

    /// Restore every snapshotted file in reverse order, trying all of them. The error names each
    /// file that could not be put back.
    pub fn restore(&self) -> Result<()> {
        let mut failed = vec![];
        for (path, orig) in self.entries.iter().rev() {
            let res = match orig {
                Some((bytes, perms)) => write_atomic_bytes(path, bytes, Some(perms.clone())),
                None => match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                    _ => Ok(()),
                },
            };
            if let Err(e) = res {
                failed.push(format!("{} ({e})", path.display()));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            bail!(
                "could not restore the previous contents of: {}; check these files before launching again",
                failed.join(", ")
            )
        }
    }

    /// Restore after `cause` failed; the returned message carries any restore failures too.
    pub fn restore_after(&self, cause: impl std::fmt::Display) -> String {
        match self.restore() {
            Ok(()) => cause.to_string(),
            Err(e) => format!("{cause}; {e:#}"),
        }
    }
}

/// Write descriptors + `dlc_load.json`, snapshotting prior bytes first and restoring on any write error.
pub fn write_dlc_load_snapshotted(
    data_dir: &Path,
    p: &Playset,
    mods: &[Mod],
    dlcs: &[Dlc],
) -> Result<LaunchSnapshot> {
    let load = dlc_load(p, mods, dlcs);
    let mut to_write: Vec<(PathBuf, String)> = vec![];
    for m in mods.iter().filter(|m| load.enabled_mods.contains(&m.id)) {
        if let Some(text) = &m.needs_descriptor {
            to_write.push((data_dir.join(&m.id), text.clone()));
        }
    }
    let dl_json = serde_json::to_string_pretty(&load)?;
    to_write.push((data_dir.join("dlc_load.json"), dl_json));
    let paths: Vec<PathBuf> = to_write.iter().map(|(p, _)| p.clone()).collect();
    let snap = LaunchSnapshot::capture(&paths)?;
    for (path, text) in &to_write {
        if let Err(e) = write_atomic(path, text) {
            bail!("{}", snap.restore_after(format!("{e:#}")));
        }
    }
    Ok(snap)
}

/// Rescan mods and installed DLCs of one install.
pub fn scan(i: &Install) -> (Vec<Mod>, Vec<Dlc>) {
    (
        crate::mods::scan_mods(&i.data_dir, i.workshop.as_deref()),
        crate::mods::scan_dlcs(&i.dir),
    )
}

/// Preflight executable, sweep saves (propagating errors), snapshot prior files, then write
/// descriptors + dlc_load.json. Returns the snapshot so callers can restore if `spawn` fails.
pub fn prepare_launch_with_snapshot(
    install: &Install,
    roots: &Roots,
    p: &Playset,
    mods: &[Mod],
    dlcs: &[Dlc],
) -> Result<LaunchSnapshot> {
    let _lock = lock_app(roots)?;
    preflight_exe(&install.exe)?;
    crate::saves::sweep(install, roots)?;
    write_dlc_load_snapshotted(&install.data_dir, p, mods, dlcs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Channel, GAMES, put, tmp_dir};

    fn m(id: &str, needs: bool) -> Mod {
        Mod {
            id: id.into(),
            name: id.into(),
            needs_descriptor: needs.then(|| format!("name=\"{id}\"\npath=\"/ws\"\n")),
            ..Default::default()
        }
    }

    fn dlc(id: &str) -> Dlc {
        Dlc {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        }
    }

    fn ids(p: &Playset) -> Vec<&str> {
        p.mods.iter().map(|e| e.id.as_str()).collect()
    }

    fn install(base: &Path, exe: &str) -> Install {
        Install {
            def: &GAMES[0],
            channel: Channel::Steam,
            dir: base.join("game"),
            launcher_dir: base.join("game"),
            theme_file: base.join("game/assets/theme-settings.json"),
            settings_layout: base.join("game/settings-layout.json"),
            workshop: None,
            version: None,
            raw_version: None,
            data_dir: base.join("data"),
            exe: PathBuf::from(exe),
            exe_args: vec![],
        }
    }

    #[test]
    fn reconcile_toggle_move() {
        let mut p = Playset::new("P");
        p.reconcile(&[m("a", false), m("b", false), m("c", false)]);
        assert!(p.mods.iter().all(|e| !e.enabled));
        p.toggle("b");
        p.reconcile(&[m("a", false), m("d", false)]);
        assert_eq!(ids(&p), ["a", "b", "c", "d"]);
        assert!(p.mods[1].enabled && !p.mods[3].enabled);
        p.move_mod("d", "a");
        assert_eq!(ids(&p), ["d", "a", "b", "c"]);
        p.move_mod("d", "c");
        assert_eq!(ids(&p), ["a", "b", "c", "d"]);
        p.move_mod("a", "zzz");
        assert_eq!(ids(&p), ["a", "b", "c", "d"]);
    }

    #[test]
    fn playset_ops() {
        let mut g = GameState::default();
        assert_eq!(g.current().name, "Default");
        let mut p = Playset::new("One");
        p.reconcile(&[m("a", false)]);
        p.toggle("a");
        let mut d = p.duplicate();
        d.rename("  Two ");
        d.rename("   ");
        d.toggle("a");
        g.add(p);
        g.add(d);
        assert_eq!((g.active, g.current().name.as_str()), (2, "Two"));
        assert!(g.playsets[1].mods[0].enabled && !g.playsets[2].mods[0].enabled);
        assert_eq!(g.playsets[1].duplicate().name, "One copy");
    }

    #[test]
    fn dlc_toggle() {
        let mut p = Playset::new("P");
        assert!(p.dlc_enabled("x"));
        p.toggle_dlc("x");
        assert!(!p.dlc_enabled("x"));
        p.toggle_dlc("x");
        assert!(p.disabled_dlcs.is_empty());
    }

    #[test]
    fn dlc_load_order_skip_and_subset() {
        let b = tmp_dir("export");
        let data = b.join("data");
        let mut p = Playset::new("P");
        p.reconcile(&[
            m("mod/c.mod", false),
            m("mod/ugc_1.mod", true),
            m("mod/off.mod", true),
            m("mod/a.mod", false),
        ]);
        p.mods.push(Entry {
            id: "mod/missing.mod".into(),
            enabled: true,
        });
        for id in ["mod/c.mod", "mod/ugc_1.mod", "mod/a.mod", "mod/missing.mod"] {
            p.toggle(id);
        }
        p.disabled_dlcs = vec!["dlc/a.dlc".into(), "dlc/not_installed.dlc".into()];
        let scanned = [
            m("mod/c.mod", false),
            m("mod/ugc_1.mod", true),
            m("mod/off.mod", true),
            m("mod/a.mod", false),
        ];
        let installed = [dlc("dlc/a.dlc"), dlc("dlc/b.dlc")];
        write_dlc_load_snapshotted(&data, &p, &scanned, &installed).unwrap();

        let out: DlcLoad =
            serde_json::from_str(&std::fs::read_to_string(data.join("dlc_load.json")).unwrap())
                .unwrap();
        assert_eq!(
            out.enabled_mods,
            ["mod/c.mod", "mod/ugc_1.mod", "mod/a.mod"]
        );
        assert_eq!(out.disabled_dlcs, ["dlc/a.dlc"]);
        assert!(
            out.disabled_dlcs
                .iter()
                .all(|d| installed.iter().any(|i| &i.id == d))
        );
        assert!(
            std::fs::read_to_string(data.join("mod/ugc_1.mod"))
                .unwrap()
                .contains("path=\"/ws\"")
        );
        assert!(!data.join("mod/off.mod").exists());
    }

    #[test]
    fn store_round_trip_and_seed() {
        let b = tmp_dir("store");
        let inst = install(&b, "x");
        put(
            &inst.data_dir.join("dlc_load.json"),
            r#"{"enabled_mods":["mod/ugc_1.mod"],"disabled_dlcs":["dlc/x.dlc"]}"#,
        );
        let path = store_path(&Roots {
            data: b.join("appdata"),
            ..Default::default()
        });
        let (mut s, warn) = Store::peek(&path, std::slice::from_ref(&inst));
        assert!(warn.is_none());
        let p = s.game("stellaris").current().clone();
        assert_eq!(
            (p.name.as_str(), ids(&p), p.mods[0].enabled),
            ("Default", vec!["mod/ugc_1.mod"], true)
        );
        assert_eq!(p.disabled_dlcs, ["dlc/x.dlc"]);
        s.game("stellaris").launch_args = "-debug_mode".into();
        s.save(&path).unwrap();
        let (s2, _) = Store::peek(&path, &[inst]);
        assert_eq!(s2.games["stellaris"].launch_args, "-debug_mode");
        assert_eq!(s2.games["stellaris"].playsets.len(), 1);
    }

    #[test]
    fn store_clamps_active_and_handles_corrupt() {
        let b = tmp_dir("corrupt");
        let inst = install(&b, "x");
        let path = b.join("playsets.json");
        put(
            &path,
            r#"{"games":{"stellaris":{"active":9,"playsets":[{"name":"A"}]}}}"#,
        );
        let (s, w) = Store::peek(&path, std::slice::from_ref(&inst));
        assert!(w.is_none());
        assert_eq!(s.games["stellaris"].active, 0);
        put(&path, "{ not json");
        let (s, w) = Store::peek(&path, &[inst]);
        assert!(w.is_some() && s.read_only);
        assert_eq!(s.games["stellaris"].playsets.len(), 1);
        s.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        std::fs::remove_file(&path).unwrap();
        // unreadable (here: a directory) -> warn and never overwrite
        std::fs::create_dir_all(&path).unwrap();
        let (s, w) = Store::peek(&path, &[install(&b, "x")]);
        assert!(w.is_some() && s.read_only);
        s.save(&path).unwrap();
        assert!(path.is_dir());
    }

    fn roots(b: &Path) -> Roots {
        Roots {
            data: b.join("appdata"),
            ..Default::default()
        }
    }

    #[test]
    fn dlc_load_pure_and_read_back() {
        let b = tmp_dir("dlcload");
        let mut p = Playset::new("P");
        p.reconcile(&[m("mod/a.mod", false), m("mod/b.mod", false)]);
        p.toggle("mod/b.mod");
        p.disabled_dlcs = vec!["dlc/a.dlc".into(), "dlc/gone.dlc".into()];
        let (mods, dlcs) = (
            [m("mod/a.mod", false), m("mod/b.mod", false)],
            [dlc("dlc/a.dlc")],
        );
        let l = dlc_load(&p, &mods, &dlcs);
        assert_eq!(
            (l.enabled_mods.as_slice(), l.disabled_dlcs.as_slice()),
            (
                &["mod/b.mod".to_string()][..],
                &["dlc/a.dlc".to_string()][..]
            )
        );
        assert!(read_dlc_load(&b).is_none());
        write_dlc_load_snapshotted(&b, &p, &mods, &dlcs).unwrap();
        assert!(read_dlc_load(&b) == Some(l));
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp_dir("perm");
        put(&b.join("f"), "old");
        std::fs::set_permissions(b.join("f"), std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&b.join("f"), "new").unwrap();
        let mode = std::fs::metadata(b.join("f")).unwrap().permissions().mode();
        assert_eq!(
            (mode & 0o777, std::fs::read_to_string(b.join("f")).unwrap()),
            (0o600, "new".into())
        );
    }

    #[test]
    fn cross_process_lock_blocks_independent_try_lock_and_is_reentrant() {
        let b = tmp_dir("applock");
        let r = roots(&b);
        let guard1 = lock_app(&r).unwrap();
        // Same thread is re-entrant (no deadlock).
        let guard2 = lock_app(&r).unwrap();
        let lock_file = app_dir(&r).join(".lock");
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_file)
            .unwrap();
        assert!(
            probe.try_lock().is_err(),
            "independent handle must see exclusive OS file lock"
        );
        drop(guard2);
        // Still locked while guard1 is held.
        assert!(probe.try_lock().is_err());
        drop(guard1);
        // Released after outermost guard drops.
        probe.try_lock().expect("lock released");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_launch_fails_closed_before_writing() {
        let b = tmp_dir("prepare-fail");
        std::fs::create_dir_all(b.join("game")).unwrap();
        let r = roots(&b);
        let mut inst = install(&b, "/usr/bin/true");
        let orig_dl = r#"{"enabled_mods":["mod/orig.mod"],"disabled_dlcs":[]}"#;
        put(&inst.data_dir.join("dlc_load.json"), orig_dl);

        // 1. Missing exe fails preflight before writing dlc_load.json or saves.json.
        inst.exe = b.join("missing_exe");
        let mut p = Playset::new("P");
        let m1 = m("mod/ugc_1.mod", true);
        p.reconcile(std::slice::from_ref(&m1));
        p.toggle("mod/ugc_1.mod");
        assert!(
            prepare_launch_with_snapshot(&inst, &r, &p, std::slice::from_ref(&m1), &[]).is_err()
        );
        assert_eq!(
            std::fs::read_to_string(inst.data_dir.join("dlc_load.json")).unwrap(),
            orig_dl
        );
        assert!(!inst.data_dir.join("mod/ugc_1.mod").exists());
        assert!(!app_dir(&r).join("saves.json").exists());

        // 2. Corrupt saves.json fails closed (propagates sweep error, does not overwrite dlc_load.json).
        inst.exe = PathBuf::from("/usr/bin/true");
        put(&app_dir(&r).join("saves.json"), "{ corrupt saves");
        assert!(prepare_launch_with_snapshot(&inst, &r, &p, &[m1], &[]).is_err());
        assert_eq!(
            std::fs::read_to_string(inst.data_dir.join("dlc_load.json")).unwrap(),
            orig_dl
        );
        assert_eq!(
            std::fs::read_to_string(app_dir(&r).join("saves.json")).unwrap(),
            "{ corrupt saves"
        );
    }
    #[cfg(unix)]
    #[test]
    fn restore_failure_names_the_files_and_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp_dir("restore-fail");
        let f = b.join("d/dlc_load.json");
        put(&f, "orig");
        let snap = LaunchSnapshot::capture(std::slice::from_ref(&f)).unwrap();
        put(&f, "changed");
        std::fs::set_permissions(b.join("d"), Permissions::from_mode(0o555)).unwrap();
        let err = snap.restore().unwrap_err().to_string();
        let msg = snap.restore_after("boom");
        std::fs::set_permissions(b.join("d"), Permissions::from_mode(0o755)).unwrap();
        assert!(
            err.contains("dlc_load.json") && err.contains("restore"),
            "{err}"
        );
        assert!(
            msg.starts_with("boom; ") && msg.contains("dlc_load.json"),
            "{msg}"
        );
        // Writable again: restore succeeds and puts the original back.
        snap.restore().unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "orig");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_launch_refuses_when_a_save_root_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let b = tmp_dir("launch-unreadable-saves");
        std::fs::create_dir_all(b.join("game")).unwrap();
        let inst = install(&b, "/usr/bin/true");
        let orig = r#"{"enabled_mods":["mod/orig.mod"],"disabled_dlcs":[]}"#;
        put(&inst.data_dir.join("dlc_load.json"), orig);
        let emp = inst.data_dir.join("save games/emp");
        put(&emp.join("a.sav"), "x");
        std::fs::set_permissions(&emp, Permissions::from_mode(0o000)).unwrap();
        let res = prepare_launch_with_snapshot(&inst, &roots(&b), &Playset::new("P"), &[], &[]);
        std::fs::set_permissions(&emp, Permissions::from_mode(0o755)).unwrap();
        let err = res.err().unwrap().to_string();
        assert!(err.contains("could not read save location"), "{err}");
        assert_eq!(
            std::fs::read_to_string(inst.data_dir.join("dlc_load.json")).unwrap(),
            orig
        );
    }

    #[test]
    fn r3_1_failed_seed_is_never_materialized_by_saving() {
        let b = tmp_dir("seed-not-materialized");
        let inst = install(&b, "x");
        put(&inst.data_dir.join("dlc_load.json"), "{ broken");
        let path = b.join("playsets.json");
        let (mut s, _) = Store::peek(&path, std::slice::from_ref(&inst));
        assert!(s.bad_dlc_load.contains("stellaris"));
        s.game("eu4").launch_args = "-x".into();
        s.save(&path).unwrap();
        // The corrupt seed was not written, so the next read still sees it as corrupt.
        let (s2, _) = Store::peek(&path, &[inst]);
        assert!(s2.bad_dlc_load.contains("stellaris"));
        assert_eq!(s2.games["eu4"].launch_args, "-x");
    }
}
