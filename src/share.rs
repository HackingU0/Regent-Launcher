//! Share a playset: `regent1:` code / JSON export, import (own, official launcher, Irony),
//! and a diff against the local playset. UI-free. No paths or local ids ever leave the machine.
use crate::index::{FileIndex, Manifest};
use crate::mods::{Dlc, Mod};
use crate::playset::{Entry, GameState, Playset};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

const PREFIX: &str = "regent1:";
/// Inflate cap for codes (and the biggest JSON/zip entry we read).
const LIMIT: usize = 1 << 20;
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn yes() -> bool {
    true
}
fn is_on(b: &bool) -> bool {
    *b
}
fn is_off(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PMod {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Hash of the checksum-relevant files (see `mod_hash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Only foreign formats carry disabled mods; exports list enabled mods only.
    #[serde(default = "yes", skip_serializing_if = "is_on")]
    pub enabled: bool,
    /// The exporter could not fully index or read this mod: `hash` is absent for that reason,
    /// not because the mod is clean.
    #[serde(default, skip_serializing_if = "is_off")]
    pub incomplete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub regent: u32,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mods: Vec<PMod>,
    #[serde(default)]
    pub disabled_dlcs: Vec<String>,
    /// "Regent fingerprint" (not the game's checksum); empty for foreign formats.
    #[serde(default)]
    pub fingerprint: String,
    /// Set when any exported mod is `incomplete`.
    #[serde(default, skip_serializing_if = "is_off")]
    pub incomplete: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Missing {
    pub name: String,
    pub steam: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    /// Theirs, not in our enabled mods.
    pub missing: Vec<String>,
    /// Ours, not in theirs.
    pub extra: Vec<String>,
    /// Shared mods load in a different order.
    pub order: bool,
    /// (mod, theirs, ours)
    pub version: Vec<(String, String, String)>,
    /// Same mod, different checksum-relevant files.
    pub content: Vec<String>,
    /// (theirs, ours)
    pub game_version: Option<(String, String)>,
    /// Additional differences or missing verification evidence (surfaced in `lines()` and `equal()`).
    #[serde(skip)]
    pub notes: Vec<String>,
}

impl Diff {
    pub fn equal(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && !self.order
            && self.version.is_empty()
            && self.content.is_empty()
            && self.game_version.is_none()
            && self.notes.is_empty()
    }

    /// One line per difference; a single "identical" line when equal.
    pub fn lines(&self) -> Vec<String> {
        let mut v: Vec<String> = vec![];
        if let Some((t, o)) = &self.game_version {
            v.push(format!("game version: theirs {t}, ours {o}"));
        }
        v.extend(self.missing.iter().map(|m| format!("missing here: {m}")));
        v.extend(self.extra.iter().map(|m| format!("extra here: {m}")));
        if self.order {
            v.push("load order differs".into());
        }
        v.extend(
            self.version
                .iter()
                .map(|(m, t, o)| format!("version: {m} theirs {t}, ours {o}")),
        );
        v.extend(self.content.iter().map(|m| format!("files differ: {m}")));
        v.extend(self.notes.iter().cloned());
        if v.is_empty() {
            v.push("identical mod list, order, versions and files".into());
        }
        v
    }
}

pub fn workshop_url(id: &str) -> String {
    format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}")
}

// ---------------------------------------------------------------- encoding

fn b64_enc(d: &[u8]) -> String {
    let mut s = String::new();
    for c in d.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | (*b as u32) << (16 - 8 * i));
        for i in 0..=c.len() {
            s.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

fn b64_dec(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (vec![], 0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=') {
        acc = acc << 6 | B64.iter().position(|b| *b == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub fn to_code(p: &Payload) -> String {
    let json = serde_json::to_vec(p).expect("payload serializes");
    format!(
        "{PREFIX}{}",
        b64_enc(&miniz_oxide::deflate::compress_to_vec(&json, 9))
    )
}

pub fn to_json(p: &Payload) -> String {
    serde_json::to_string_pretty(p).expect("payload serializes")
}

// ---------------------------------------------------------------- parsing

/// Detect and decode: regent code, our JSON, official launcher JSON, Irony zip or exported.json.
pub fn parse(bytes: &[u8]) -> Result<Payload> {
    if bytes.starts_with(b"PK\x03\x04") {
        let mut r = std::io::Cursor::new(bytes);
        let es = crate::zip::entries(&mut r)?;
        let Some(e) = es.iter().find(|e| {
            let n = e.name.to_lowercase();
            n.rsplit('/').next() == Some("exported.json")
        }) else {
            bail!("zip has no exported.json");
        };
        let raw = crate::zip::read(&mut r, e, LIMIT)?;
        let text = String::from_utf8_lossy(&raw);
        let text = text.trim_start_matches('\u{feff}').trim();
        return from_json(text.as_bytes());
    }
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}').trim();
    if let Some(code) = text.strip_prefix(PREFIX) {
        let packed = code.split_whitespace().collect::<String>();
        let Some(raw) = b64_dec(&packed) else {
            bail!("share code is damaged (bad characters)");
        };
        let Ok(json) = miniz_oxide::inflate::decompress_to_vec_with_limit(&raw, LIMIT) else {
            bail!("share code is damaged or larger than 1 MB");
        };
        return from_json(&json);
    }
    from_json(text.as_bytes())
}

fn from_json(json: &[u8]) -> Result<Payload> {
    let Ok(v) = serde_json::from_slice::<Value>(json) else {
        bail!("not a share code, playset JSON or zip");
    };
    if v.get("regent").is_some() {
        let p: Payload = serde_json::from_value(v)?;
        if p.regent != 1 {
            bail!("share format version {} is not supported", p.regent);
        }
        for (i, m) in p.mods.iter().enumerate() {
            if let Some(s) = &m.steam
                && !digits(s)
            {
                bail!(
                    "mod #{} ({}): '{s}' is not a valid Workshop id",
                    i + 1,
                    m.name
                );
            }
        }
        return Ok(p);
    }
    foreign(&v)
}

fn key<'a>(o: &'a serde_json::Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    o.iter()
        .find(|(k, _)| names.contains(&k.to_lowercase().as_str()))
        .map(|(_, v)| v)
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn looks_like_path(s: &str) -> bool {
    let t = s.trim();
    t.contains('/')
        || t.contains('\\')
        || t.starts_with('~')
        || (t.len() >= 2 && t.as_bytes()[0].is_ascii_alphabetic() && t.as_bytes()[1] == b':')
}

fn extract_ugc_id(s: &str) -> Option<String> {
    let norm = s.trim().replace('\\', "/");
    let file = norm.rsplit('/').next().unwrap_or(&norm);
    let low = file.to_ascii_lowercase();
    let mid = low.strip_prefix("ugc_")?.strip_suffix(".mod")?;
    digits(mid).then(|| mid.to_owned())
}

fn is_mod_descriptor_path(s: &str) -> bool {
    let low = s.trim().to_ascii_lowercase();
    low.ends_with(".mod") || looks_like_path(&low)
}

fn steam_from_mod_id(v: &Value) -> Option<String> {
    match v {
        Value::Object(o) => key(o, &["steamid", "remoteid", "remote_id", "remotefileid"])
            .and_then(text_of)
            .filter(|s| digits(s)),
        _ => text_of(v).filter(|s| digits(s)),
    }
}

/// The Workshop id an object entry states outright. Absent, null or an empty string is "none";
/// anything else that is not all digits is an error, never silently "none" (that would let the
/// entry match a different mod by name).
fn explicit_steam(o: &serde_json::Map<String, Value>, idx: usize) -> Result<Option<String>> {
    let Some(v) = key(o, &["steamid", "remoteid", "remote_id", "remotefileid"]) else {
        return Ok(None);
    };
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        _ => match text_of(v) {
            Some(s) if digits(&s) => Ok(Some(s)),
            _ => bail!("mod #{}: invalid Workshop id {v}", idx + 1),
        },
    }
}

/// Official launcher export or Irony (supports `Mods`, `ModNames`, `ModIds`, `"mod/ugc_N.mod"`,
/// workshop id strings, and launcher mod objects).
fn foreign(v: &Value) -> Result<Payload> {
    let (obj, list) = match v {
        Value::Array(a) => (None, Some(a)),
        Value::Object(o) => (Some(o), key(o, &["mods"]).and_then(Value::as_array)),
        _ => (None, None),
    };
    let mod_names = obj
        .and_then(|o| key(o, &["modnames"]))
        .and_then(Value::as_array);
    let mod_ids = obj
        .and_then(|o| key(o, &["modids"]))
        .and_then(Value::as_array);
    let count = list
        .map(Vec::len)
        .or_else(|| mod_names.map(Vec::len))
        .or_else(|| mod_ids.map(Vec::len));
    let Some(count) = count else {
        bail!("unrecognized playset format (no mods list)");
    };
    let mut mods: Vec<(Option<i64>, PMod)> = vec![];
    for idx in 0..count {
        let parallel_name = mod_names.and_then(|a| a.get(idx)).and_then(text_of);
        let parallel_steam = mod_ids.and_then(|a| a.get(idx)).and_then(steam_from_mod_id);
        let item = list.and_then(|a| a.get(idx));
        let (pos, name, steam, on) = match item {
            Some(Value::String(raw)) => {
                let s = raw.trim();
                let ugc_steam = if digits(s) {
                    Some(s.to_owned())
                } else {
                    extract_ugc_id(s)
                };
                let steam = parallel_steam.or(ugc_steam);
                let name = if let Some(pn) = parallel_name {
                    pn
                } else if is_mod_descriptor_path(s) {
                    if let Some(ref sid) = steam {
                        sid.clone()
                    } else {
                        // Local descriptor path without ModNames: strip path and `.mod` suffix.
                        let norm = s.replace('\\', "/");
                        let file = norm.rsplit('/').next().unwrap_or(&norm);
                        file.strip_suffix(".mod")
                            .or_else(|| file.strip_suffix(".MOD"))
                            .unwrap_or(file)
                            .to_owned()
                    }
                } else {
                    s.to_owned()
                };
                (None, name, steam, true)
            }
            Some(Value::Object(o)) => {
                let steam = explicit_steam(o, idx)?
                    .or_else(|| {
                        key(o, &["path", "filename", "descriptor"])
                            .and_then(text_of)
                            .and_then(|s| extract_ugc_id(&s))
                    })
                    .or(parallel_steam);
                let name = key(o, &["displayname", "name", "modname"])
                    .and_then(text_of)
                    .or(parallel_name)
                    .or_else(|| steam.clone())
                    .unwrap_or_default();
                let pos = key(o, &["position"])
                    .and_then(text_of)
                    .and_then(|s| s.parse().ok());
                let on = key(o, &["enabled"])
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                (pos, name, steam, on)
            }
            None => {
                let name = parallel_name
                    .or_else(|| parallel_steam.clone())
                    .unwrap_or_default();
                (None, name, parallel_steam, true)
            }
            _ => continue,
        };
        if name.is_empty() {
            continue;
        }
        mods.push((
            pos,
            PMod {
                name,
                steam,
                enabled: on,
                ..Default::default()
            },
        ));
    }
    mods.sort_by_key(|(p, _)| p.unwrap_or(i64::MAX)); // stable: unpositioned keep file order
    let s = |n: &[&str]| obj.and_then(|o| key(o, n)).and_then(text_of);
    Ok(Payload {
        regent: 0,
        game: s(&["game"]).unwrap_or_default().to_lowercase(),
        name: s(&["name", "playsetname"]).unwrap_or_else(|| "Imported playset".into()),
        mods: mods.into_iter().map(|(_, m)| m).collect(),
        ..Default::default()
    })
}

// ---------------------------------------------------------------- fingerprints

struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf29ce484222325)
    }
    fn w(&mut self, b: &[u8]) {
        for x in b {
            self.0 = (self.0 ^ *x as u64).wrapping_mul(0x100000001b3);
        }
    }
    fn sep(&mut self, b: &[u8]) {
        self.w(b);
        self.w(&[0]);
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

enum Src {
    Dir(std::path::PathBuf),
    Zip(std::io::BufReader<std::fs::File>, Vec<crate::zip::Entry>),
    None,
}

impl Src {
    fn open(m: &Mod) -> Src {
        match &m.root {
            Some(r) if r.is_dir() => Src::Dir(r.clone()),
            Some(r) => (|| {
                let mut f = std::io::BufReader::new(std::fs::File::open(r).ok()?);
                let es = crate::zip::entries(&mut f).ok()?;
                Some(Src::Zip(f, es))
            })()
            .unwrap_or(Src::None),
            None => Src::None,
        }
    }

    fn read(&mut self, rel: &str) -> Option<Vec<u8>> {
        match self {
            Src::Dir(d) => std::fs::read(d.join(rel)).ok(),
            Src::Zip(f, es) => {
                let e = es.iter().find(|e| e.name == rel)?;
                crate::zip::read(f, e, 1 << 28).ok()
            }
            Src::None => None,
        }
    }
}

/// FNV-1a 64 over the sorted checksum-relevant files (path + bytes) and sorted replace_paths.
/// `Ok(None)` when the mod touches nothing the manifest lists (or there is no manifest).
/// `Err(())` when the mod failed to index or a checksum-relevant file could not be read.
fn mod_hash_checked(
    m: &Mod,
    ix: &FileIndex,
    mf: Option<&Manifest>,
) -> std::result::Result<Option<String>, ()> {
    let Some(mf) = mf else {
        return Ok(None);
    };
    let Some(files) = ix.mods.get(&m.id) else {
        return Err(());
    };
    if !files.complete {
        return Err(());
    }
    let mut rel: Vec<&String> = files.paths.iter().filter(|p| mf.matches(p)).collect();
    if rel.is_empty() && !files.checksum {
        return Ok(None);
    }
    rel.sort();
    let mut src = Src::open(m);
    let mut h = Fnv::new();
    for p in rel {
        h.sep(p.as_bytes());
        let Some(b) = src.read(p) else {
            return Err(());
        };
        h.w(&(b.len() as u64).to_le_bytes());
        h.w(&b);
    }
    let mut rp: Vec<&String> = m.replace_paths.iter().collect();
    rp.sort();
    for r in rp {
        h.sep(b"replace_path");
        h.sep(r.as_bytes());
    }
    Ok(Some(h.hex()))
}

fn fingerprint(game_version: Option<&str>, mods: &[PMod]) -> String {
    let mut h = Fnv::new();
    h.sep(game_version.unwrap_or("").as_bytes());
    for m in mods.iter().filter(|m| m.enabled) {
        h.sep(m.steam.as_deref().unwrap_or(&m.name).as_bytes());
        h.sep(m.hash.as_deref().unwrap_or("").as_bytes());
    }
    h.hex()
}

// ---------------------------------------------------------------- export / verify / import

fn safe_mod_name(m: &Mod, steam: Option<&str>) -> String {
    let raw = m.name.trim();
    if raw.is_empty() || raw == m.id || looks_like_path(raw) || raw.eq_ignore_ascii_case(&m.id) {
        match steam {
            Some(id) => format!("Workshop mod {id}"),
            None => "Unnamed local mod".to_string(),
        }
    } else {
        raw.to_string()
    }
}

fn safe_opt(v: Option<&str>) -> Option<String> {
    let s = v?.trim();
    (!s.is_empty() && !looks_like_path(s)).then(|| s.to_owned())
}

fn filter_known_dlcs(ids: &[String], dlcs: &[Dlc]) -> Vec<String> {
    let known: HashSet<&str> = dlcs.iter().map(|d| d.id.as_str()).collect();
    let mut seen = HashSet::new();
    ids.iter()
        .filter(|id| known.contains(id.as_str()) && seen.insert(id.as_str()))
        .cloned()
        .collect()
}

struct LocalMod {
    pmod: PMod,
    /// True when the mod could not be indexed or a checksum-relevant file could not be read.
    unverifiable: bool,
}

/// Our enabled, installed mods in load order, in shareable form.
fn local_checked(
    p: &Playset,
    mods: &[Mod],
    ix: &FileIndex,
    mf: Option<&Manifest>,
) -> Vec<LocalMod> {
    p.visible(mods)
        .into_iter()
        .filter(|(e, _)| e.enabled)
        .map(|(_, m)| {
            let steam = m
                .remote_id
                .as_deref()
                .filter(|s| digits(s))
                .map(str::to_owned);
            let name = safe_mod_name(m, steam.as_deref());
            let (hash, unverifiable) = match mod_hash_checked(m, ix, mf) {
                Ok(h) => (h, false),
                Err(()) => (None, true),
            };
            LocalMod {
                pmod: PMod {
                    name,
                    steam,
                    version: safe_opt(m.version.as_deref()),
                    hash,
                    enabled: true,
                    incomplete: unverifiable,
                },
                unverifiable,
            }
        })
        .collect()
}

pub fn export(
    game: &str,
    game_version: Option<&str>,
    p: &Playset,
    mods: &[Mod],
    dlcs: &[Dlc],
    ix: &FileIndex,
    mf: Option<&Manifest>,
) -> Payload {
    let ms: Vec<PMod> = local_checked(p, mods, ix, mf)
        .into_iter()
        .map(|l| l.pmod)
        .collect();
    let safe_name = {
        let n = p.name.trim();
        if n.is_empty() || looks_like_path(n) {
            "Shared playset".to_string()
        } else {
            n.to_string()
        }
    };
    let safe_gv = safe_opt(game_version);
    let fp = fingerprint(safe_gv.as_deref(), &ms);
    let incomplete = ms.iter().any(|m| m.incomplete);
    Payload {
        regent: 1,
        game: game.into(),
        game_version: safe_gv,
        name: safe_name,
        fingerprint: fp,
        mods: ms,
        disabled_dlcs: filter_known_dlcs(&p.disabled_dlcs, dlcs),
        incomplete,
    }
}

/// When the payload specifies a Workshop id, require an exact Steam id match (no fallback to name).
/// Otherwise match the first unused candidate with the exact name.
fn locate(cands: &[(Option<&str>, &str)], taken: &[bool], p: &PMod) -> Option<usize> {
    let free = |i: &usize| !taken[*i];
    if let Some(s) = p.steam.as_deref() {
        return (0..cands.len())
            .filter(free)
            .find(|&i| cands[i].0 == Some(s));
    }
    (0..cands.len())
        .filter(free)
        .find(|&i| cands[i].1 == p.name)
}

/// Verify `t` against our active playset and return both the diff and our local fingerprint
/// (computing mod hashes once).
pub fn verify_with_fingerprint(
    t: &Payload,
    game_version: Option<&str>,
    p: &Playset,
    mods: &[Mod],
    dlcs: &[Dlc],
    ix: &FileIndex,
    mf: Option<&Manifest>,
) -> (Diff, String) {
    let ours_full = local_checked(p, mods, ix, mf);
    let ours_pmods: Vec<PMod> = ours_full.iter().map(|l| l.pmod.clone()).collect();
    let safe_gv = safe_opt(game_version);
    let ours_fp = fingerprint(safe_gv.as_deref(), &ours_pmods);
    let cands: Vec<_> = ours_full
        .iter()
        .map(|l| (l.pmod.steam.as_deref(), l.pmod.name.as_str()))
        .collect();
    let mut taken = vec![false; ours_full.len()];
    let mut d = Diff::default();
    let mut seq = vec![];
    let mut sender_incomplete = false;
    for m in t.mods.iter().filter(|m| m.enabled) {
        let Some(i) = locate(&cands, &taken, m) else {
            d.missing.push(m.name.clone());
            continue;
        };
        taken[i] = true;
        seq.push(i);
        let local_entry = &ours_full[i];
        let o = &local_entry.pmod;
        if let (Some(a), Some(b)) = (&m.version, &o.version)
            && a != b
        {
            d.version.push((m.name.clone(), a.clone(), b.clone()));
        }
        if local_entry.unverifiable {
            d.notes.push(format!(
                "cannot verify files: {} could not be indexed or read",
                m.name
            ));
        } else if t.regent == 1 && m.incomplete {
            sender_incomplete = true;
            d.notes.push(format!(
                "cannot verify files: the sender could not fully read {}",
                m.name
            ));
        } else if t.regent == 1 && mf.is_some() {
            if m.hash != o.hash {
                d.content.push(m.name.clone());
            }
        } else if let (Some(a), Some(b)) = (&m.hash, &o.hash)
            && a != b
        {
            d.content.push(m.name.clone());
        }
    }
    d.order = !seq.windows(2).all(|w| w[0] < w[1]);
    d.extra = ours_full
        .iter()
        .zip(&taken)
        .filter(|(_, t)| !**t)
        .map(|(l, _)| l.pmod.name.clone())
        .collect();
    if let (Some(a), Some(b)) = (&t.game_version, safe_gv.as_deref())
        && a != b
    {
        d.game_version = Some((a.clone(), b.to_owned()));
    }
    if t.regent == 1 {
        if mf.is_none() {
            d.notes
                .push("cannot verify file contents: checksum_manifest.txt not found".to_string());
        }
        let mut their_dlcs = t.disabled_dlcs.clone();
        their_dlcs.sort();
        their_dlcs.dedup();
        let mut our_dlcs = filter_known_dlcs(&p.disabled_dlcs, dlcs);
        our_dlcs.sort();
        if their_dlcs != our_dlcs {
            d.notes.push("disabled DLCs differ".to_string());
        }
        if t.incomplete && !sender_incomplete {
            d.notes
                .push("cannot verify files: the sender's export was incomplete".to_string());
        }
        if t.fingerprint.is_empty() {
            d.notes
                .push("cannot verify: the payload has no Regent fingerprint".to_string());
        } else if t.fingerprint != ours_fp && d.equal() {
            d.notes.push(format!(
                "fingerprint differs: theirs {}, ours {}",
                t.fingerprint, ours_fp
            ));
        }
    } else {
        d.notes
            .push("cannot verify file contents: imported format has no file hashes".to_string());
    }
    (d, ours_fp)
}

/// A playset from the payload. Mods we have are enabled (unless the source said off); mods we
/// lack stay as disabled entries (the game file never lists uninstalled mods) and are reported.
/// `disabled_dlcs` is filtered to scan-known DLC ids.
pub fn import(t: &Payload, mods: &[Mod], dlcs: &[Dlc]) -> (Playset, Vec<Missing>) {
    let cands: Vec<_> = mods
        .iter()
        .map(|m| (m.remote_id.as_deref(), m.name.as_str()))
        .collect();
    let mut taken = vec![false; mods.len()];
    let mut p = Playset::new(&t.name);
    let mut missing = vec![];
    for m in &t.mods {
        match locate(&cands, &taken, m) {
            Some(i) => {
                taken[i] = true;
                p.mods.push(Entry {
                    id: mods[i].id.clone(),
                    enabled: m.enabled,
                });
            }
            None if m.enabled => {
                let valid_steam = m.steam.as_deref().filter(|s| digits(s));
                if let Some(s) = valid_steam {
                    p.mods.push(Entry {
                        id: format!("mod/ugc_{s}.mod"),
                        enabled: false,
                    });
                }
                missing.push(Missing {
                    name: m.name.clone(),
                    steam: valid_steam.map(str::to_owned),
                    url: valid_steam.map(workshop_url),
                });
            }
            None => {}
        }
    }
    p.disabled_dlcs = filter_known_dlcs(&t.disabled_dlcs, dlcs);
    (p, missing)
}

/// Add an imported playset under a name no other playset of the game uses, and switch to it.
/// Returns the final name.
pub fn adopt(g: &mut GameState, mut p: Playset, mods: &[Mod]) -> String {
    let base = p.name.clone();
    let mut n = 1;
    while g.playsets.iter().any(|x| x.name == p.name) {
        n += 1;
        p.name = format!("{base} ({n})");
    }
    g.add(p);
    g.current().reconcile(mods);
    g.current().name.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{put, tmp_dir};
    use crate::index::build;
    use std::path::Path;

    const MANIFEST: &str =
        "directory\nname = common\nsub_directories = yes\nfile_extension = .txt\n";

    fn md(root: &Path, id: &str, name: &str, steam: Option<&str>, files: &[(&str, &str)]) -> Mod {
        let dir = root.join(id);
        for (f, t) in files {
            put(&dir.join(f), t);
        }
        Mod {
            id: format!("mod/{id}.mod"),
            name: name.into(),
            version: Some("1.0".into()),
            root: Some(dir),
            remote_id: steam.map(Into::into),
            ..Default::default()
        }
    }

    fn setup(tag: &str, bodies: [&str; 2]) -> (std::path::PathBuf, Vec<Mod>) {
        let b = tmp_dir(tag);
        put(&b.join("game/checksum_manifest.txt"), MANIFEST);
        let mods = vec![
            md(
                &b.join("m"),
                "a",
                "Alpha",
                Some("111"),
                &[("common/x.txt", bodies[0]), ("gfx/p.dds", "z")],
            ),
            md(
                &b.join("m"),
                "b",
                "Beta",
                None,
                &[("common/y.txt", bodies[1])],
            ),
        ];
        (b, mods)
    }

    fn ps(mods: &[Mod]) -> Playset {
        let mut p = Playset::new("Mine");
        p.mods = mods
            .iter()
            .map(|m| Entry {
                id: m.id.clone(),
                enabled: true,
            })
            .collect();
        p
    }

    fn exp(b: &Path, mods: &[Mod], p: &Playset) -> Payload {
        let mf = Manifest::load(&b.join("game"));
        let ix = build(mods, mf.as_ref());
        export("stellaris", Some("v4.6.0"), p, mods, &[], &ix, mf.as_ref())
    }

    fn ver(b: &Path, t: &Payload, gv: &str, mods: &[Mod], p: &Playset) -> Diff {
        let mf = Manifest::load(&b.join("game"));
        let ix = build(mods, mf.as_ref());
        verify_with_fingerprint(t, Some(gv), p, mods, &[], &ix, mf.as_ref()).0
    }

    #[test]
    fn base64_round_trip() {
        for n in 0..40usize {
            let d: Vec<u8> = (0..n).map(|i| (i * 37 + 250) as u8).collect();
            assert_eq!(b64_dec(&b64_enc(&d)).unwrap(), d, "len {n}");
        }
        assert_eq!(b64_enc(b"\xfb\xff\xbf"), "-_-_");
        assert!(b64_dec("a+b").is_none());
    }

    #[test]
    fn code_round_trip_and_privacy() {
        let (b, mods) = setup("share-code", ["one", "two"]);
        let p = ps(&mods);
        let x = exp(&b, &mods, &p);
        let code = to_code(&x);
        assert!(code.starts_with("regent1:"));
        assert_eq!(parse(code.as_bytes()).unwrap(), x);
        assert_eq!(parse(to_json(&x).as_bytes()).unwrap(), x);
        let wrapped = format!("  {}\n{}\n", &code[..20], &code[20..]);
        assert_eq!(parse(wrapped.as_bytes()).unwrap(), x);
        let (json, home) = (to_json(&x), b.to_string_lossy().into_owned());
        assert!(!json.contains(&home) && !json.contains("mod/a.mod") && !json.contains("/Users"));
        assert_eq!(x.mods[0].steam.as_deref(), Some("111"));
        assert!(x.mods[0].hash.is_some() && x.mods[1].hash.is_some());
        assert_eq!(x.fingerprint.len(), 16);
        // known-answer: FNV-1a 64 of "a" (stable across Rust releases, unlike DefaultHasher)
        let mut h = Fnv::new();
        h.w(b"a");
        assert_eq!(h.hex(), "af63dc4c8601ec8c");
        assert_eq!(exp(&b, &mods, &p).fingerprint, x.fingerprint);
    }

    #[test]
    fn rejects_garbage_and_bombs() {
        assert!(parse(b"regent1:!!!").is_err());
        assert!(parse(b"hello").is_err());
        assert!(parse(b"{\"regent\":9}").is_err());
        assert!(parse(b"{\"x\":1}").is_err());
        let bomb = miniz_oxide::deflate::compress_to_vec(&vec![b' '; 3 << 20], 9);
        assert!(parse(format!("regent1:{}", b64_enc(&bomb)).as_bytes()).is_err());
    }

    #[test]
    fn official_json_position_and_enabled() {
        let j = r#"{"game":"stellaris","name":"Initial","mods":[
          {"displayName":"C","enabled":true,"position":"10","steamId":"3"},
          {"displayName":"A","enabled":true,"position":2,"steamId":"1"},
          {"displayName":"B","enabled":false,"position":"3","steamId":"2"}]}"#;
        let p = parse(j.as_bytes()).unwrap();
        let names: Vec<_> = p.mods.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"], "numeric, not lexical");
        assert!(!p.mods[1].enabled);
        assert_eq!((p.game.as_str(), p.name.as_str()), ("stellaris", "Initial"));
        let (pl, missing) = import(&p, &[], &[]);
        assert_eq!(missing.len(), 2, "disabled mod B is not reported");
        assert_eq!(
            missing[0].url.as_deref(),
            Some("https://steamcommunity.com/sharedfiles/filedetails/?id=1")
        );
        assert!(
            pl.mods.iter().all(|e| !e.enabled),
            "missing mods are never enabled"
        );
    }

    fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
        let mut o = b"PK\x03\x04".to_vec();
        for v in [20u16, 0, 0, 0, 0] {
            o.extend(v.to_le_bytes());
        }
        for v in [0u32, data.len() as u32, data.len() as u32] {
            o.extend(v.to_le_bytes());
        }
        o.extend((name.len() as u16).to_le_bytes());
        o.extend([0, 0]);
        o.extend(name.as_bytes());
        o.extend(data);
        let cd_off = o.len() as u32;
        let mut cd = b"PK\x01\x02".to_vec();
        for v in [20u16, 20, 0, 0, 0, 0] {
            cd.extend(v.to_le_bytes());
        }
        for v in [0u32, data.len() as u32, data.len() as u32] {
            cd.extend(v.to_le_bytes());
        }
        cd.extend((name.len() as u16).to_le_bytes());
        cd.extend([0u8; 8]);
        cd.extend(0u32.to_le_bytes());
        cd.extend(0u32.to_le_bytes());
        cd.extend(name.as_bytes());
        o.extend(&cd);
        o.extend(b"PK\x05\x06\0\0\0\0\x01\0\x01\0");
        o.extend((cd.len() as u32).to_le_bytes());
        o.extend(cd_off.to_le_bytes());
        o.extend([0, 0]);
        o
    }

    #[test]
    fn irony_exported_json_and_zip() {
        let j = r#"{"Name":"Irony set","Game":"stellaris","Mods":["937289339","Some Mod",{"Name":"Obj","RemoteId":"55"}]}"#;
        let p = parse(j.as_bytes()).unwrap();
        assert_eq!((p.name.as_str(), p.mods.len()), ("Irony set", 3));
        assert_eq!(p.mods[0].steam.as_deref(), Some("937289339"));
        assert_eq!(p.mods[1].steam, None);
        assert_eq!(p.mods[2].steam.as_deref(), Some("55"));
        assert_eq!(
            parse(&stored_zip("pack/Exported.json", j.as_bytes())).unwrap(),
            p
        );
        assert!(parse(&stored_zip("other.txt", b"x")).is_err());
    }

    #[test]
    fn import_matches_steam_before_name_and_never_enables_missing() {
        let (_, mods) = setup("share-import", ["one", "two"]);
        let t = Payload {
            regent: 1,
            name: "Theirs".into(),
            disabled_dlcs: vec!["d1".into()],
            mods: vec![
                PMod {
                    name: "Launcher Name".into(),
                    steam: Some("111".into()),
                    enabled: true,
                    ..Default::default()
                },
                PMod {
                    name: "Beta".into(),
                    enabled: true,
                    ..Default::default()
                },
                PMod {
                    name: "Gone".into(),
                    steam: Some("999".into()),
                    enabled: true,
                    ..Default::default()
                },
                PMod {
                    name: "Nameless".into(),
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let dlcs = [Dlc {
            id: "d1".into(),
            name: "D1".into(),
            ..Default::default()
        }];
        let (p, missing) = import(&t, &mods, &dlcs);
        let on: Vec<_> = p
            .mods
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(on, ["mod/a.mod", "mod/b.mod"]);
        let gone = p.mods.iter().find(|e| e.id == "mod/ugc_999.mod").unwrap();
        assert!(!gone.enabled);
        assert_eq!(missing.len(), 2);
        assert_eq!(
            missing[0].url.as_deref(),
            Some("https://steamcommunity.com/sharedfiles/filedetails/?id=999")
        );
        assert_eq!(missing[1].url, None);
        assert_eq!(p.disabled_dlcs, ["d1"]);
    }

    #[test]
    fn verify_diff_kinds() {
        let (b, mods) = setup("share-verify", ["one", "two"]);
        let p = ps(&mods);
        let t = exp(&b, &mods, &p);
        let d = ver(&b, &t, "v4.6.0", &mods, &p);
        assert!(d.equal(), "{:?}", d.lines());
        assert_eq!(d.lines().len(), 1);

        // missing + extra
        let mut p2 = p.clone();
        p2.mods[1].enabled = false;
        let d = ver(&b, &t, "v4.6.0", &mods, &p2);
        assert_eq!(
            (d.missing.as_slice(), d.extra.len()),
            (["Beta".to_string()].as_slice(), 0)
        );
        let mut t2 = t.clone();
        t2.mods.remove(1);
        let d = ver(&b, &t2, "v4.6.0", &mods, &p);
        assert_eq!(d.extra, ["Beta"]);

        // order
        let mut p3 = p.clone();
        p3.mods.reverse();
        let d = ver(&b, &t, "v4.6.0", &mods, &p3);
        assert!(d.order && d.missing.is_empty() && d.extra.is_empty());

        // version, game version
        let mut t3 = t.clone();
        t3.mods[0].version = Some("2.0".into());
        let d = ver(&b, &t3, "v4.7.0", &mods, &p);
        assert_eq!(d.version, [("Alpha".into(), "2.0".into(), "1.0".into())]);
        assert_eq!(d.game_version, Some(("v4.6.0".into(), "v4.7.0".into())));

        // content: change a checksum-relevant file
        std::fs::write(b.join("m/a/common/x.txt"), "changed").unwrap();
        let d = ver(&b, &t, "v4.6.0", &mods, &p);
        assert_eq!(d.content, ["Alpha"]);
        // a non-manifest file does not change the hash
        std::fs::write(b.join("m/a/common/x.txt"), "one").unwrap();
        std::fs::write(b.join("m/a/gfx/p.dds"), "other").unwrap();
        assert!(ver(&b, &t, "v4.6.0", &mods, &p).equal());
    }

    #[test]
    fn explicit_workshop_id_never_falls_back_to_name_and_dlcs_filtered() {
        let (b, mods) = setup("share-strict-id", ["one", "two"]);
        // Payload names "Alpha" with a DIFFERENT Workshop id "999" (installed Alpha is "111").
        let t = Payload {
            regent: 1,
            name: "Strict".into(),
            mods: vec![PMod {
                name: "Alpha".into(),
                steam: Some("999".into()),
                enabled: true,
                ..Default::default()
            }],
            disabled_dlcs: vec!["dlc/known.dlc".into(), "/etc/passwd".into()],
            ..Default::default()
        };
        let known_dlcs = [Dlc {
            id: "dlc/known.dlc".into(),
            name: "Known".into(),
            ..Default::default()
        }];
        let (imported, missing) = import(&t, &mods, &known_dlcs);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].steam.as_deref(), Some("999"));
        assert!(
            imported.mods.iter().all(|e| !e.enabled),
            "must NOT match installed mod Alpha (111) when payload requests steam id 999"
        );
        assert_eq!(imported.disabled_dlcs, ["dlc/known.dlc"]);

        let p = ps(&mods);
        let d = ver(&b, &t, "v4.6.0", &mods, &p);
        assert_eq!(d.missing, ["Alpha"]);
    }

    #[test]
    fn export_sanitizes_unnamed_local_mods_paths_and_unknown_dlcs() {
        let b = tmp_dir("share-sanitize");
        put(&b.join("game/checksum_manifest.txt"), MANIFEST);
        let local = Mod {
            id: "mod/secret_local.mod".into(),
            name: "mod/secret_local.mod".into(), // unnamed local mod fallback
            version: Some("/Users/alice/dev/1.0".into()),
            root: Some(b.join("m/local")),
            ..Default::default()
        };
        put(&b.join("m/local/gfx/a.dds"), "x");
        let mut p = Playset::new("/Users/alice/my_playset");
        p.mods.push(Entry {
            id: local.id.clone(),
            enabled: true,
        });
        p.disabled_dlcs = vec!["dlc/ok.dlc".into(), "/secret/path.dlc".into()];
        let dlcs = [Dlc {
            id: "dlc/ok.dlc".into(),
            name: "OK".into(),
            ..Default::default()
        }];
        let mf = Manifest::load(&b.join("game"));
        let ix = build(std::slice::from_ref(&local), mf.as_ref());
        let out = export(
            "stellaris",
            Some("v4.6.0"),
            &p,
            &[local],
            &dlcs,
            &ix,
            mf.as_ref(),
        );
        let json = to_json(&out);
        assert!(!json.contains("secret_local"), "{json}");
        assert!(!json.contains("/Users/alice"), "{json}");
        assert!(!json.contains("/secret/path"), "{json}");
        assert_eq!(out.mods[0].name, "Unnamed local mod");
        assert_eq!(out.mods[0].version, None);
        assert_eq!(out.name, "Shared playset");
        assert_eq!(out.disabled_dlcs, ["dlc/ok.dlc"]);
    }

    #[test]
    fn irony_mod_paths_modnames_modids_and_zip_non_recursive() {
        let j = r#"{
            "Name": "Irony Real",
            "Game": "Stellaris",
            "Mods": ["mod/ugc_1623423360.mod", "mod/my_local.mod"],
            "ModNames": ["UI Overhaul Dynamic", "My Local Mod"],
            "ModIds": [
                {"SteamId": 1623423360, "ParadoxId": null},
                {"SteamId": null, "ParadoxId": null}
            ]
        }"#;
        let p = parse(j.as_bytes()).unwrap();
        assert_eq!(p.mods.len(), 2);
        assert_eq!(p.mods[0].name, "UI Overhaul Dynamic");
        assert_eq!(p.mods[0].steam.as_deref(), Some("1623423360"));
        assert_eq!(p.mods[1].name, "My Local Mod");
        assert_eq!(p.mods[1].steam, None);

        // Without ModNames, mod/ugc_<n>.mod still extracts steam id and never leaks raw path as name.
        let j2 = r#"{"Name":"Paths Only","Mods":["mod/ugc_999888.mod"]}"#;
        let p2 = parse(j2.as_bytes()).unwrap();
        assert_eq!(p2.mods[0].steam.as_deref(), Some("999888"));
        assert!(!p2.mods[0].name.contains("mod/"));

        // Crafted zip whose exported.json starts with PK\x03\x04 fails cleanly (no infinite recursion).
        let recursive = stored_zip("exported.json", b"PK\x03\x04not_json");
        assert!(parse(&recursive).is_err());
    }

    #[test]
    fn verify_reports_missing_evidence_dlcs_incomplete_and_fingerprint() {
        let (b, mods) = setup("share-verify-strict", ["one", "two"]);
        let dlcs = [Dlc {
            id: "dlc/u.dlc".into(),
            name: "Utopia".into(),
            ..Default::default()
        }];
        let p = ps(&mods);
        let mf = Manifest::load(&b.join("game"));
        let ix = build(&mods, mf.as_ref());
        let t = export(
            "stellaris",
            Some("v4.6.0"),
            &p,
            &mods,
            &dlcs,
            &ix,
            mf.as_ref(),
        );

        // 1. Missing manifest -> unverifiable, equal() == false
        let ix_nomf = build(&mods, None);
        let d = verify_with_fingerprint(&t, Some("v4.6.0"), &p, &mods, &dlcs, &ix_nomf, None).0;
        assert!(!d.equal());
        assert!(d.lines().iter().any(|l| l.contains("checksum_manifest")));

        // 2. Differing disabled_dlcs -> equal() == false
        let mut p_dlc = p.clone();
        p_dlc.disabled_dlcs = vec!["dlc/u.dlc".into()];
        let d =
            verify_with_fingerprint(&t, Some("v4.6.0"), &p_dlc, &mods, &dlcs, &ix, mf.as_ref()).0;
        assert!(!d.equal());
        assert!(d.lines().iter().any(|l| l.contains("disabled DLCs differ")));

        // 3. One-sided (Some, None) hash difference -> content diff
        let mut t_nohash = t.clone();
        t_nohash.mods[0].hash = None;
        let d = verify_with_fingerprint(
            &t_nohash,
            Some("v4.6.0"),
            &p,
            &mods,
            &dlcs,
            &ix,
            mf.as_ref(),
        )
        .0;
        assert!(!d.equal());
        assert_eq!(d.content, ["Alpha"]);

        // 4. Tampered fingerprint -> equal() == false
        let mut t_badfp = t.clone();
        t_badfp.fingerprint = "0000000000000000".into();
        let d =
            verify_with_fingerprint(&t_badfp, Some("v4.6.0"), &p, &mods, &dlcs, &ix, mf.as_ref()).0;
        assert!(!d.equal());
        assert!(d.lines().iter().any(|l| l.contains("fingerprint differs")));

        // 5. Foreign format without hashes -> unverifiable
        let foreign = parse(r#"{"game":"stellaris","name":"F","mods":[{"displayName":"Alpha","steamId":"111","enabled":true},{"displayName":"Beta","enabled":true}]}"#.as_bytes()).unwrap();
        let d =
            verify_with_fingerprint(&foreign, Some("v4.6.0"), &p, &mods, &dlcs, &ix, mf.as_ref()).0;
        assert!(!d.equal());
        assert!(d.lines().iter().any(|l| l.contains("no file hashes")));
    }
    #[test]
    fn r3_6_malformed_workshop_id_is_rejected_and_absent_fingerprint_is_unverified() {
        let bad = br#"{"regent":1,"name":"P","mods":[{"name":"Alpha","steam":"12x"}]}"#;
        let e = parse(bad).unwrap_err().to_string();
        assert!(e.contains("Alpha") && e.contains("12x"), "{e}");
        let bad = br#"{"game":"stellaris","mods":[{"displayName":"Alpha","steamId":"12x"}]}"#;
        assert!(parse(bad).is_err());
        // An empty id is simply absent.
        let ok = br#"{"game":"stellaris","mods":[{"displayName":"Alpha","steamId":""}]}"#;
        assert!(parse(ok).unwrap().mods[0].steam.is_none());

        let (b, mods) = setup("r3-6-fp", ["one", "two"]);
        let p = ps(&mods);
        let mf = Manifest::load(&b.join("game"));
        let ix = build(&mods, mf.as_ref());
        let mut t = export(
            "stellaris",
            Some("v4.6.0"),
            &p,
            &mods,
            &[],
            &ix,
            mf.as_ref(),
        );
        assert!(
            verify_with_fingerprint(&t, Some("v4.6.0"), &p, &mods, &[], &ix, mf.as_ref())
                .0
                .equal()
        );
        t.fingerprint.clear();
        let d = verify_with_fingerprint(&t, Some("v4.6.0"), &p, &mods, &[], &ix, mf.as_ref()).0;
        assert!(!d.equal());
        assert!(
            d.lines()
                .iter()
                .any(|l| l.contains("no Regent fingerprint"))
        );
    }

    #[test]
    fn r3_7_sender_incompleteness_survives_export_and_blocks_equal() {
        let (b, mods) = setup("r3-7-incomplete", ["one", "two"]);
        let p = ps(&mods);
        let mf = Manifest::load(&b.join("game"));
        // The sender cannot read the first mod's files.
        let mut sender_mods = mods.clone();
        sender_mods[0].root = Some(b.join("vanished"));
        let ix_s = build(&sender_mods, mf.as_ref());
        let t = export(
            "stellaris",
            Some("v4.6.0"),
            &p,
            &sender_mods,
            &[],
            &ix_s,
            mf.as_ref(),
        );
        assert!(t.incomplete && t.mods[0].incomplete && t.mods[0].hash.is_none());
        assert!(!t.mods[1].incomplete);
        // The wire form round-trips the flag.
        let back = parse(&serde_json::to_vec(&t).unwrap()).unwrap();
        assert!(back.incomplete && back.mods[0].incomplete);
        // A complete receiver must not report "identical".
        let ix_r = build(&mods, mf.as_ref());
        let d =
            verify_with_fingerprint(&back, Some("v4.6.0"), &p, &mods, &[], &ix_r, mf.as_ref()).0;
        assert!(!d.equal(), "{:?}", d.lines());
        assert!(
            d.lines()
                .iter()
                .any(|l| l.contains("sender could not fully read"))
        );
    }
}
