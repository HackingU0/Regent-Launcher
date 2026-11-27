//! UI-free Tauri backend: command bodies, DTOs, caching, and serialization.
//!
//! Disk owns `playsets.json` (load, mutate, save per command).
//! In memory we cache only discovered installs, per-game scans (immutable `Arc<Scan>`),
//! lazily computed file indexes/sizes inside each `Scan`, and active game process counts.
use crate::art;
use crate::conflicts::{self, Checksum};
use crate::crash;
use crate::game::{self, Install, Roots};
use crate::index::{self, FileIndex, Manifest};
use crate::mods::{self, Dlc, Mod};
use crate::playset::{self, Entry, GameState, Playset, Store};
use crate::saves;
use crate::settings::{self as st, File, Files, Kind};
use crate::share;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- error & event

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    Stale,
    ReadOnly,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AppError {
    pub fn stale(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Stale,
            message: message.into(),
        }
    }

    pub fn read_only(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::ReadOnly,
            message: message.into(),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Failed,
            message: message.into(),
        }
    }

    fn from_anyhow(err: anyhow::Error) -> Self {
        Self::failed(format!("{err:#}"))
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

pub type AppResult<T> = Result<T, AppError>;

/// Payload of the `regent:refresh` event emitted to the webview.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshEvent {
    pub why: &'static str,
    pub game: Option<String>,
    pub warning: Option<String>,
}

impl RefreshEvent {
    pub fn focus() -> Self {
        Self {
            why: "focus",
            game: None,
            warning: None,
        }
    }

    /// Cached images for `game` changed: why is "art" or "thumbs".
    fn images(why: &'static str, game: &str) -> Self {
        Self {
            why,
            game: Some(game.to_string()),
            warning: None,
        }
    }

    pub fn exit(game: String, warning: Option<String>) -> Self {
        Self {
            why: "exit",
            game: Some(game),
            warning,
        }
    }
}

// ---------------------------------------------------------------- DTOs

/// The playset the UI believes is active.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct At {
    pub index: usize,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub version: Option<String>,
    pub raw_version: Option<String>,
}

impl GameInfo {
    fn from_install(i: &Install) -> Self {
        Self {
            id: i.def.id.to_string(),
            title: i.def.title.to_string(),
            channel: format!("{:?}", i.channel),
            version: i.version.clone(),
            raw_version: i.raw_version.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamesResponse {
    pub games: Vec<GameInfo>,
    pub fixture: bool,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameArt {
    pub logo: bool,
    pub background: bool,
    /// `art/wallpaper` resolves: custom, else cached default, else theme background.
    pub wallpaper: bool,
    /// The user set their own wallpaper for this game.
    pub custom: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub supported_version: Option<String>,
    pub mismatch: bool,
    pub remote_id: Option<String>,
    pub dependencies: Vec<String>,
    pub has_thumb: bool,
    /// Unix seconds (acf `timeupdated` for workshop mods, descriptor mtime for local mods).
    pub updated: Option<u64>,
    /// Bytes (acf `size` for workshop mods; local mod sizes arrive via `summary.sizes`).
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DlcInfo {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub affects_checksum: bool,
    pub has_thumb: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaysetDto {
    pub name: String,
    pub mods: Vec<Entry>,
    pub disabled_dlcs: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStateDto {
    pub active: usize,
    pub playsets: Vec<PlaysetDto>,
    pub launch_args: String,
}

impl From<GameState> for GameStateDto {
    fn from(g: GameState) -> Self {
        Self {
            active: g.active,
            playsets: g
                .playsets
                .into_iter()
                .map(|p| PlaysetDto {
                    name: p.name,
                    mods: p.mods,
                    disabled_dlcs: p.disabled_dlcs,
                })
                .collect(),
            launch_args: g.launch_args,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    pub game: String,
    pub art: GameArt,
    pub mods: Vec<ModInfo>,
    pub dlcs: Vec<DlcInfo>,
    pub state: GameStateDto,
    pub read_only: bool,
    pub warning: Option<String>,
    pub running: u32,
    pub crash_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictPairSummary {
    pub winner: String,
    pub loser: String,
    pub files: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestSummary {
    pub order: Vec<Entry>,
    pub reasons: Vec<String>,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub has_manifest: bool,
    pub pairs: Vec<ConflictPairSummary>,
    pub conflicts_by_mod: HashMap<String, usize>,
    pub suggest: SuggestSummary,
    pub checksum: Checksum,
    pub sizes: HashMap<String, u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchInfo {
    pub pid: u32,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FieldKind {
    Toggle,
    Slider { min: f64, max: f64, step: f64 },
    Int { max: u32 },
    Resolution,
    Mode,
    Language,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldOption {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsField {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: FieldKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<FieldOption>>,
    pub value: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSettings {
    pub game: Option<String>,
    pub pdx: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPaths {
    pub game: String,
    pub pdx: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub id: u64,
    /// Unix milliseconds (same timestamp as `id`).
    pub at: u64,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub rev: String,
    pub fields: Vec<SettingsField>,
    pub raw: RawSettings,
    pub paths: SettingsPaths,
    pub backups: Vec<BackupInfo>,
}

fn de_field_value<'de, D>(de: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(de)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Bool(b) => if b { "yes" } else { "no" }.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldEdit {
    pub field: String,
    #[serde(deserialize_with = "de_field_value")]
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsFileKey {
    Game,
    Pdx,
}

impl SettingsFileKey {
    fn to_file(self) -> File {
        match self {
            SettingsFileKey::Game => File::Game,
            SettingsFileKey::Pdx => File::Pdx,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawEdit {
    pub file: SettingsFileKey,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsApplyResponse {
    pub view: SettingsView,
    pub backup_dir: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SaveBinding {
    Unknown,
    Playset { name: String },
    Custom { count: usize },
}

impl From<saves::Binding> for SaveBinding {
    fn from(b: saves::Binding) -> Self {
        match b {
            saves::Binding::Unknown => SaveBinding::Unknown,
            saves::Binding::Playset(name) => SaveBinding::Playset { name },
            saves::Binding::Custom(count) => SaveBinding::Custom { count },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SavePlan {
    Use {
        playset: String,
    },
    Make {
        playset: String,
        missing: Vec<String>,
    },
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRow {
    pub rel: String,
    pub title: String,
    pub date: String,
    pub version: String,
    pub mtime: u64,
    pub binding: SaveBinding,
    pub plan: SavePlan,
    pub can_continue: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavesResponse {
    pub rows: Vec<SaveRow>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SavePlayOutcome {
    Launched {
        state: GameStateDto,
        using: String,
        manual_load: bool,
        pid: u32,
        warning: Option<String>,
    },
    NeedsConfirm {
        reason: String,
        missing: Vec<String>,
        /// Opaque identity of the plan; send it back as `expect` with `force`.
        token: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashListResponse {
    pub crashes: Vec<String>,
    pub has_live: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashReport {
    #[serde(flatten)]
    pub report: crash::Report,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareExportResponse {
    pub code: String,
    pub json: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareInspectResponse {
    pub name: String,
    pub game: String,
    pub game_version: Option<String>,
    pub mod_count: usize,
    pub missing: Vec<share::Missing>,
    pub diff: share::Diff,
    pub lines: Vec<String>,
    pub equal: bool,
    pub theirs: String,
    pub ours: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareImportResponse {
    pub state: GameStateDto,
    pub name: String,
    pub missing: Vec<share::Missing>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OpenTarget {
    Workshop {
        id: String,
    },
    CrashDir {
        game: String,
        #[serde(default)]
        name: Option<String>,
    },
    Backups {
        game: String,
    },
}

// ---------------------------------------------------------------- internal scan cache

const MAX_SHARE_INPUT_BYTES: usize = 4 * 1024 * 1024;

struct ModMeta {
    thumb: Option<(PathBuf, PathBuf)>,
    updated: Option<u64>,
    acf_size: Option<u64>,
}

struct Computed {
    index: Arc<FileIndex>,
    manifest: Option<Manifest>,
    sizes: HashMap<String, u64>,
}

type IndexKey = Vec<(String, Option<PathBuf>, Option<u64>)>;

struct Scan {
    install: Install,
    mods: Vec<Mod>,
    dlcs: Vec<Dlc>,
    meta: HashMap<String, ModMeta>,
    dlc_thumbs: HashMap<String, (PathBuf, PathBuf)>,
    index_key: IndexKey,
    acf_mtime: Option<SystemTime>,
    mod_dir_mtime: Option<SystemTime>,
    manifest_mtime: Option<SystemTime>,
    computed: Arc<OnceLock<Computed>>,
}

impl Scan {
    fn ensure_computed(&self) -> &Computed {
        self.computed.get_or_init(|| {
            let manifest = Manifest::load(&self.install.dir);
            let index = Arc::new(index::build(&self.mods, manifest.as_ref()));
            let mut sizes = HashMap::with_capacity(self.mods.len());
            for m in &self.mods {
                let sz = self
                    .meta
                    .get(&m.id)
                    .and_then(|mm| mm.acf_size)
                    .unwrap_or_else(|| m.root.as_deref().map_or(0, mods::disk_size));
                sizes.insert(m.id.clone(), sz);
            }
            Computed {
                index,
                manifest,
                sizes,
            }
        })
    }
}

struct Scans {
    installs: Vec<Install>,
    by_game: HashMap<&'static str, Arc<Scan>>,
    generation: HashMap<&'static str, u64>,
}

struct LaunchGuard {
    running: Arc<Mutex<HashMap<&'static str, u32>>>,
    game: &'static str,
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if let Ok(mut r) = self.running.lock()
            && let Some(n) = r.get_mut(self.game)
        {
            *n = n.saturating_sub(1);
        }
    }
}

type Emitter = Arc<dyn Fn(RefreshEvent) + Send + Sync>;

pub struct Backend {
    roots: Roots,
    fixture: bool,
    scans: RwLock<Scans>,
    running: Arc<Mutex<HashMap<&'static str, u32>>>,
    emit: Emitter,
    default_wallpapers_started: AtomicBool,
    /// One wallpaper set/reset at a time, so the last one the user picked is the one that sticks.
    wallpaper_lock: Mutex<()>,
    /// `(game, workshop id)` whose Steam preview was already requested this session.
    previews_tried: Mutex<HashSet<(&'static str, String)>>,
}

// ---------------------------------------------------------------- helpers

fn mtime_of(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

fn mtime_secs(p: &Path) -> Option<u64> {
    mtime_of(p)?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn plain_component(name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) {
        return false;
    }
    let mut c = Path::new(name).components();
    matches!((c.next(), c.next()), (Some(Component::Normal(_)), None))
}

fn dlc_thumb_path(game_dir: &Path, dlc: &Dlc) -> Option<(PathBuf, PathBuf)> {
    let dir = game_dir.join(&dlc.id).parent()?.to_path_buf();
    mods::contained_dir(game_dir, &dir)?;
    let name = dlc
        .thumbnail
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("thumbnail.png");
    let path = mods::plain_child(&dir, name)?;
    Some((dir, path))
}

fn build_scan(install: Install, prev: Option<&Arc<Scan>>, invalidate: bool) -> Arc<Scan> {
    let acf_path = install
        .workshop
        .as_deref()
        .and_then(mods::workshop_acf_path);
    let acf_mtime = acf_path.as_deref().and_then(mtime_of);
    let mod_dir_mtime = mtime_of(&install.data_dir.join("mod"));
    let manifest_mtime = mtime_of(&install.dir.join("checksum_manifest.txt"));

    let (mods_list, dlcs) = playset::scan(&install);
    let ws_meta = install
        .workshop
        .as_deref()
        .map(mods::workshop_meta)
        .unwrap_or_default();

    let mut meta = HashMap::with_capacity(mods_list.len());
    let mut index_key = Vec::with_capacity(mods_list.len());
    for m in &mods_list {
        let thumb = mods::picture(m).and_then(|p| Some((m.root.clone()?, p)));
        let ws = m.remote_id.as_deref().and_then(|id| ws_meta.get(id));
        let (acf_size, updated, stamp) = match ws {
            Some(&(sz, ts)) => (
                Some(sz),
                (ts > 0).then_some(ts),
                (ts > 0)
                    .then_some(ts)
                    .or_else(|| m.root.as_deref().and_then(mtime_secs)),
            ),
            None => {
                let desc_ts = mtime_secs(&install.data_dir.join(&m.id));
                let root_ts = m.root.as_deref().and_then(mtime_secs);
                (None, desc_ts.or(root_ts), root_ts.or(desc_ts))
            }
        };
        index_key.push((m.id.clone(), m.root.clone(), stamp));
        meta.insert(
            m.id.clone(),
            ModMeta {
                thumb,
                updated,
                acf_size,
            },
        );
    }

    let mut dlc_thumbs = HashMap::new();
    for d in &dlcs {
        if let Some(pair) = dlc_thumb_path(&install.dir, d) {
            dlc_thumbs.insert(d.id.clone(), pair);
        }
    }

    let computed = if invalidate {
        Arc::new(OnceLock::new())
    } else {
        match prev {
            Some(old) if old.index_key == index_key && old.manifest_mtime == manifest_mtime => {
                Arc::clone(&old.computed)
            }
            _ => Arc::new(OnceLock::new()),
        }
    };

    Arc::new(Scan {
        install,
        mods: mods_list,
        dlcs,
        meta,
        dlc_thumbs,
        index_key,
        acf_mtime,
        mod_dir_mtime,
        manifest_mtime,
        computed,
    })
}

fn fnv1a_files(files: &Files) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut write = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    };
    for opt in [&files.game, &files.pdx] {
        match opt {
            None => write(&[0]),
            Some(s) => {
                write(&[1]);
                write(&(s.len() as u64).to_le_bytes());
                write(s.as_bytes());
            }
        }
    }
    format!("{h:016x}")
}

fn check_at(g: &GameState, at: &At) -> AppResult<()> {
    if g.active == at.index && g.playsets.get(g.active).is_some_and(|p| p.name == at.name) {
        Ok(())
    } else {
        Err(AppError::stale("playsets changed outside Regent"))
    }
}

fn enabled_order(p: &Playset, mods: &[Mod]) -> Vec<String> {
    p.visible(mods)
        .into_iter()
        .filter(|(e, _)| e.enabled)
        .map(|(e, _)| e.id.clone())
        .collect()
}

/// Opaque identity of a risky plan: kind, the playset it would launch with (entries, order and
/// disabled DLCs) and the missing ids. FNV-1a over length-prefixed parts, so no id can alias another.
fn plan_token(plan: &saves::Plan, g: &GameState) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut part = |s: &str| {
        for &b in (s.len() as u64).to_le_bytes().iter().chain(s.as_bytes()) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    };
    let playset = |p: &Playset, part: &mut dyn FnMut(&str)| {
        part(&p.name);
        for e in &p.mods {
            part(&e.id);
            part(if e.enabled { "1" } else { "0" });
        }
        part("|dlc");
        for d in &p.disabled_dlcs {
            part(d);
        }
    };
    match plan {
        saves::Plan::Use(i) => {
            part("use");
            if let Some(p) = g.playsets.get(*i) {
                playset(p, &mut part);
            }
        }
        saves::Plan::Unknown => {
            part("unknown");
            if let Some(p) = g.playsets.get(g.active) {
                playset(p, &mut part);
            }
        }
        saves::Plan::Make(p, missing) => {
            part("make");
            playset(p, &mut part);
            part("|missing");
            for m in missing {
                part(m);
            }
        }
    }
    format!("{h:016x}")
}

fn save_needs_confirm(plan: &saves::Plan, g: &GameState) -> SavePlayOutcome {
    let token = plan_token(plan, g);
    match plan {
        saves::Plan::Unknown => SavePlayOutcome::NeedsConfirm {
            reason: "This save was not played through Regent, so its playset is unknown."
                .to_string(),
            missing: vec![],
            token,
        },
        saves::Plan::Make(_, missing) => SavePlayOutcome::NeedsConfirm {
            reason: "This save requires mods that are not installed.".to_string(),
            missing: missing.clone(),
            token,
        },
        saves::Plan::Use(_) => unreachable!(),
    }
}

/// `expect` is the token of the plan the user confirmed. A different risky plan asks again
/// (writes nothing); a plan that became safe is stale.
fn check_save_plan(
    plan: &saves::Plan,
    g: &GameState,
    force: bool,
    expect: Option<&str>,
) -> AppResult<Option<SavePlayOutcome>> {
    if !force && plan.risky() {
        return Ok(Some(save_needs_confirm(plan, g)));
    }
    if let Some(exp) = expect {
        if !plan.risky() {
            return Err(AppError::stale(
                "save plan changed since confirmation; reload and try again",
            ));
        }
        if plan_token(plan, g) != exp {
            return Ok(Some(save_needs_confirm(plan, g)));
        }
    }
    Ok(None)
}

fn save_plan_dto(plan: &saves::Plan, g: &GameState) -> SavePlan {
    match plan {
        saves::Plan::Use(i) => SavePlan::Use {
            playset: g
                .playsets
                .get(*i)
                .map_or_else(String::new, |p| p.name.clone()),
        },
        saves::Plan::Make(p, missing) => SavePlan::Make {
            playset: p.name.clone(),
            missing: missing.clone(),
        },
        saves::Plan::Unknown => SavePlan::Unknown,
    }
}

fn shown_setting_value(kind: Kind, v: &str) -> String {
    match (kind, v.parse::<f64>()) {
        (Kind::Slider { .. }, Ok(x)) => x.to_string(),
        _ => v.to_string(),
    }
}

fn build_settings_view(install: &Install, roots: &Roots, files: &Files) -> SettingsView {
    let langs = st::language_options(install);
    let mut fields = vec![];
    for f in &st::FIELDS {
        let Some(r) = f.read(files) else { continue };
        let (kind, options) = match f.kind {
            Kind::Toggle => (FieldKind::Toggle, None),
            Kind::Slider { min, max, step } => (FieldKind::Slider { min, max, step }, None),
            Kind::Int { max } => (FieldKind::Int { max }, None),
            Kind::Resolution => (FieldKind::Resolution, None),
            Kind::Mode => (
                FieldKind::Mode,
                Some(
                    st::MODES
                        .iter()
                        .map(|(l, v)| FieldOption {
                            label: (*l).to_string(),
                            value: (*v).to_string(),
                        })
                        .collect(),
                ),
            ),
            Kind::Language => (
                FieldKind::Language,
                (!langs.is_empty()).then(|| {
                    langs
                        .iter()
                        .map(|(l, v)| FieldOption {
                            label: l.clone(),
                            value: v.clone(),
                        })
                        .collect()
                }),
            ),
        };
        fields.push(SettingsField {
            id: f.id.to_string(),
            label: f.label.to_string(),
            kind,
            options,
            value: shown_setting_value(f.kind, &r.value),
            note: r.note,
        });
    }
    let raw = RawSettings {
        game: files.game.as_deref().map(st::to_lf),
        pdx: files.pdx.as_deref().map(st::to_lf),
    };
    let paths = SettingsPaths {
        game: st::path(install, File::Game).display().to_string(),
        pdx: st::path(install, File::Pdx).display().to_string(),
    };
    let backups = st::backups(install, roots)
        .into_iter()
        .map(|b| BackupInfo {
            id: b.id,
            at: b.id,
            files: b.files.iter().map(|f| f.name().to_string()).collect(),
        })
        .collect();
    SettingsView {
        rev: fnv1a_files(files),
        fields,
        raw,
        paths,
        backups,
    }
}

fn spawn_os_open(arg: &str) -> AppResult<()> {
    let prog = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(prog)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::failed(format!("failed to open {arg}: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------- Backend

impl Backend {
    pub fn new(roots: Roots, fixture: bool) -> Self {
        Self::with_emitter(roots, fixture, Arc::new(|_| {}))
    }

    pub fn with_emitter(roots: Roots, fixture: bool, emit: Emitter) -> Self {
        Self {
            roots,
            fixture,
            scans: RwLock::new(Scans {
                installs: vec![],
                by_game: HashMap::new(),
                generation: HashMap::new(),
            }),
            running: Arc::new(Mutex::new(HashMap::new())),
            emit,
            default_wallpapers_started: AtomicBool::new(false),
            wallpaper_lock: Mutex::new(()),
            previews_tried: Mutex::new(HashSet::new()),
        }
    }

    fn app_dir(&self) -> PathBuf {
        playset::app_dir(&self.roots)
    }

    fn game_art(&self, install: &Install) -> GameArt {
        let app_dir = self.app_dir();
        let has = |which| {
            art::target(&app_dir, install, which)
                .is_some_and(|(root, path)| mods::contained_file(&root, &path).is_some())
        };
        GameArt {
            logo: has("logo"),
            background: has("background"),
            wallpaper: has("wallpaper"),
            custom: art::custom_wallpaper(&app_dir, install).is_some(),
        }
    }

    /// Once per session, download missing curated wallpapers off-thread. Offline is silent.
    fn fetch_default_wallpapers(&self, installs: &[Install]) {
        if self.fixture || self.default_wallpapers_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let app_dir = self.app_dir();
        let wanted: Vec<Install> = installs
            .iter()
            .filter(|i| i.def.wallpaper.is_some())
            .filter(|i| art::default_wallpaper(&app_dir, i).is_none())
            .cloned()
            .collect();
        if wanted.is_empty() {
            return;
        }
        let emit = Arc::clone(&self.emit);
        let _ = std::thread::Builder::new()
            .name("regent-wallpapers".into())
            .spawn(move || {
                for install in wanted {
                    if art::fetch_default_wallpaper(&app_dir, &install) {
                        emit(RefreshEvent::images("art", install.def.id));
                    }
                }
            });
    }

    /// Off-thread Steam workshop previews for mods that ship no local image.
    fn fetch_mod_previews(&self, scan: &Scan) {
        if self.fixture {
            return;
        }
        let (game, app_dir) = (scan.install.def.id, self.app_dir());
        let Ok(mut tried) = self.previews_tried.lock() else {
            return;
        };
        let ids: Vec<String> = scan
            .mods
            .iter()
            .filter(|m| scan.meta.get(&m.id).is_none_or(|x| x.thumb.is_none()))
            .filter_map(|m| m.remote_id.as_deref())
            .filter(|id| art::is_workshop_id(id))
            .filter(|id| art::steam_preview(&app_dir, &scan.install, id).is_none())
            .filter(|id| tried.insert((game, id.to_string())))
            .map(str::to_owned)
            .collect();
        if ids.is_empty() {
            return;
        }
        let emit = Arc::clone(&self.emit);
        let _ = std::thread::Builder::new()
            .name(format!("regent-previews-{game}"))
            .spawn(move || {
                art::fetch_previews(&app_dir, game, &ids, || {
                    emit(RefreshEvent::images("thumbs", game));
                });
            });
    }

    fn write_lock(&self) -> AppResult<playset::AppLock> {
        playset::lock_app(&self.roots).map_err(AppError::from_anyhow)
    }

    fn store_path(&self) -> PathBuf {
        playset::store_path(&self.roots)
    }

    fn refresh_installs(&self) -> Vec<Install> {
        let installs = game::discover(&self.roots);
        if let Ok(mut guard) = self.scans.write() {
            guard
                .by_game
                .retain(|id, _| installs.iter().any(|i| i.def.id == *id));
            guard.installs = installs.clone();
        }
        installs
    }

    fn installs_snapshot(&self) -> Vec<Install> {
        let cached = self
            .scans
            .read()
            .map(|g| g.installs.clone())
            .unwrap_or_default();
        if cached.is_empty() {
            self.refresh_installs()
        } else {
            cached
        }
    }

    fn install_for(&self, game_id: &str) -> AppResult<(Install, Vec<Install>)> {
        let mut installs = self.installs_snapshot();
        if let Some(i) = installs.iter().find(|i| i.def.id == game_id).cloned() {
            return Ok((i, installs));
        }
        installs = self.refresh_installs();
        installs
            .iter()
            .find(|i| i.def.id == game_id)
            .cloned()
            .map(|i| (i, installs))
            .ok_or_else(|| AppError::failed(format!("game '{game_id}' is not installed")))
    }

    fn cached_scan(&self, game_id: &str) -> Option<Arc<Scan>> {
        self.scans
            .read()
            .ok()
            .and_then(|g| g.by_game.get(game_id).cloned())
    }

    fn bump_generation(&self, game_id: &'static str) -> u64 {
        let mut g = self.scans.write().unwrap_or_else(|e| e.into_inner());
        let slot = g.generation.entry(game_id).or_insert(0);
        *slot += 1;
        *slot
    }

    /// Publish `scan` only if `generation` is still the current generation for this game.
    /// Returns true if published, false if superseded by a newer scan.
    fn publish_scan(&self, scan: Arc<Scan>, generation: u64) -> bool {
        let mut g = self.scans.write().unwrap_or_else(|e| e.into_inner());
        let id = scan.install.def.id;
        if g.generation.get(id).copied() == Some(generation) {
            g.by_game.insert(id, scan);
            true
        } else {
            false
        }
    }

    fn get_or_scan(&self, game_id: &str, force: bool) -> AppResult<(Arc<Scan>, Vec<Install>)> {
        let (install, installs) = self.install_for(game_id)?;
        let prev = self.cached_scan(install.def.id);
        let need_scan = match &prev {
            None => true,
            Some(_) if force => true,
            Some(old) => {
                let acf_now = install
                    .workshop
                    .as_deref()
                    .and_then(mods::workshop_acf_path)
                    .as_deref()
                    .and_then(mtime_of);
                let mod_now = mtime_of(&install.data_dir.join("mod"));
                let manifest_now = mtime_of(&install.dir.join("checksum_manifest.txt"));
                acf_now != old.acf_mtime
                    || mod_now != old.mod_dir_mtime
                    || manifest_now != old.manifest_mtime
            }
        };
        let scan = if need_scan {
            let generation = self.bump_generation(install.def.id);
            let fresh = build_scan(install, prev.as_ref(), force);
            if self.publish_scan(Arc::clone(&fresh), generation) {
                self.fetch_mod_previews(&fresh);
                fresh
            } else {
                self.cached_scan(fresh.install.def.id).unwrap_or(fresh)
            }
        } else {
            prev.expect("checked Some above")
        };
        Ok((scan, installs))
    }

    fn mutate_playset<F>(&self, game_id: &str, at: Option<&At>, f: F) -> AppResult<GameStateDto>
    where
        F: FnOnce(&mut GameState, &[Mod], &[Dlc]) -> AppResult<()>,
    {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let path = self.store_path();
        // Fast-fail before creating a lockfile if playsets.json is already corrupt/unreadable.
        let (mut pre, pre_warn) = Store::peek(&path, &installs);
        if pre.read_only {
            return Err(AppError::read_only(
                pre_warn.unwrap_or_else(|| "playsets.json is read-only".into()),
            ));
        }
        // A stale caller must not create the lockfile; checked again under the lock.
        if let Some(expected) = at {
            check_at(pre.game(scan.install.def.id), expected)?;
        }
        let _w = self.write_lock()?;
        // Ordinary commands never quarantine playsets.json: use peek and fail closed.
        let (mut store, warning) = Store::peek(&path, &installs);
        if store.read_only {
            return Err(AppError::read_only(
                warning.unwrap_or_else(|| "playsets.json is read-only".into()),
            ));
        }
        if store.bad_dlc_load.contains(scan.install.def.id) {
            return Err(AppError::failed(Store::bad_seed_message(&scan.install)));
        }
        let g = store.game(scan.install.def.id);
        if let Some(expected) = at {
            check_at(g, expected)?;
        }
        g.current().reconcile(&scan.mods);
        f(g, &scan.mods, &scan.dlcs)?;
        g.current().reconcile(&scan.mods);
        let out = GameStateDto::from(g.clone());
        store.save(&path).map_err(AppError::from_anyhow)?;
        Ok(out)
    }

    fn reserve_launch(&self, game_id: &'static str) -> AppResult<LaunchGuard> {
        let mut r = self
            .running
            .lock()
            .map_err(|_| AppError::failed("internal lock poisoned"))?;
        if r.get(game_id).copied().unwrap_or(0) > 0 {
            return Err(AppError::failed(format!(
                "{game_id} is already starting or running"
            )));
        }
        *r.entry(game_id).or_insert(0) += 1;
        Ok(LaunchGuard {
            running: Arc::clone(&self.running),
            game: game_id,
        })
    }

    fn spawn_and_watch(
        self: &Arc<Self>,
        install: Install,
        mut child: Child,
        guard: LaunchGuard,
    ) -> u32 {
        let pid = child.id();
        let game_id = install.def.id;
        let this = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name(format!("regent-watch-{game_id}"))
            .spawn(move || {
                let _ = child.wait();
                let warning = {
                    let _lock = playset::lock_app(&this.roots);
                    saves::sweep(&install, &this.roots)
                        .err()
                        .map(|e| format!("{e:#}"))
                };
                // Drop guard before emitting exit so callers immediately see running == 0.
                drop(guard);
                (this.emit)(RefreshEvent::exit(game_id.to_string(), warning));
            });
        pid
    }

    fn running_count(&self, game_id: &str) -> u32 {
        self.running
            .lock()
            .ok()
            .and_then(|r| r.get(game_id).copied())
            .unwrap_or(0)
    }

    /// `(containment root, path)` behind a `thumb://` key, from cached scan data. Mods without
    /// a local image fall back to their downloaded Steam preview.
    fn thumb_target(&self, game_id: &str, kind: &str, id: &str) -> Option<(PathBuf, PathBuf)> {
        let (scan, install) = {
            let guard = self.scans.read().ok()?;
            let scan = guard.by_game.get(game_id).cloned();
            let install = scan
                .as_ref()
                .map(|s| s.install.clone())
                .or_else(|| guard.installs.iter().find(|i| i.def.id == game_id).cloned());
            (scan, install)
        };
        match kind {
            "mod" => {
                let scan = scan?;
                if let Some(thumb) = &scan.meta.get(id)?.thumb {
                    return Some(thumb.clone());
                }
                let m = scan.mods.iter().find(|m| m.id == id)?;
                let app_dir = self.app_dir();
                let path = art::steam_preview(&app_dir, &scan.install, m.remote_id.as_deref()?)?;
                Some((app_dir, path))
            }
            "dlc" => scan?.dlc_thumbs.get(id).cloned(),
            "art" => art::target(&self.app_dir(), &install?, id),
            _ => None,
        }
    }

    /// Resolve a `thumb://` request key `(game, kind, id)`, verifying canonical containment in
    /// the expected root and rejecting symlinks.
    pub fn resolve_thumb(&self, game_id: &str, kind: &str, id: &str) -> Option<PathBuf> {
        let (root, path) = self.thumb_target(game_id, kind, id)?;
        mods::contained_file(&root, &path)
    }

    /// Resolve and read a `thumb://` asset with open-time symlink rejection, root containment,
    /// swap detection, and size cap.
    pub fn read_thumb(&self, game_id: &str, kind: &str, id: &str) -> Option<(PathBuf, Vec<u8>)> {
        let (root, path) = self.thumb_target(game_id, kind, id)?;
        let bytes = mods::read_contained_file(&root, &path, art::MAX_IMAGE_BYTES)?;
        Some((path, bytes))
    }

    // ------------------------------------------------------------ commands

    pub fn games(&self) -> AppResult<GamesResponse> {
        let installs = self.refresh_installs();
        self.fetch_default_wallpapers(&installs);
        let (_, warning) = Store::peek(&self.store_path(), &installs);
        let games = installs.iter().map(GameInfo::from_install).collect();
        Ok(GamesResponse {
            games,
            fixture: self.fixture,
            warning,
        })
    }

    pub fn load(&self, game_id: &str, rescan: bool) -> AppResult<GameView> {
        // Strictly read-only: never sweep or write saves.json / playsets.json on load or rescan.
        let (scan, installs) = self.get_or_scan(game_id, rescan)?;
        let (mut store, warning) = Store::peek(&self.store_path(), &installs);
        let read_only = store.read_only;
        let g = store.game(scan.install.def.id);
        g.current().reconcile(&scan.mods);
        let state = GameStateDto::from(g.clone());

        let computed_sizes = scan.computed.get().map(|c| &c.sizes);
        let app_dir = self.app_dir();
        let mods = scan
            .mods
            .iter()
            .map(|m| {
                let mm = scan.meta.get(&m.id);
                let size = mm
                    .and_then(|x| x.acf_size)
                    .or_else(|| computed_sizes.and_then(|s| s.get(&m.id).copied()));
                ModInfo {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    supported_version: m.supported_version.clone(),
                    mismatch: m.mismatch(scan.install.raw_version.as_deref()),
                    remote_id: m.remote_id.clone(),
                    dependencies: m.dependencies.clone(),
                    has_thumb: mm.is_some_and(|x| x.thumb.is_some())
                        || m.remote_id.as_deref().is_some_and(|id| {
                            art::steam_preview(&app_dir, &scan.install, id).is_some()
                        }),
                    updated: mm.and_then(|x| x.updated),
                    size,
                }
            })
            .collect();
        let dlcs = scan
            .dlcs
            .iter()
            .map(|d| DlcInfo {
                id: d.id.clone(),
                name: d.name.clone(),
                category: d.category.clone(),
                affects_checksum: d.affects_checksum,
                has_thumb: scan.dlc_thumbs.contains_key(&d.id),
            })
            .collect();
        let art = self.game_art(&scan.install);
        let crash_count = crash::list(&scan.install.data_dir).len();
        let running = self.running_count(scan.install.def.id);
        Ok(GameView {
            game: scan.install.def.id.to_string(),
            art,
            mods,
            dlcs,
            state,
            read_only,
            warning,
            running,
            crash_count,
        })
    }

    /// Download `url` (https or a wallhaven page link / id) as this game's wallpaper.
    pub fn wallpaper_set_url(&self, game_id: &str, url: &str) -> AppResult<GameArt> {
        let (install, _) = self.install_for(game_id)?;
        let source = art::parse_wallpaper_url(url).map_err(AppError::failed)?;
        let bytes = art::download_wallpaper(&source).map_err(AppError::failed)?;
        self.save_wallpaper(&install, &bytes)
    }

    /// Copy a user-picked image file in as this game's wallpaper.
    pub fn wallpaper_set_file(&self, game_id: &str, path: &Path) -> AppResult<GameArt> {
        let (install, _) = self.install_for(game_id)?;
        let bytes = art::read_picked_file(path).map_err(AppError::failed)?;
        self.save_wallpaper(&install, &bytes)
    }

    fn save_wallpaper(&self, install: &Install, bytes: &[u8]) -> AppResult<GameArt> {
        let _one = self
            .wallpaper_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        art::set_custom_wallpaper(&self.app_dir(), install, bytes).map_err(AppError::failed)?;
        Ok(self.game_art(install))
    }

    pub fn wallpaper_reset(&self, game_id: &str) -> AppResult<GameArt> {
        let (install, _) = self.install_for(game_id)?;
        let _one = self
            .wallpaper_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        art::reset_custom_wallpaper(&self.app_dir(), &install)
            .map_err(|e| AppError::failed(format!("Could not reset the wallpaper: {e}")))?;
        Ok(self.game_art(&install))
    }

    pub fn summary(&self, game_id: &str) -> AppResult<Summary> {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let computed = scan.ensure_computed();
        let (mut store, _) = Store::peek(&self.store_path(), &installs);
        let p = store.game(scan.install.def.id).current();
        p.reconcile(&scan.mods);

        let order = enabled_order(p, &scan.mods);
        let found = conflicts::conflicts(&computed.index, &order);
        let mut conflicts_by_mod: HashMap<String, usize> = HashMap::new();
        for c in &found {
            for id in &c.mods {
                *conflicts_by_mod.entry(id.clone()).or_default() += 1;
            }
        }
        let pairs = conflicts::pairs(&found)
            .into_iter()
            .map(|pr| ConflictPairSummary {
                winner: pr.winner,
                loser: pr.loser,
                files: pr.paths.len(),
            })
            .collect();
        let (suggested, reasons) = conflicts::suggest(p, &scan.mods);
        let changed = suggested
            .iter()
            .map(|e| e.id.as_str())
            .ne(p.mods.iter().map(|e| e.id.as_str()));
        let checksum = conflicts::checksum_report(&computed.index, p, &scan.mods);
        Ok(Summary {
            has_manifest: computed.index.has_manifest,
            pairs,
            conflicts_by_mod,
            suggest: SuggestSummary {
                order: suggested,
                reasons,
                changed,
            },
            checksum,
            sizes: computed.sizes.clone(),
        })
    }

    pub fn conflict_files(
        &self,
        game_id: &str,
        winner: &str,
        loser: &str,
    ) -> AppResult<Vec<String>> {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let computed = scan.ensure_computed();
        let (mut store, _) = Store::peek(&self.store_path(), &installs);
        let p = store.game(scan.install.def.id).current();
        p.reconcile(&scan.mods);
        let order = enabled_order(p, &scan.mods);
        let found = conflicts::conflicts(&computed.index, &order);
        let paths = conflicts::pairs(&found)
            .into_iter()
            .find(|pr| pr.winner == winner && pr.loser == loser)
            .map(|pr| pr.paths)
            .unwrap_or_default();
        Ok(paths)
    }

    pub fn set_mods_enabled(
        &self,
        game_id: &str,
        at: &At,
        ids: &[String],
        on: bool,
    ) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, Some(at), |g, _, _| {
            let p = g.current();
            for id in ids {
                if let Some(e) = p.mods.iter_mut().find(|e| e.id == *id) {
                    e.enabled = on;
                }
            }
            Ok(())
        })
    }

    pub fn move_mod(
        &self,
        game_id: &str,
        at: &At,
        id: &str,
        before_id: Option<&str>,
    ) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, Some(at), |g, _, _| {
            let p = g.current();
            if before_id == Some(id) {
                return Ok(());
            }
            let Some(from) = p.mods.iter().position(|e| e.id == id) else {
                return Err(AppError::failed(format!("no such mod: {id}")));
            };
            if let Some(target) = before_id
                && !p.mods.iter().any(|e| e.id == target)
            {
                return Err(AppError::failed(format!("no such mod: {target}")));
            }
            let entry = p.mods.remove(from);
            match before_id {
                Some(target) => {
                    let pos = p
                        .mods
                        .iter()
                        .position(|e| e.id == target)
                        .expect("verified above");
                    p.mods.insert(pos, entry);
                }
                None => p.mods.push(entry),
            }
            Ok(())
        })
    }

    pub fn apply_order(&self, game_id: &str, at: &At) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, Some(at), |g, mods, _| {
            let (entries, _) = conflicts::suggest(g.current(), mods);
            g.current().mods = entries;
            Ok(())
        })
    }

    pub fn set_dlcs_enabled(
        &self,
        game_id: &str,
        at: &At,
        ids: &[String],
        on: bool,
    ) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, Some(at), |g, _, _| {
            let p = g.current();
            for id in ids {
                if on {
                    p.disabled_dlcs.retain(|d| d != id);
                } else if !p.disabled_dlcs.iter().any(|d| d == id) {
                    p.disabled_dlcs.push(id.clone());
                }
            }
            Ok(())
        })
    }

    pub fn playset_new(&self, game_id: &str, name: &str) -> AppResult<GameStateDto> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::failed("playset name cannot be empty"));
        }
        self.mutate_playset(game_id, None, |g, _, _| {
            g.add(Playset::new(name));
            Ok(())
        })
    }

    pub fn playset_copy(
        &self,
        game_id: &str,
        at: &At,
        name: Option<&str>,
    ) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, Some(at), |g, _, _| {
            let mut p = g.current().duplicate();
            if let Some(n) = name {
                p.rename(n);
            }
            g.add(p);
            Ok(())
        })
    }

    pub fn playset_rename(&self, game_id: &str, at: &At, name: &str) -> AppResult<GameStateDto> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::failed("playset name cannot be empty"));
        }
        self.mutate_playset(game_id, Some(at), |g, _, _| {
            g.current().rename(name);
            Ok(())
        })
    }

    pub fn playset_use(&self, game_id: &str, index: usize, name: &str) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, None, |g, _, _| {
            if g.playsets.get(index).is_some_and(|p| p.name == name) {
                g.active = index;
                Ok(())
            } else {
                Err(AppError::stale("playsets changed outside Regent"))
            }
        })
    }

    pub fn set_launch_args(&self, game_id: &str, args: &str) -> AppResult<GameStateDto> {
        self.mutate_playset(game_id, None, |g, _, _| {
            g.launch_args = args.to_string();
            Ok(())
        })
    }

    pub fn launch(
        self: &Arc<Self>,
        game_id: &str,
        at: &At,
        extra: &[String],
    ) -> AppResult<LaunchInfo> {
        let (install, installs) = self.install_for(game_id)?;
        let path = self.store_path();
        // Fast-fail before creating a lockfile if store or dlc_load is corrupt.
        let (mut pre, pre_warn) = Store::peek(&path, &installs);
        if pre.read_only {
            return Err(AppError::read_only(pre_warn.unwrap_or_else(|| {
                "playsets.json is unreadable; refusing to launch from a guessed playset".into()
            })));
        }
        if pre.bad_dlc_load.contains(install.def.id) {
            return Err(AppError::failed(format!(
                "{} exists but could not be parsed; refusing to launch from an empty seeded playset. Back up or fix dlc_load.json first.",
                install.data_dir.join("dlc_load.json").display()
            )));
        }
        check_at(pre.game(install.def.id), at)?;
        saves::check_book_readable(&self.roots).map_err(AppError::from_anyhow)?;
        let (child, guard) = {
            let _w = self.write_lock()?;
            let guard = self.reserve_launch(install.def.id)?;
            let (mut store, store_warn) = Store::peek(&path, &installs);
            if store.read_only {
                return Err(AppError::read_only(store_warn.unwrap_or_else(|| {
                    "playsets.json is unreadable; refusing to launch from a guessed playset".into()
                })));
            }
            if store.bad_dlc_load.contains(install.def.id) {
                return Err(AppError::failed(format!(
                    "{} exists but could not be parsed; refusing to launch from an empty seeded playset. Back up or fix dlc_load.json first.",
                    install.data_dir.join("dlc_load.json").display()
                )));
            }
            let g = store.game(install.def.id);
            check_at(g, at)?;
            let saved_args = g.launch_args.clone();
            // Revalidate mods/descriptors fresh from disk at launch time.
            let (fresh_mods, fresh_dlcs) = playset::scan(&install);
            g.current().reconcile(&fresh_mods);
            let p = g.current().clone();
            let snap = playset::prepare_launch_with_snapshot(
                &install,
                &self.roots,
                &p,
                &fresh_mods,
                &fresh_dlcs,
            )
            .map_err(AppError::from_anyhow)?;
            let mut cmd = game::launch_command(&install, &saved_args);
            cmd.args(extra)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    return Err(AppError::failed(snap.restore_after(format!(
                        "failed to start {}: {e}",
                        install.exe.display()
                    ))));
                }
            };
            (child, guard)
        };
        let pid = self.spawn_and_watch(install, child, guard);
        Ok(LaunchInfo { pid, warning: None })
    }

    pub fn settings_get(&self, game_id: &str) -> AppResult<SettingsView> {
        let (install, _) = self.install_for(game_id)?;
        let files = Files::load(&install).map_err(AppError::from_anyhow)?;
        Ok(build_settings_view(&install, &self.roots, &files))
    }

    pub fn settings_apply(
        &self,
        game_id: &str,
        rev: &str,
        edits: &[FieldEdit],
        raw: &[RawEdit],
    ) -> AppResult<SettingsApplyResponse> {
        let (install, _) = self.install_for(game_id)?;
        let _w = self.write_lock()?;
        let orig = Files::load(&install).map_err(AppError::from_anyhow)?;
        if fnv1a_files(&orig) != rev {
            return Err(AppError::stale("settings changed on disk; reloaded"));
        }
        let mut base = orig.clone();
        for r in raw {
            let f = r.file.to_file();
            let Some(orig_text) = orig.get(f) else {
                return Err(AppError::failed(format!("{} does not exist", f.name())));
            };
            *base.slot(f) = Some(st::restore_eol(orig_text, &r.text));
        }
        let langs = st::language_options(&install);
        let mut planned = vec![];
        for e in edits {
            let Some(field) = st::field(&e.field) else {
                return Err(AppError::failed(format!("unknown setting '{}'", e.field)));
            };
            if field.kind == Kind::Language
                && !langs.is_empty()
                && !langs.iter().any(|(_, v)| v == &e.value)
            {
                return Err(AppError::failed(format!(
                    "{}: unknown language '{}'",
                    field.label, e.value
                )));
            }
            let step = field
                .plan(&e.value, &base)
                .map_err(|err| AppError::failed(format!("{}: {err:#}", field.label)))?;
            planned.extend(step);
        }
        let new = st::edit(&base, &planned).map_err(AppError::from_anyhow)?;
        let backup_dir =
            st::write(&install, &self.roots, &orig, &new).map_err(AppError::from_anyhow)?;
        let fresh = Files::load(&install).map_err(AppError::from_anyhow)?;
        Ok(SettingsApplyResponse {
            view: build_settings_view(&install, &self.roots, &fresh),
            backup_dir: backup_dir.map(|p| p.display().to_string()),
        })
    }

    pub fn settings_rollback(&self, game_id: &str, id: u64) -> AppResult<SettingsView> {
        let (install, _) = self.install_for(game_id)?;
        let _w = self.write_lock()?;
        let restored =
            st::rollback_by_id(&install, &self.roots, id).map_err(AppError::from_anyhow)?;
        if restored.is_none() {
            return Err(AppError::stale("backup no longer exists"));
        }
        let fresh = Files::load(&install).map_err(AppError::from_anyhow)?;
        Ok(build_settings_view(&install, &self.roots, &fresh))
    }

    fn build_saves_response(&self, scan: &Scan, installs: &[Install]) -> AppResult<SavesResponse> {
        let (mut store, store_warning) = Store::peek(&self.store_path(), installs);
        let g = store.game(scan.install.def.id);
        g.current().reconcile(&scan.mods);
        let (bound_rows, book_warning) = saves::bound_peek(&scan.install, &self.roots);
        let all: Vec<saves::Save> = bound_rows.iter().map(|b| b.save.clone()).collect();
        let rows = bound_rows
            .into_iter()
            .map(|b| {
                let binding = SaveBinding::from(saves::binding(&b, g, &scan.mods, &scan.dlcs));
                let plan = save_plan_dto(&saves::plan(&b, g, &scan.mods, &scan.dlcs), g);
                let can_continue = saves::continue_arg(&scan.install, &b.save, &all).is_some();
                let meta = b.save.meta.clone().unwrap_or_default();
                SaveRow {
                    title: b.save.title(),
                    rel: b.save.rel,
                    date: meta.date,
                    version: meta.version,
                    mtime: b.save.mtime,
                    binding,
                    plan,
                    can_continue,
                }
            })
            .collect();
        let warning = match (store_warning, book_warning) {
            (Some(a), Some(b)) => Some(format!("{a}; {b}")),
            (a, b) => a.or(b),
        };
        Ok(SavesResponse { rows, warning })
    }

    pub fn saves_list(&self, game_id: &str) -> AppResult<SavesResponse> {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        self.build_saves_response(&scan, &installs)
    }

    pub fn saves_bind(&self, game_id: &str, at: &At, rel: &str) -> AppResult<SavesResponse> {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let path = self.store_path();
        let (mut pre, pre_warn) = Store::peek(&path, &installs);
        if pre.read_only {
            return Err(AppError::read_only(
                pre_warn.unwrap_or_else(|| "playsets.json is read-only".into()),
            ));
        }
        check_at(pre.game(scan.install.def.id), at)?;
        saves::check_book_readable(&self.roots).map_err(AppError::from_anyhow)?;
        {
            let _w = self.write_lock()?;
            let (mut store, warning) = Store::peek(&path, &installs);
            if store.read_only {
                return Err(AppError::read_only(
                    warning.unwrap_or_else(|| "playsets.json is read-only".into()),
                ));
            }
            let g = store.game(scan.install.def.id);
            check_at(g, at)?;
            g.current().reconcile(&scan.mods);
            let snap = playset::dlc_load(g.current(), &scan.mods, &scan.dlcs);
            saves::bind(&scan.install, &self.roots, rel, &snap).map_err(AppError::from_anyhow)?;
        }
        self.build_saves_response(&scan, &installs)
    }

    pub fn saves_play(
        self: &Arc<Self>,
        game_id: &str,
        at: &At,
        rel: &str,
        force: bool,
        expect: Option<&str>,
    ) -> AppResult<SavePlayOutcome> {
        let (install, installs) = self.install_for(game_id)?;
        let path = self.store_path();
        // Locate only the selected save outside the lock (no collection scan or zip meta reads).
        let Some(pre_save) = saves::find_one(&install, &self.roots, rel) else {
            return Err(AppError::failed(format!("no such save: {rel}")));
        };
        // Fast-fail before creating a lockfile on read-only, corrupt dlc_load, stale At, or confirmation refusal.
        {
            let (mut pre_store, pre_warn) = Store::peek(&path, &installs);
            if pre_store.read_only {
                return Err(AppError::read_only(pre_warn.unwrap_or_else(|| {
                    "playsets.json is unreadable; refusing to launch from a guessed playset".into()
                })));
            }
            if pre_store.bad_dlc_load.contains(install.def.id) {
                return Err(AppError::failed(format!(
                    "{} exists but could not be parsed; refusing to launch from an empty seeded playset. Back up or fix dlc_load.json first.",
                    install.data_dir.join("dlc_load.json").display()
                )));
            }
            let g = pre_store.game(install.def.id);
            check_at(g, at)?;
            let (fresh_mods, fresh_dlcs) = playset::scan(&install);
            g.current().reconcile(&fresh_mods);
            let (b, _) = saves::bound_one_peek(&install, &self.roots, pre_save);
            let plan = saves::plan(&b, g, &fresh_mods, &fresh_dlcs);
            if let Some(outcome) = check_save_plan(&plan, g, force, expect)? {
                return Ok(outcome);
            }
            saves::check_book_readable(&self.roots).map_err(AppError::from_anyhow)?;
        }
        let (child, guard, state, using, manual_load, warning) = {
            let _w = self.write_lock()?;
            let guard = self.reserve_launch(install.def.id)?;
            let (mut store, store_warn) = Store::peek(&path, &installs);
            if store.read_only {
                return Err(AppError::read_only(store_warn.unwrap_or_else(|| {
                    "playsets.json is unreadable; refusing to launch from a guessed playset".into()
                })));
            }
            if store.bad_dlc_load.contains(install.def.id) {
                return Err(AppError::failed(format!(
                    "{} exists but could not be parsed; refusing to launch from an empty seeded playset. Back up or fix dlc_load.json first.",
                    install.data_dir.join("dlc_load.json").display()
                )));
            }
            let g = store.game(install.def.id);
            check_at(g, at)?;
            // Revalidate only the selected save + its book record under the lock.
            let Some(save) = saves::find_one(&install, &self.roots, rel) else {
                return Err(AppError::failed(format!("no such save: {rel}")));
            };
            let (fresh_mods, fresh_dlcs) = playset::scan(&install);
            g.current().reconcile(&fresh_mods);
            let (b, _) = saves::bound_one_peek(&install, &self.roots, save);
            let plan = saves::plan(&b, g, &fresh_mods, &fresh_dlcs);
            if let Some(outcome) = check_save_plan(&plan, g, force, expect)? {
                return Ok(outcome);
            }
            let using = match &plan {
                saves::Plan::Use(i) => g.playsets[*i].name.clone(),
                saves::Plan::Make(p, _) => p.name.clone(),
                saves::Plan::Unknown => g.current().name.clone(),
            };
            let saved_args = g.launch_args.clone();
            saves::adopt(g, plan, &fresh_mods, &fresh_dlcs);
            g.current().reconcile(&fresh_mods);
            let p = g.current().clone();
            let state = GameStateDto::from(g.clone());
            // Preflight exe, sweep saves (failing closed on error), snapshot & write dlc_load.
            let snap = playset::prepare_launch_with_snapshot(
                &install,
                &self.roots,
                &p,
                &fresh_mods,
                &fresh_dlcs,
            )
            .map_err(AppError::from_anyhow)?;
            let cont = saves::continue_arg(&install, &b.save, &saves::scan(&install, &self.roots));
            let manual_load = cont.is_none();
            let mut cmd = game::launch_command(&install, &saved_args);
            if let Some(arg) = cont {
                cmd.arg(arg);
            }
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    return Err(AppError::failed(snap.restore_after(format!(
                        "failed to start {}: {e}",
                        install.exe.display()
                    ))));
                }
            };
            // Persist the active playset switch only after a successful spawn.
            let warning = store
                .save(&path)
                .err()
                .map(|e| format!("launched, but failed to save playsets.json: {e:#}"));
            (child, guard, state, using, manual_load, warning)
        };
        let pid = self.spawn_and_watch(install, child, guard);
        Ok(SavePlayOutcome::Launched {
            state,
            using,
            manual_load,
            pid,
            warning,
        })
    }

    pub fn crash_list(&self, game_id: &str) -> AppResult<CrashListResponse> {
        let (install, _) = self.install_for(game_id)?;
        let crashes = crash::list(&install.data_dir)
            .into_iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        let has_live = mods::contained_file(
            &install.data_dir,
            &crash::live_dir(&install.data_dir).join("error.log"),
        )
        .is_some();
        Ok(CrashListResponse { crashes, has_live })
    }

    pub fn crash_diagnose(&self, game_id: &str, name: Option<&str>) -> AppResult<CrashReport> {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let dir = match name {
            None => {
                let d = crash::live_dir(&scan.install.data_dir);
                mods::contained_dir(&scan.install.data_dir, &d)
                    .ok_or_else(|| AppError::failed(format!("cannot read {}", d.display())))?
            }
            Some(n) => {
                if !plain_component(n) {
                    return Err(AppError::failed(format!("invalid crash name: {n}")));
                }
                let root = scan.install.data_dir.join("crashes");
                let d = root.join(n);
                mods::contained_dir(&root, &d)
                    .ok_or_else(|| AppError::failed(format!("invalid crash dir: {n}")))?
            }
        };
        let computed = scan.ensure_computed();
        let (mut store, _) = Store::peek(&self.store_path(), &installs);
        let p = store.game(scan.install.def.id).current();
        p.reconcile(&scan.mods);
        let fallback = enabled_order(p, &scan.mods);
        let report = crash::diagnose(
            &dir,
            &computed.index,
            &scan.mods,
            &scan.install.dir,
            &fallback,
        )
        .map_err(AppError::failed)?;
        let lines = report.text();
        Ok(CrashReport { report, lines })
    }

    pub fn share_export(&self, game_id: &str, at: &At) -> AppResult<ShareExportResponse> {
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let (mut store, _) = Store::peek(&self.store_path(), &installs);
        let g = store.game(scan.install.def.id);
        check_at(g, at)?;
        g.current().reconcile(&scan.mods);
        let p = g.current().clone();
        let computed = scan.ensure_computed();
        let payload = share::export(
            scan.install.def.id,
            scan.install.raw_version.as_deref(),
            &p,
            &scan.mods,
            &scan.dlcs,
            &computed.index,
            computed.manifest.as_ref(),
        );
        Ok(ShareExportResponse {
            code: share::to_code(&payload),
            json: share::to_json(&payload),
            fingerprint: payload.fingerprint,
        })
    }

    pub fn share_inspect(&self, game_id: &str, text: &str) -> AppResult<ShareInspectResponse> {
        if text.len() > MAX_SHARE_INPUT_BYTES {
            return Err(AppError::failed("share input is larger than 4 MB"));
        }
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let payload = share::parse(text.as_bytes()).map_err(AppError::from_anyhow)?;
        if !payload.game.is_empty() && payload.game != scan.install.def.id {
            return Err(AppError::failed(format!(
                "this playset is for {}, not {}",
                payload.game, scan.install.def.id
            )));
        }
        let computed = scan.ensure_computed();
        let (mut store, _) = Store::peek(&self.store_path(), &installs);
        let p = store.game(scan.install.def.id).current();
        p.reconcile(&scan.mods);
        let (_, missing) = share::import(&payload, &scan.mods, &scan.dlcs);
        let (diff, ours) = share::verify_with_fingerprint(
            &payload,
            scan.install.raw_version.as_deref(),
            p,
            &scan.mods,
            &scan.dlcs,
            &computed.index,
            computed.manifest.as_ref(),
        );
        let lines = diff.lines();
        let equal = diff.equal();
        let mod_count = payload.mods.iter().filter(|m| m.enabled).count();
        Ok(ShareInspectResponse {
            name: payload.name,
            game: payload.game,
            game_version: payload.game_version,
            mod_count,
            missing,
            diff,
            lines,
            equal,
            theirs: payload.fingerprint,
            ours,
        })
    }

    pub fn share_import(&self, game_id: &str, text: &str) -> AppResult<ShareImportResponse> {
        if text.len() > MAX_SHARE_INPUT_BYTES {
            return Err(AppError::failed("share input is larger than 4 MB"));
        }
        let (scan, installs) = self.get_or_scan(game_id, false)?;
        let payload = share::parse(text.as_bytes()).map_err(AppError::from_anyhow)?;
        if !payload.game.is_empty() && payload.game != scan.install.def.id {
            return Err(AppError::failed(format!(
                "this playset is for {}, not {}",
                payload.game, scan.install.def.id
            )));
        }
        let (p, missing) = share::import(&payload, &scan.mods, &scan.dlcs);
        let path = self.store_path();
        let (pre, pre_warn) = Store::peek(&path, &installs);
        if pre.read_only {
            return Err(AppError::read_only(
                pre_warn.unwrap_or_else(|| "playsets.json is read-only".into()),
            ));
        }
        let _w = self.write_lock()?;
        // Ordinary commands never quarantine playsets.json: use peek and fail closed.
        let (mut store, warning) = Store::peek(&path, &installs);
        if store.read_only {
            return Err(AppError::read_only(
                warning.unwrap_or_else(|| "playsets.json is read-only".into()),
            ));
        }
        if store.bad_dlc_load.contains(scan.install.def.id) {
            return Err(AppError::failed(Store::bad_seed_message(&scan.install)));
        }
        let g = store.game(scan.install.def.id);
        let name = share::adopt(g, p, &scan.mods);
        let state = GameStateDto::from(g.clone());
        store.save(&path).map_err(AppError::from_anyhow)?;
        Ok(ShareImportResponse {
            state,
            name,
            missing,
        })
    }

    pub fn open(&self, target: OpenTarget) -> AppResult<()> {
        match target {
            OpenTarget::Workshop { id } => {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(AppError::failed("invalid workshop id"));
                }
                spawn_os_open(&share::workshop_url(&id))
            }
            OpenTarget::CrashDir { game, name } => {
                let (install, _) = self.install_for(&game)?;
                let dir = match name.as_deref() {
                    None | Some("") => {
                        let d = crash::live_dir(&install.data_dir);
                        mods::contained_dir(&install.data_dir, &d).ok_or_else(|| {
                            AppError::failed(format!("folder does not exist: {}", d.display()))
                        })?
                    }
                    Some(n) => {
                        if !plain_component(n) {
                            return Err(AppError::failed(format!("invalid crash name: {n}")));
                        }
                        let root = install.data_dir.join("crashes");
                        let d = root.join(n);
                        mods::contained_dir(&root, &d).ok_or_else(|| {
                            AppError::failed(format!("folder does not exist: {}", d.display()))
                        })?
                    }
                };
                spawn_os_open(&dir.to_string_lossy())
            }
            OpenTarget::Backups { game } => {
                let (install, _) = self.install_for(&game)?;
                let root = playset::app_dir(&self.roots);
                let dir = root.join("backups").join(install.def.id);
                let canon = mods::contained_dir(&root, &dir)
                    .ok_or_else(|| AppError::failed("no settings backups folder yet"))?;
                spawn_os_open(&canon.to_string_lossy())
            }
        }
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{put, tmp_dir};
    use crate::saves::tests::{dl, put_save, sav};
    use std::sync::mpsc;
    use std::time::Duration;

    const LAUNCHER_JSON: &str = r#"{
        "gameId": "stellaris",
        "version": "Corona Borealis v4.6.0 (3d1a)",
        "rawVersion": "v4.6.0",
        "gameDataPath": "~/Documents/Paradox Interactive/Stellaris",
        "exePath": "/usr/bin/true",
        "exeArgs": ["-gdpr-compliant"]
    }"#;

    const SETTINGS_GAME: &str = "language=\"l_english\"\ngraphics=\n{\n\tsize=\n\t{\n\t\tx=1920\n\t\ty=1080\n\t}\n\tgui_scale=1.000000\n\tfullScreen=no\n\tborderless=yes\n\tvsync=yes\n}\nmaster_volume=100.000000\nautosave=4\n";
    const SETTINGS_PDX: &str =
        "\"Graphics\"=\n{\n\t\"vsync\"=\n\t{\n\t\tenabled=yes\n\t\tversion=0\n\t}\n}\n";

    fn setup(tag: &str) -> (PathBuf, Arc<Backend>, mpsc::Receiver<RefreshEvent>) {
        let base = tmp_dir(tag);
        let g = base.join("steam/steamapps/common/Stellaris");
        put(&g.join("launcher-settings.json"), LAUNCHER_JSON);
        put(
            &g.join("checksum_manifest.txt"),
            "directory\nname = common\nsub_directories = yes\nfile_extension = .txt\n",
        );
        put(
            &g.join("dlc/dlc014_utopia/dlc014.dlc"),
            "name=\"Utopia\"\narchive=\"dlc/dlc014_utopia/dlc014.zip\"\ncategory=\"expansion\"\nthumbnail=\"thumbnail.png\"\n",
        );
        put(&g.join("dlc/dlc014_utopia/dlc014.zip"), "z");
        put(&g.join("dlc/dlc014_utopia/thumbnail.png"), "PNG");

        let data = base.join("docs/Paradox Interactive/Stellaris");
        for (file, name, extra) in [
            ("a.mod", "Alpha", "supported_version=\"v4.6.*\"\n"),
            ("b.mod", "Beta", "supported_version=\"v4.4.*\"\n"),
            ("c.mod", "Gamma", "supported_version=\"v4.6.*\"\n"),
        ] {
            let dir_name = file.trim_end_matches(".mod");
            put(
                &data.join("mod").join(dir_name).join("common/x.txt"),
                &format!("content-{dir_name}"),
            );
            put(
                &data.join("mod").join(file),
                &format!("name=\"{name}\"\n{extra}path=\"mod/{dir_name}\"\n"),
            );
        }

        let (tx, rx) = mpsc::channel();
        let backend = Arc::new(Backend::with_emitter(
            Roots::under(&base),
            true,
            Arc::new(move |ev| {
                let _ = tx.send(ev);
            }),
        ));
        (base, backend, rx)
    }

    fn at_of(v: &GameView) -> At {
        At {
            index: v.state.active,
            name: v.state.playsets[v.state.active].name.clone(),
        }
    }

    #[test]
    fn wallpaper_commands_and_cached_preview_fallback() {
        let (base, backend, _) = setup("art-cmds");
        let app_dir = playset::app_dir(&Roots::under(&base));
        put(
            &base.join("docs/Paradox Interactive/Stellaris/mod/a.mod"),
            "name=\"Alpha\"\npath=\"mod/a\"\nremote_file_id=\"555\"\n",
        );
        let v = backend.load("stellaris", false).unwrap();
        assert!(!v.art.logo && !v.art.background && !v.art.wallpaper && !v.art.custom);
        let has_thumb = |v: &GameView| {
            v.mods
                .iter()
                .find(|m| m.id == "mod/a.mod")
                .unwrap()
                .has_thumb
        };
        assert!(!has_thumb(&v));
        assert_eq!(backend.read_thumb("stellaris", "mod", "mod/a.mod"), None);
        // Appears after the scan: load and read must both see it without a rescan.
        put(&app_dir.join("mod-thumbs/stellaris/555.png"), "preview");
        assert!(has_thumb(&backend.load("stellaris", false).unwrap()));
        let (_, bytes) = backend.read_thumb("stellaris", "mod", "mod/a.mod").unwrap();
        assert_eq!(bytes, b"preview");

        let picked = base.join("picked.anything");
        std::fs::write(&picked, b"\xff\xd8\xffjpeg").unwrap();
        let art = backend.wallpaper_set_file("stellaris", &picked).unwrap();
        assert!(art.wallpaper && art.custom && !art.background);
        let (path, bytes) = backend.read_thumb("stellaris", "art", "wallpaper").unwrap();
        assert_eq!(path, app_dir.join("wallpapers/stellaris.custom.jpg"));
        assert_eq!(bytes, b"\xff\xd8\xffjpeg");

        let err = backend
            .wallpaper_set_url("stellaris", "http://example.com/a.png")
            .unwrap_err();
        assert_eq!(err.message, "Only https links are supported");
        assert!(backend.wallpaper_set_file("../x", &picked).is_err());

        let art = backend.wallpaper_reset("stellaris").unwrap();
        assert!(!art.wallpaper && !art.custom);
    }

    #[test]
    fn read_commands_leave_corrupt_playsets_in_place() {
        let (base, backend, _) = setup("corrupt-read");
        let sp = playset::store_path(&Roots::under(&base));
        put(&sp, "{ corrupt json");

        let g = backend.games().unwrap();
        assert!(g.warning.is_some());
        assert_eq!(g.games.len(), 1);
        assert_eq!(
            g.games[0].version.as_deref(),
            Some("Corona Borealis v4.6.0 (3d1a)")
        );

        let v = backend.load("stellaris", false).unwrap();
        assert!(v.read_only && v.warning.is_some());
        let _ = backend.summary("stellaris").unwrap();
        let _ = backend.saves_list("stellaris").unwrap();
        let _ = backend.settings_get("stellaris").unwrap();
        let _ = backend.crash_list("stellaris").unwrap();

        assert_eq!(std::fs::read_to_string(&sp).unwrap(), "{ corrupt json");
        let entries: Vec<_> = std::fs::read_dir(sp.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!entries.iter().any(|n| n.contains(".bad-")));
    }

    #[test]
    fn stale_at_refused_and_writes_nothing() {
        let (base, backend, _) = setup("stale-at");
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);
        backend
            .set_mods_enabled("stellaris", &at, &["mod/a.mod".into()], true)
            .unwrap();
        let sp = playset::store_path(&Roots::under(&base));
        let before = std::fs::read_to_string(&sp).unwrap();

        let stale = At {
            index: 0,
            name: "WrongName".into(),
        };
        let err = backend
            .set_mods_enabled("stellaris", &stale, &["mod/b.mod".into()], true)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Stale);
        assert_eq!(
            backend
                .move_mod("stellaris", &stale, "mod/b.mod", Some("mod/a.mod"))
                .unwrap_err()
                .kind,
            ErrorKind::Stale
        );
        assert_eq!(
            backend
                .playset_use("stellaris", 0, "WrongName")
                .unwrap_err()
                .kind,
            ErrorKind::Stale
        );
        assert_eq!(std::fs::read_to_string(&sp).unwrap(), before);
    }

    #[test]
    fn move_mod_before_and_append_with_hidden_entries() {
        let (base, backend, _) = setup("move-mod");
        // Seed store with a hidden entry between a and b.
        let sp = playset::store_path(&Roots::under(&base));
        put(
            &sp,
            r#"{"games":{"stellaris":{"active":0,"playsets":[{"name":"Default","mods":[
                {"id":"mod/a.mod","enabled":true},
                {"id":"mod/hidden.mod","enabled":false},
                {"id":"mod/b.mod","enabled":true},
                {"id":"mod/c.mod","enabled":true}
            ],"disabled_dlcs":[]}],"launch_args":""}}}"#,
        );
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);

        fn ids(st: &GameStateDto) -> Vec<&str> {
            st.playsets[0].mods.iter().map(|e| e.id.as_str()).collect()
        }
        // Move c before a
        let s = backend
            .move_mod("stellaris", &at, "mod/c.mod", Some("mod/a.mod"))
            .unwrap();
        assert_eq!(
            ids(&s),
            ["mod/c.mod", "mod/a.mod", "mod/hidden.mod", "mod/b.mod"]
        );

        // Move c before b (moving down past hidden entry)
        let s = backend
            .move_mod("stellaris", &at, "mod/c.mod", Some("mod/b.mod"))
            .unwrap();
        assert_eq!(
            ids(&s),
            ["mod/a.mod", "mod/hidden.mod", "mod/c.mod", "mod/b.mod"]
        );

        // Append a (before_id = None)
        let s = backend
            .move_mod("stellaris", &at, "mod/a.mod", None)
            .unwrap();
        assert_eq!(
            ids(&s),
            ["mod/hidden.mod", "mod/c.mod", "mod/b.mod", "mod/a.mod"]
        );
    }

    #[test]
    fn settings_apply_bad_edit_crlf_and_rollback_by_id() {
        let (base, backend, _) = setup("settings");
        let data = base.join("docs/Paradox Interactive/Stellaris");
        let game_path = data.join("settings.txt");
        let pdx_path = data.join("pdx_settings.txt");
        let crlf_game = SETTINGS_GAME.replace('\n', "\r\n");
        let crlf_pdx = SETTINGS_PDX.replace('\n', "\r\n");
        put(&game_path, &crlf_game);
        put(&pdx_path, &crlf_pdx);

        let view = backend.settings_get("stellaris").unwrap();
        assert!(view.raw.game.as_deref().unwrap().contains('\n'));
        assert!(!view.raw.game.as_deref().unwrap().contains("\r\n"));

        // One valid edit + one bad edit -> error, nothing written, no backups created
        let err = backend
            .settings_apply(
                "stellaris",
                &view.rev,
                &[
                    FieldEdit {
                        field: "vsync".into(),
                        value: "no".into(),
                    },
                    FieldEdit {
                        field: "master-volume".into(),
                        value: "999".into(),
                    },
                ],
                &[],
            )
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        assert_eq!(std::fs::read_to_string(&game_path).unwrap(), crlf_game);
        assert_eq!(std::fs::read_to_string(&pdx_path).unwrap(), crlf_pdx);
        assert!(
            backend
                .settings_get("stellaris")
                .unwrap()
                .backups
                .is_empty()
        );

        // Raw LF edit + form edit preserves CRLF on disk
        let edited_raw = view
            .raw
            .game
            .as_deref()
            .unwrap()
            .replace("autosave=4", "autosave=2");
        let applied = backend
            .settings_apply(
                "stellaris",
                &view.rev,
                &[FieldEdit {
                    field: "vsync".into(),
                    value: "no".into(),
                }],
                &[RawEdit {
                    file: SettingsFileKey::Game,
                    text: edited_raw,
                }],
            )
            .unwrap();
        assert!(applied.backup_dir.is_some());
        let disk = std::fs::read_to_string(&game_path).unwrap();
        assert!(disk.contains("autosave=2\r\n"));
        assert!(disk.contains("vsync=no\r\n"));
        assert!(!disk.replace("\r\n", "").contains('\n'));

        // Stale rev refused
        assert_eq!(
            backend
                .settings_apply("stellaris", &view.rev, &[], &[])
                .unwrap_err()
                .kind,
            ErrorKind::Stale
        );

        // Rollback by backup id restores original; unknown id is Stale
        let backup_id = applied.view.backups[0].id;
        let restored = backend.settings_rollback("stellaris", backup_id).unwrap();
        assert_eq!(std::fs::read_to_string(&game_path).unwrap(), crlf_game);
        assert!(restored.backups.len() >= 2);
        assert_eq!(
            backend.settings_rollback("stellaris", 42).unwrap_err().kind,
            ErrorKind::Stale
        );
    }

    #[test]
    fn saves_play_refusal_writes_nothing() {
        let (base, backend, _) = setup("saves-refuse");
        let roots = Roots::under(&base);
        let inst = game::discover(&roots).remove(0);
        put_save(
            &inst,
            "emp/old.sav",
            &sav("Old Empire", "2200.01.01"),
            SystemTime::now() + Duration::from_secs(5),
        );

        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);
        let sp = playset::store_path(&roots);
        let saves_json = playset::app_dir(&roots).join("saves.json");
        let dlc_load_path = inst.data_dir.join("dlc_load.json");
        assert!(!sp.exists() && !saves_json.exists() && !dlc_load_path.exists());

        // Unknown save without force -> NeedsConfirm, writes nothing
        let out = backend
            .saves_play("stellaris", &at, "emp/old.sav", false, None)
            .unwrap();
        assert!(matches!(out, SavePlayOutcome::NeedsConfirm { .. }));
        assert!(!sp.exists() && !saves_json.exists() && !dlc_load_path.exists());

        // Save with missing mod -> NeedsConfirm naming missing mod, writes nothing on refusal
        saves::sweep(&inst, &roots).unwrap();
        dl(&inst, &["mod/a.mod", "mod/gone.mod"], &[]);
        put_save(
            &inst,
            "emp/miss.sav",
            &sav("Missing", "2210.01.01"),
            SystemTime::now() + Duration::from_secs(30),
        );
        saves::sweep(&inst, &roots).unwrap();
        let before_saves = std::fs::read_to_string(&saves_json).unwrap();
        let before_dl = std::fs::read_to_string(&dlc_load_path).unwrap();
        let out = backend
            .saves_play("stellaris", &at, "emp/miss.sav", false, None)
            .unwrap();
        match out {
            SavePlayOutcome::NeedsConfirm { missing, .. } => {
                assert_eq!(missing, ["mod/gone.mod"]);
            }
            _ => panic!("expected NeedsConfirm"),
        }
        assert!(!sp.exists());
        assert_eq!(std::fs::read_to_string(&saves_json).unwrap(), before_saves);
        assert_eq!(std::fs::read_to_string(&dlc_load_path).unwrap(), before_dl);
    }

    #[cfg(unix)]
    #[test]
    fn launch_refuses_read_only_and_watches_exit() {
        let (base, backend, rx) = setup("launch-ro");
        let sp = playset::store_path(&Roots::under(&base));
        put(&sp, "{ not valid json");
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);
        let err = backend.launch("stellaris", &at, &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ReadOnly);
        assert!(
            !base
                .join("docs/Paradox Interactive/Stellaris/dlc_load.json")
                .exists()
        );

        // Fix store and launch /usr/bin/true
        std::fs::remove_file(&sp).unwrap();
        let info = backend
            .launch("stellaris", &at, &["-debug_mode".into()])
            .unwrap();
        assert!(info.pid > 0);
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ev.why, "exit");
        assert_eq!(ev.game.as_deref(), Some("stellaris"));
        assert_eq!(backend.running_count("stellaris"), 0);
    }

    #[test]
    fn share_export_contains_no_path() {
        let (base, backend, _) = setup("share-priv");
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);
        backend
            .set_mods_enabled("stellaris", &at, &["mod/a.mod".into()], true)
            .unwrap();
        let exp = backend.share_export("stellaris", &at).unwrap();
        assert!(exp.code.starts_with("regent1:"));
        let base_str = base.to_string_lossy();
        assert!(!exp.json.contains(base_str.as_ref()));
        assert!(!exp.json.contains("mod/a.mod"));
        assert!(!exp.code.contains(base_str.as_ref()));
    }

    #[test]
    fn summary_suggest_changed_false_on_pure_cycle_and_conflict_files() {
        let (base, backend, _) = setup("cycle");
        let data = base.join("docs/Paradox Interactive/Stellaris/mod");
        put(
            &data.join("a.mod"),
            "name=\"Alpha\"\npath=\"mod/a\"\ndependencies={ \"Beta\" }\n",
        );
        put(
            &data.join("b.mod"),
            "name=\"Beta\"\npath=\"mod/b\"\ndependencies={ \"Alpha\" }\n",
        );
        let v = backend.load("stellaris", true).unwrap();
        let at = at_of(&v);
        backend
            .set_mods_enabled(
                "stellaris",
                &at,
                &["mod/a.mod".into(), "mod/b.mod".into()],
                true,
            )
            .unwrap();

        let sum = backend.summary("stellaris").unwrap();
        assert!(sum.has_manifest);
        assert!(
            !sum.suggest.reasons.is_empty(),
            "cycle produces a kept-order reason"
        );
        assert!(
            !sum.suggest.changed,
            "pure cycle leaves order unchanged so changed must be false"
        );
        assert_eq!(sum.pairs.len(), 1);
        assert_eq!(sum.pairs[0].files, 1);
        let files = backend
            .conflict_files("stellaris", &sum.pairs[0].winner, &sum.pairs[0].loser)
            .unwrap();
        assert_eq!(files, ["common/x.txt"]);
    }

    fn fs_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        fn walk(
            base: &Path,
            dir: &Path,
            out: &mut std::collections::BTreeMap<PathBuf, (Vec<u8>, SystemTime)>,
        ) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(meta) = std::fs::symlink_metadata(&p) else {
                    continue;
                };
                if meta.file_type().is_dir() {
                    walk(base, &p, out);
                } else if meta.file_type().is_file() {
                    let rel = p.strip_prefix(base).unwrap().to_path_buf();
                    let bytes = std::fs::read(&p).unwrap();
                    let mtime = meta.modified().unwrap();
                    out.insert(rel, (bytes, mtime));
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn r1_r3_reads_and_corrupt_refusals_leave_bytes_and_mtimes_unchanged() {
        let (base, backend, _) = setup("fs-snap-r1");
        let roots = Roots::under(&base);
        let inst = game::discover(&roots).remove(0);
        let sp = playset::store_path(&roots);
        let saves_json = playset::app_dir(&roots).join("saves.json");
        let dlc_load_path = inst.data_dir.join("dlc_load.json");
        put(&sp, "{ corrupt playsets");
        put(&saves_json, "{ corrupt saves");
        put(&dlc_load_path, "{ corrupt dlc_load");
        put_save(
            &inst,
            "emp/a.sav",
            &sav("Alpha", "2200.01.01"),
            SystemTime::now(),
        );

        let before = fs_snapshot(&base);

        // Cold boot, load (rescan false & true), summary, saves_list, settings_get, crash_list
        let g = backend.games().unwrap();
        assert!(g.warning.is_some());
        let v1 = backend.load("stellaris", false).unwrap();
        assert!(v1.read_only && v1.warning.is_some());
        let v2 = backend.load("stellaris", true).unwrap();
        assert!(v2.read_only && v2.warning.is_some());
        let _ = backend.summary("stellaris").unwrap();
        let sl = backend.saves_list("stellaris").unwrap();
        assert!(sl.warning.is_some());
        let _ = backend.settings_get("stellaris").unwrap();
        let _ = backend.crash_list("stellaris").unwrap();

        // Ordinary mutating commands fail closed on corrupt playsets.json (never quarantine).
        let at = at_of(&v1);
        assert_eq!(
            backend
                .set_mods_enabled("stellaris", &at, &["mod/a.mod".into()], true)
                .unwrap_err()
                .kind,
            ErrorKind::ReadOnly
        );
        assert_eq!(
            backend.playset_new("stellaris", "New").unwrap_err().kind,
            ErrorKind::ReadOnly
        );
        assert_eq!(
            backend
                .share_import("stellaris", r#"{"game":"stellaris","name":"X","mods":[]}"#)
                .unwrap_err()
                .kind,
            ErrorKind::ReadOnly
        );
        assert_eq!(
            backend.launch("stellaris", &at, &[]).unwrap_err().kind,
            ErrorKind::ReadOnly
        );
        assert_eq!(
            backend
                .saves_play("stellaris", &at, "emp/a.sav", true, None)
                .unwrap_err()
                .kind,
            ErrorKind::ReadOnly
        );

        // Remove corrupt playsets.json, keep corrupt saves.json & dlc_load.json:
        // corrupt book refusal on launch / saves_play / saves_bind must also leave bytes & mtimes unchanged.
        assert_eq!(fs_snapshot(&base), before);
        std::fs::remove_file(&sp).unwrap();
        let before2 = fs_snapshot(&base);
        let v3 = backend.load("stellaris", true).unwrap();
        let at3 = at_of(&v3);
        let sl3 = backend.saves_list("stellaris").unwrap();
        assert!(
            sl3.warning
                .as_deref()
                .is_some_and(|w| w.contains("corrupt"))
        );
        assert!(backend.launch("stellaris", &at3, &[]).is_err());
        assert!(backend.saves_bind("stellaris", &at3, "emp/a.sav").is_err());
        assert!(
            backend
                .saves_play("stellaris", &at3, "emp/a.sav", true, None)
                .is_err()
        );
        assert_eq!(fs_snapshot(&base), before2);
    }

    #[cfg(unix)]
    #[test]
    fn r2_launch_refuses_corrupt_seeded_dlc_load_and_saves_play_restores_on_spawn_fail() {
        use std::os::unix::fs::PermissionsExt;
        let (base, backend, _) = setup("r2-launch");
        let roots = Roots::under(&base);
        let inst = game::discover(&roots).remove(0);
        let dl_path = inst.data_dir.join("dlc_load.json");
        put(&dl_path, "{ broken dlc_load");

        // Launch refuses when playset was seeded from corrupt dlc_load.json.
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);
        let err = backend.launch("stellaris", &at, &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        assert!(err.message.contains("dlc_load.json"), "{}", err.message);
        assert_eq!(
            std::fs::read_to_string(&dl_path).unwrap(),
            "{ broken dlc_load"
        );
        assert!(!dl_path.with_extension("json.regent-bak").exists());

        // Fix dlc_load, bind a save to Default, switch to Other, then make exe fail at spawn time.
        put(
            &dl_path,
            r#"{"enabled_mods":["mod/a.mod"],"disabled_dlcs":[]}"#,
        );
        let v = backend.load("stellaris", true).unwrap();
        let at = at_of(&v);
        backend
            .set_mods_enabled("stellaris", &at, &["mod/a.mod".into()], true)
            .unwrap();
        put_save(
            &inst,
            "emp/s1.sav",
            &sav("Emp", "2201.01.01"),
            SystemTime::now() + Duration::from_secs(10),
        );
        backend.saves_bind("stellaris", &at, "emp/s1.sav").unwrap();
        backend.playset_new("stellaris", "Other").unwrap();
        let sp = playset::store_path(&roots);
        let store_before = std::fs::read_to_string(&sp).unwrap();
        let dl_before = std::fs::read_to_string(&dl_path).unwrap();

        // Point launcher-settings.json exePath to an executable with a nonexistent shebang so spawn fails.
        let bad_exe = base.join("bad_exe.sh");
        put(&bad_exe, "#!/nonexistent/regent_interp_xyz\n");
        std::fs::set_permissions(&bad_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        put(
            &inst.dir.join("launcher-settings.json"),
            &format!(
                r#"{{"gameId":"stellaris","rawVersion":"v4.6.0","gameDataPath":"~/Documents/Paradox Interactive/Stellaris","exePath":"{}"}}"#,
                bad_exe.display()
            ),
        );
        backend.games().unwrap();
        let v_other = backend.load("stellaris", true).unwrap();
        let at_other = at_of(&v_other);
        assert_eq!(at_other.name, "Other");
        let err = backend
            .saves_play("stellaris", &at_other, "emp/s1.sav", false, None)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        // Active playset in playsets.json and dlc_load.json must both be untouched/restored!
        assert_eq!(std::fs::read_to_string(&sp).unwrap(), store_before);
        assert_eq!(std::fs::read_to_string(&dl_path).unwrap(), dl_before);
    }

    #[cfg(unix)]
    #[test]
    fn r5_saves_play_expect_rejects_changed_plan() {
        let (base, backend, rx) = setup("r5-expect");
        let roots = Roots::under(&base);
        let inst = game::discover(&roots).remove(0);
        saves::sweep(&inst, &roots).unwrap();
        dl(&inst, &["mod/a.mod", "mod/gone1.mod"], &[]);
        put_save(
            &inst,
            "emp/m.sav",
            &sav("M", "2210.01.01"),
            SystemTime::now() + Duration::from_secs(20),
        );
        saves::sweep(&inst, &roots).unwrap();
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);

        // First ask: NeedsConfirm carries the plan token.
        let tok = match backend
            .saves_play("stellaris", &at, "emp/m.sav", false, None)
            .unwrap()
        {
            SavePlayOutcome::NeedsConfirm { missing, token, .. } => {
                assert_eq!(missing, ["mod/gone1.mod"]);
                token
            }
            _ => panic!("expected NeedsConfirm"),
        };

        // A token from some other plan returns NeedsConfirm with the current token, no launch.
        let out = backend
            .saves_play("stellaris", &at, "emp/m.sav", true, Some("bogus"))
            .unwrap();
        match out {
            SavePlayOutcome::NeedsConfirm { token, .. } => assert_eq!(token, tok),
            _ => panic!("expected NeedsConfirm when expect mismatches"),
        }

        // Passing the matching token launches.
        let out = backend
            .saves_play("stellaris", &at, "emp/m.sav", true, Some(&tok))
            .unwrap();
        assert!(matches!(out, SavePlayOutcome::Launched { .. }));
        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // Now a playset matches (Use), so passing expect="unknown" returns Stale.
        let v2 = backend.load("stellaris", false).unwrap();
        let at2 = at_of(&v2);
        // Bind emp/m.sav to current playset so plan becomes Use.
        backend.saves_bind("stellaris", &at2, "emp/m.sav").unwrap();
        let err = backend
            .saves_play("stellaris", &at2, "emp/m.sav", true, Some(&tok))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Stale);
    }

    #[cfg(unix)]
    #[test]
    fn r3_5_token_distinguishes_same_missing_list_with_different_mods() {
        let (base, backend, _rx) = setup("r3-5-token");
        let roots = Roots::under(&base);
        let inst = game::discover(&roots).remove(0);
        saves::sweep(&inst, &roots).unwrap();
        dl(&inst, &["mod/a.mod", "mod/gone1.mod"], &[]);
        put_save(
            &inst,
            "emp/m.sav",
            &sav("M", "2210.01.01"),
            SystemTime::now() + Duration::from_secs(20),
        );
        saves::sweep(&inst, &roots).unwrap();
        let at = at_of(&backend.load("stellaris", false).unwrap());
        let SavePlayOutcome::NeedsConfirm { token: t1, .. } = backend
            .saves_play("stellaris", &at, "emp/m.sav", false, None)
            .unwrap()
        else {
            panic!("expected NeedsConfirm");
        };
        // The save is rewritten with a different enabled set but the same missing list.
        dl(&inst, &["mod/b.mod", "mod/gone1.mod"], &[]);
        put_save(
            &inst,
            "emp/m.sav",
            &sav("M", "2210.02.01"),
            SystemTime::now() + Duration::from_secs(40),
        );
        saves::sweep(&inst, &roots).unwrap();
        let dl_before = std::fs::read_to_string(inst.data_dir.join("dlc_load.json")).unwrap();
        let SavePlayOutcome::NeedsConfirm {
            token: t2, missing, ..
        } = backend
            .saves_play("stellaris", &at, "emp/m.sav", true, Some(&t1))
            .unwrap()
        else {
            panic!("a token for a different plan must not launch");
        };
        assert_eq!(missing, ["mod/gone1.mod"]);
        assert_ne!(t1, t2);
        assert_eq!(
            std::fs::read_to_string(inst.data_dir.join("dlc_load.json")).unwrap(),
            dl_before
        );
    }

    #[test]
    fn r3_9_stale_at_is_refused_before_the_lockfile_is_created() {
        let (base, backend, _rx) = setup("r3-9-stale-lock");
        let lock = playset::app_dir(&Roots::under(&base)).join(".lock");
        let _ = backend.load("stellaris", false).unwrap();
        let stale = At {
            index: 0,
            name: "No such playset".into(),
        };
        let kind = |r: AppResult<()>| r.unwrap_err().kind;
        assert_eq!(
            kind(
                backend
                    .set_mods_enabled("stellaris", &stale, &["mod/a.mod".into()], true)
                    .map(|_| ())
            ),
            ErrorKind::Stale
        );
        assert_eq!(
            kind(backend.launch("stellaris", &stale, &[]).map(|_| ())),
            ErrorKind::Stale
        );
        assert_eq!(
            kind(
                backend
                    .saves_bind("stellaris", &stale, "emp/a.sav")
                    .map(|_| ())
            ),
            ErrorKind::Stale
        );
        assert!(!lock.exists(), "a stale call must not create .lock");
    }

    #[test]
    fn r8_r15_enum_dto_json_fields_are_camel_case() {
        let outcome = SavePlayOutcome::Launched {
            state: GameStateDto {
                active: 0,
                playsets: vec![],
                launch_args: String::new(),
            },
            using: "Default".into(),
            manual_load: true,
            pid: 42,
            warning: None,
        };
        let v = serde_json::to_value(&outcome).unwrap();
        assert_eq!(v["kind"], "launched");
        assert_eq!(v["manualLoad"], true);
        assert!(v.get("manual_load").is_none());

        let nc = serde_json::to_value(SavePlayOutcome::NeedsConfirm {
            reason: "r".into(),
            missing: vec!["m".into()],
            token: "t".into(),
        })
        .unwrap();
        assert_eq!(nc["token"], "t");
        assert_eq!(nc["kind"], "needsConfirm");
        assert_eq!(nc["reason"], "r");
        assert_eq!(nc["missing"][0], "m");

        let fk = serde_json::to_value(FieldKind::Slider {
            min: 0.0,
            max: 100.0,
            step: 1.0,
        })
        .unwrap();
        assert_eq!(fk["kind"], "slider");
        assert_eq!(fk["max"], 100.0);

        let sb = serde_json::to_value(SaveBinding::Playset { name: "A".into() }).unwrap();
        assert_eq!(sb["kind"], "playset");
        assert_eq!(sb["name"], "A");
        let sc = serde_json::to_value(SaveBinding::Custom { count: 3 }).unwrap();
        assert_eq!(sc["kind"], "custom");
        assert_eq!(sc["count"], 3);

        let sp = serde_json::to_value(SavePlan::Make {
            playset: "For s".into(),
            missing: vec!["x".into()],
        })
        .unwrap();
        assert_eq!(sp["kind"], "make");
        assert_eq!(sp["playset"], "For s");

        // R15: OpenTarget::CrashDir accepts omitted, null, or string name.
        for j in [
            r#"{"kind":"crashDir","game":"stellaris"}"#,
            r#"{"kind":"crashDir","game":"stellaris","name":null}"#,
            r#"{"kind":"crashDir","game":"stellaris","name":"c1"}"#,
        ] {
            let t: OpenTarget = serde_json::from_str(j).unwrap();
            assert!(matches!(t, OpenTarget::CrashDir { .. }), "{j}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn r9_launch_gating_rejects_second_concurrent_launch_and_saves_play() {
        use std::os::unix::fs::PermissionsExt;
        let (base, backend, rx) = setup("r9-gate");
        let roots = Roots::under(&base);
        let inst = game::discover(&roots).remove(0);
        let sleeper = base.join("sleep_game.sh");
        put(&sleeper, "#!/bin/sh\nsleep 0.35\n");
        std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o755)).unwrap();
        put(
            &inst.dir.join("launcher-settings.json"),
            &format!(
                r#"{{"gameId":"stellaris","rawVersion":"v4.6.0","gameDataPath":"~/Documents/Paradox Interactive/Stellaris","exePath":"{}"}}"#,
                sleeper.display()
            ),
        );
        put_save(
            &inst,
            "emp/s.sav",
            &sav("S", "2200.01.01"),
            SystemTime::now(),
        );
        backend.games().unwrap();
        let v = backend.load("stellaris", true).unwrap();
        let at = at_of(&v);

        let info = backend.launch("stellaris", &at, &[]).unwrap();
        assert!(info.pid > 0);
        // Second launch while first is running is refused.
        let err = backend.launch("stellaris", &at, &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        assert!(err.message.contains("already"), "{}", err.message);
        // saves_play for same game while running is also refused.
        let err2 = backend
            .saves_play("stellaris", &at, "emp/s.sav", true, None)
            .unwrap_err();
        assert_eq!(err2.kind, ErrorKind::Failed);

        // After exit event, launching succeeds again.
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ev.why, "exit");
        assert_eq!(backend.running_count("stellaris"), 0);
        let info2 = backend.launch("stellaris", &at, &[]).unwrap();
        assert!(info2.pid > 0);
        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn r10_rescan_invalidates_computed_index_and_generation_guard_prevents_stale_publish() {
        let (base, backend, _) = setup("r10-rescan");
        let v = backend.load("stellaris", false).unwrap();
        let at = at_of(&v);
        backend
            .set_mods_enabled(
                "stellaris",
                &at,
                &["mod/a.mod".into(), "mod/b.mod".into()],
                true,
            )
            .unwrap();
        let s1 = backend.summary("stellaris").unwrap();
        assert_eq!(s1.pairs.len(), 1);
        assert_eq!(s1.pairs[0].files, 1);

        // Add a second conflicting file inside mod/a and mod/b (without changing mod/ dir mtime).
        let data = base.join("docs/Paradox Interactive/Stellaris/mod");
        put(&data.join("a/common/y.txt"), "1");
        put(&data.join("b/common/y.txt"), "2");

        // Explicit Refresh (rescan = true) must invalidate cached index/manifest/sizes.
        backend.load("stellaris", true).unwrap();
        let s2 = backend.summary("stellaris").unwrap();
        assert_eq!(s2.pairs[0].files, 2, "rescan=true must recompute index");

        // Generation guard: an older scan cannot overwrite a newer published scan.
        let (install, _) = backend.install_for("stellaris").unwrap();
        let g1 = backend.bump_generation(install.def.id);
        let scan1 = build_scan(install.clone(), None, true);
        let g2 = backend.bump_generation(install.def.id);
        let scan2 = build_scan(install, None, true);
        assert!(backend.publish_scan(Arc::clone(&scan2), g2));
        assert!(
            !backend.publish_scan(Arc::clone(&scan1), g1),
            "stale generation must not publish"
        );
        let current = backend.cached_scan("stellaris").unwrap();
        assert!(Arc::ptr_eq(&current, &scan2));
    }

    #[test]
    fn r4_fixture_scripts_refuse_unsafe_targets_and_stage_valid_regeneration() {
        use std::process::Command;
        let repo_scripts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts");
        let tmp = tmp_dir("r4-fixture");
        let tmp_scripts = tmp.join("scripts");
        std::fs::create_dir_all(&tmp_scripts).unwrap();
        std::fs::copy(
            repo_scripts.join("make_fixture.py"),
            tmp_scripts.join("make_fixture.py"),
        )
        .unwrap();
        std::fs::copy(
            repo_scripts.join("fixture.sh"),
            tmp_scripts.join("fixture.sh"),
        )
        .unwrap();
        let py = tmp_scripts.join("make_fixture.py");

        // 1. Path outside repo is refused and untouched.
        let outside = tmp_dir("r4-outside");
        put(&outside.join("keep.txt"), "important");
        let out = Command::new("python3")
            .arg(&py)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(!out.status.success(), "must refuse target outside repo");
        assert_eq!(
            std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "important"
        );

        // 2. Unmarked existing dir inside repo (no .regent-fixture) is refused and untouched.
        let unmarked = tmp.join("unmarked_dir");
        put(&unmarked.join("keep.txt"), "user data");
        let out = Command::new("python3")
            .arg(&py)
            .arg(&unmarked)
            .output()
            .unwrap();
        assert!(!out.status.success(), "must refuse unmarked dir");
        assert_eq!(
            std::fs::read_to_string(unmarked.join("keep.txt")).unwrap(),
            "user data"
        );

        // 3. Symlinked target inside repo is refused and target untouched.
        #[cfg(unix)]
        {
            let link = tmp.join("link_fix");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            let out = Command::new("python3")
                .arg(&py)
                .arg(&link)
                .output()
                .unwrap();
            assert!(!out.status.success(), "must refuse symlink target");
            assert_eq!(
                std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
                "important"
            );
            let sh_out = Command::new("bash")
                .arg(tmp_scripts.join("fixture.sh"))
                .arg(&link)
                .output()
                .unwrap();
            assert!(!sh_out.status.success(), "fixture.sh must refuse symlink");
        }

        // 4. Valid fresh creation via fixture.sh writes sentinel + .ready, and staged regeneration succeeds.
        let valid = tmp.join(".fixture");
        let sh_out = Command::new("bash")
            .arg(tmp_scripts.join("fixture.sh"))
            .arg(&valid)
            .output()
            .unwrap();
        assert!(
            sh_out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&sh_out.stderr)
        );
        assert!(valid.join(".regent-fixture").is_file());
        assert!(valid.join(".ready").is_file());

        // Reset .ready and regenerate via staged swap.
        std::fs::remove_file(valid.join(".ready")).unwrap();
        put(&valid.join("stale_file.txt"), "old");
        let sh_out2 = Command::new("bash")
            .arg(tmp_scripts.join("fixture.sh"))
            .arg(&valid)
            .output()
            .unwrap();
        assert!(sh_out2.status.success());
        assert!(!valid.join("stale_file.txt").exists());
        assert!(valid.join(".regent-fixture").is_file());
        assert!(valid.join(".ready").is_file());
    }
}
