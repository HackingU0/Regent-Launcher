//! Crash diagnosis: which active mod do the files named in a crash's error.log point at?
//! Pure correlation: the game's stack trace carries no mod information, so this only counts
//! how many log entries name files that exactly one active mod ships.
use crate::index::{FileIndex, norm};
use crate::mods::Mod;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Entries this close (seconds, mod 86,400) to the last entry count as "recent".
const RECENT: u32 = 10;
/// Maximum tail bytes read from `error.log` (8 MiB).
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum bytes read from `meta.yml` or `exception.txt` (1 MiB).
const MAX_META_BYTES: u64 = 1024 * 1024;

/// One error.log entry: `[hh:mm:ss][file.cpp:n]: text` plus its continuation lines.
pub struct Entry {
    pub secs: u32,
    pub text: String,
}

/// Crash directories under `<data_dir>/crashes`, newest first (names embed the timestamp).
/// Rejects symlinks and directories outside `<data_dir>/crashes`.
pub fn list(data_dir: &Path) -> Vec<PathBuf> {
    let root = data_dir.join("crashes");
    let Ok(rd) = std::fs::read_dir(&root) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| crate::mods::contained_dir(&root, p).is_some())
        .collect();
    v.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    v
}

/// The live log's directory (holds error.log, no meta.yml).
pub fn live_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

fn header_secs(line: &str) -> Option<u32> {
    let b = line.as_bytes();
    let d = |i: usize| (b[i] as char).to_digit(10);
    if b.len() < 11 || b[0] != b'[' || b[3] != b':' || b[6] != b':' || b[9] != b']' || b[10] != b'['
    {
        return None;
    }
    let n = |i: usize| Some(d(i)? * 10 + d(i + 1)?);
    let (hh, mm, ss) = (n(1)?, n(4)?, n(7)?);
    if hh >= 24 || mm >= 60 || ss >= 60 {
        return None;
    }
    Some(hh * 3600 + mm * 60 + ss)
}

/// Split a log into entries; lines before the first header belong to nothing and are dropped.
pub fn entries(log: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = vec![];
    for line in log.lines() {
        match header_secs(line) {
            Some(secs) => out.push(Entry {
                secs,
                text: line.to_owned(),
            }),
            None => {
                if let Some(e) = out.last_mut() {
                    e.text.push('\n');
                    e.text.push_str(line);
                }
            }
        }
    }
    out
}

fn edge(c: char) -> bool {
    !c.is_alphanumeric() && c != '_'
}

/// Distinct game-relative file paths an entry names: tokens with a '/' and a file extension,
/// cut at the first ':' (drops `:388` and `:273(inline_script)`), case-insensitively deduped.
pub fn paths(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for tok in text.split_whitespace() {
        let t = tok.trim_start_matches(edge);
        let t = t.split(':').next().unwrap_or("").trim_end_matches(edge);
        let Some((_, file)) = t.rsplit_once('/') else {
            continue;
        };
        let ext_ok = file
            .rsplit_once('.')
            .is_some_and(|(s, e)| !s.is_empty() && (1..=6).contains(&e.len()));
        if ext_ok && !t.split('/').any(|s| s == "..") && seen.insert(norm(t)) {
            out.push(t.to_owned());
        }
    }
    out
}

#[derive(Default)]
pub struct Meta {
    pub app_version: Option<String>,
    pub checksum: Option<String>,
    /// Mod_n values in n order.
    pub mods: Vec<String>,
}

pub fn parse_meta(text: &str) -> Meta {
    let mut m = Meta::default();
    let mut mods: Vec<(u32, String)> = vec![];
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim().trim_matches('"').trim());
        match k {
            "AppVersion" => m.app_version = Some(v.to_owned()),
            "ChecksumStatus" => m.checksum = Some(v.to_owned()),
            _ => {
                if let Some(n) = k.strip_prefix("Mod_").and_then(|n| n.parse().ok()) {
                    mods.push((n, v.to_owned()));
                }
            }
        }
    }
    mods.sort();
    m.mods = mods.into_iter().map(|(_, v)| v).collect();
    m
}

/// Per-mod counts, in entries.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rank {
    pub id: String,
    pub name: String,
    /// Entries naming a file only this mod ships (among the active mods).
    pub only: usize,
    pub only_recent: usize,
    /// Entries naming a file this mod shares with another active mod (shared for every owner).
    pub shared: usize,
    pub shared_recent: usize,
}

/// Entry counts per bucket; an entry naming files in several buckets counts in each.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Buckets {
    pub only: usize,
    pub shared: usize,
    pub vanilla: usize,
    /// No installed mod ships the file and the game dir lacks it.
    pub none: usize,
    /// Only installed mods that were not active at the crash ship it.
    pub inactive: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub dir: PathBuf,
    pub live: bool,
    pub app_version: Option<String>,
    pub signal: Option<String>,
    pub checksum: Option<String>,
    pub total: usize,
    /// Entries naming at least one file path.
    pub named: usize,
    pub buckets: Buckets,
    /// Mods that were active at the crash (meta.yml) but are not installed now.
    pub missing: Vec<String>,
    pub crash_mods: usize,
    /// Best first; mods with no attributed entry are left out.
    pub ranked: Vec<Rank>,
}

fn open_contained(dir: &Path, name: &str) -> Option<std::fs::File> {
    let path = dir.join(name);
    let c_path = crate::mods::contained_file(dir, &path)?;
    let pre = std::fs::symlink_metadata(&path).ok()?;
    if pre.file_type().is_symlink() || !pre.file_type().is_file() {
        return None;
    }
    let f = std::fs::File::open(&c_path).ok()?;
    let opened = f.metadata().ok()?;
    if !opened.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (pre.dev(), pre.ino()) != (opened.dev(), opened.ino()) {
            return None;
        }
        let post = std::fs::symlink_metadata(&path).ok()?;
        if post.file_type().is_symlink() || (post.dev(), post.ino()) != (opened.dev(), opened.ino())
        {
            return None;
        }
    }
    Some(f)
}

fn read_lossy_capped(dir: &Path, name: &str, max_bytes: u64, from_tail: bool) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = open_contained(dir, name)?;
    let len = f.metadata().ok()?.len();
    let mut buf = Vec::with_capacity((len.min(max_bytes) as usize).min(1 << 20));
    let truncated = from_tail && len > max_bytes;
    if truncated {
        f.seek(SeekFrom::Start(len - max_bytes)).ok()?;
    }
    f.take(max_bytes).read_to_end(&mut buf).ok()?;
    if truncated && let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        buf.drain(..=pos);
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Diagnose `<dir>/error.log`. Active mods are meta.yml's Mod_n names matched to installed mods
/// by name; with no meta.yml (the live log) `fallback` ids are the active ones.
pub fn diagnose(
    dir: &Path,
    ix: &FileIndex,
    mods: &[Mod],
    game_dir: &Path,
    fallback: &[String],
) -> Result<Report, String> {
    let log = read_lossy_capped(dir, "error.log", MAX_LOG_BYTES, true)
        .ok_or_else(|| format!("cannot read {}", dir.join("error.log").display()))?;
    let meta = read_lossy_capped(dir, "meta.yml", MAX_META_BYTES, false).map(|t| parse_meta(&t));
    let signal = read_lossy_capped(dir, "exception.txt", MAX_META_BYTES, false).and_then(|t| {
        t.lines()
            .find(|l| l.starts_with("Caught signal"))
            .map(str::to_owned)
    });
    let (active, missing, crash_mods): (Vec<String>, Vec<String>, usize) = match &meta {
        Some(m) => {
            let by_name: HashMap<String, Vec<&Mod>> =
                mods.iter().fold(HashMap::new(), |mut a, x| {
                    a.entry(x.name.trim().to_lowercase()).or_default().push(x);
                    a
                });
            let (mut act, mut miss) = (vec![], vec![]);
            for n in &m.mods {
                match by_name.get(&n.to_lowercase()) {
                    Some(v) => act.extend(v.iter().map(|x| x.id.clone())),
                    None => miss.push(n.clone()),
                }
            }
            (act, miss, m.mods.len())
        }
        None => (fallback.to_vec(), vec![], 0),
    };
    let active: HashSet<&str> = active.iter().map(String::as_str).collect();
    let name_of = |id: &str| {
        mods.iter()
            .find(|m| m.id == id)
            .map_or(id.to_owned(), |m| m.name.clone())
    };

    let es = entries(&log);
    let last = es.last().map_or(0, |e| e.secs);
    let mut vanilla: HashMap<String, bool> = HashMap::new();
    let mut ranks: HashMap<String, Rank> = HashMap::new();
    let (mut b, mut named) = (Buckets::default(), 0);
    for e in &es {
        let ps = paths(&e.text);
        if ps.is_empty() {
            continue;
        }
        named += 1;
        let recent = (last + 86_400 - e.secs) % 86_400 <= RECENT;
        let (mut only, mut shared) = (HashSet::new(), HashSet::new());
        let (mut van, mut none, mut inactive) = (false, false, false);
        for p in &ps {
            let owners = ix.by_path.get(&norm(p));
            let act: Vec<&String> = owners
                .into_iter()
                .flatten()
                .filter(|o| active.contains(o.as_str()))
                .collect();
            match act[..] {
                [one] => {
                    only.insert(one.clone());
                }
                [] => {
                    let v = *vanilla
                        .entry(norm(p))
                        .or_insert_with(|| game_dir.join(p).exists());
                    if v {
                        van = true;
                    } else if owners.is_some_and(|o| !o.is_empty()) {
                        inactive = true;
                    } else {
                        none = true;
                    }
                }
                _ => shared.extend(act.into_iter().cloned()),
            }
        }
        for (cond, n) in [
            (!only.is_empty(), &mut b.only),
            (!shared.is_empty(), &mut b.shared),
            (van, &mut b.vanilla),
            (none, &mut b.none),
            (inactive, &mut b.inactive),
        ] {
            *n += cond as usize;
        }
        for (set, is_only) in [(&only, true), (&shared, false)] {
            for id in set {
                let r = ranks.entry(id.clone()).or_insert_with(|| Rank {
                    id: id.clone(),
                    name: name_of(id),
                    only: 0,
                    only_recent: 0,
                    shared: 0,
                    shared_recent: 0,
                });
                let (n, nr) = if is_only {
                    (&mut r.only, &mut r.only_recent)
                } else {
                    (&mut r.shared, &mut r.shared_recent)
                };
                *n += 1;
                *nr += recent as usize;
            }
        }
    }
    let mut ranked: Vec<Rank> = ranks.into_values().collect();
    ranked.sort_by(|a, b| {
        (b.only_recent, b.only, b.shared_recent, b.shared)
            .cmp(&(a.only_recent, a.only, a.shared_recent, a.shared))
            .then(a.name.cmp(&b.name))
    });
    Ok(Report {
        dir: dir.to_owned(),
        live: meta.is_none(),
        app_version: meta.as_ref().and_then(|m| m.app_version.clone()),
        signal,
        checksum: meta.and_then(|m| m.checksum),
        total: es.len(),
        named,
        buckets: b,
        missing,
        crash_mods,
        ranked,
    })
}

fn num(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Report {
    /// Plain-text report lines, sent to the GUI with the report.
    pub fn text(&self) -> Vec<String> {
        let name = self
            .dir
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let mut head = if self.live {
            "Live error.log".to_string()
        } else {
            format!("Crash {name}")
        };
        let extra: Vec<String> = [
            self.app_version.as_ref().map(|v| format!("game {v}")),
            self.signal.clone(),
            self.checksum.as_ref().map(|c| format!("checksum {c}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !extra.is_empty() {
            head += &format!(" ({})", extra.join(", "));
        }
        let b = &self.buckets;
        let mut l = vec![head];
        l.push(format!(
            "error.log: {} entries, {} name a file path.",
            num(self.total),
            num(self.named)
        ));
        match self.ranked.first().filter(|r| r.only > 0) {
            Some(r) => l.push(format!(
                "Most likely {}: {} of {} entries name files only {} ships, {} in last {RECENT} s.",
                r.name,
                num(r.only),
                num(self.total),
                r.name,
                num(r.only_recent)
            )),
            None => l.push(
                "No single mod stands out: no entry names a file only one active mod ships.".into(),
            ),
        }
        if !self.ranked.is_empty() {
            l.push("Ranked by entries in the last 10 s, then total:".into());
            for (i, r) in self.ranked.iter().take(10).enumerate() {
                l.push(format!(
                    "  {}. {}: only it {} ({} recent), shared with other mods {} ({} recent)",
                    i + 1,
                    r.name,
                    num(r.only),
                    num(r.only_recent),
                    num(r.shared),
                    num(r.shared_recent)
                ));
            }
        }
        let van = if self.missing.is_empty() {
            "vanilla"
        } else {
            "vanilla (or overridden by a missing mod)"
        };
        l.push(format!(
            "Entries by file (an entry can appear in several): only one active mod {}, shared by several {}, {van} {}, no installed mod ships {}{}.",
            num(b.only),
            num(b.shared),
            num(b.vanilla),
            num(b.none),
            if b.inactive > 0 {
                format!(", only installed mods not active at the crash {}", num(b.inactive))
            } else {
                String::new()
            }
        ));
        if !self.missing.is_empty() {
            l.push(format!(
                "Active at the crash but no longer installed ({} of {}): {}.",
                self.missing.len(),
                self.crash_mods,
                self.missing.join(", ")
            ));
        }
        l.push(
            "The crash's stack trace carries no mod information; this is correlation from file names in error.log, not proof."
                .into(),
        );
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{put, tmp_dir};

    const LOG: &str = "junk before\n\
[23:59:50][a.cpp:1]: old noise common/a/only_a.txt:12 here\n\
[23:59:58][a.cpp:2]: Wrong scope at common/a/only_a.txt:388 @ x at common/a/only_a.txt:9 @ y\n\
Current scope: pop_group\n\
[00:00:03][a.cpp:3]: two mods common/shared/s.txt:273(inline_script) and \"gfx/g/vanilla.dds\",\n\
[00:00:04][a.cpp:4]: nothing here\n\
[00:00:05][a.cpp:5]: missing: events/ghost.txt line: 3 and Common/B/Only_B.txt.\n";

    fn fx() -> (PathBuf, FileIndex, Vec<Mod>, PathBuf) {
        let b = tmp_dir("crash");
        put(&b.join("game/gfx/g/vanilla.dds"), "x");
        let by = |v: &[(&str, &[&str])]| {
            v.iter()
                .map(|(p, o)| (p.to_string(), o.iter().map(|s| s.to_string()).collect()))
                .collect()
        };
        let ix = FileIndex {
            mods: HashMap::new(),
            by_path: by(&[
                ("common/a/only_a.txt", &["a"]),
                ("common/shared/s.txt", &["a", "b"]),
                ("common/b/only_b.txt", &["b"]),
                ("events/ghost.txt", &["c"]),
            ]),
            has_manifest: false,
        };
        let m = |id: &str, name: &str| Mod {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        };
        (
            b.clone(),
            ix,
            vec![m("a", "Mod A"), m("b", "Mod B"), m("c", "Mod C")],
            b.join("game"),
        )
    }

    #[test]
    fn parsing_entries_and_paths() {
        let es = entries(LOG);
        assert_eq!(es.len(), 5);
        assert_eq!(es[1].secs, 86_398);
        assert!(es[1].text.ends_with("Current scope: pop_group"));
        // two mentions of one file in an entry, :388 cut
        assert_eq!(paths(&es[1].text), ["common/a/only_a.txt"]);
        // (inline_script) and quotes/commas stripped; "file:" and "line:" ignored
        assert_eq!(
            paths(&es[2].text),
            ["common/shared/s.txt", "gfx/g/vanilla.dds"]
        );
        assert!(
            paths("Expected colon at line 5 in a/b without ext, 1/2 and http://x.y/z").is_empty()
        );
        // case-insensitive dedupe
        assert_eq!(paths("A/b.txt a/B.TXT"), ["A/b.txt"]);
        assert!(paths("a/../y.txt").is_empty());
    }

    #[test]
    fn meta_keys_have_trailing_spaces() {
        let m = parse_meta(
            "AppVersion: 4.6.0 \nMod_10: Ten \nMod_2: Two \nChecksumStatus: Modified \nDLC_1: x\n",
        );
        assert_eq!(m.mods, ["Two", "Ten"]);
        assert_eq!(
            (m.app_version.unwrap(), m.checksum.unwrap()),
            ("4.6.0".into(), "Modified".into())
        );
    }

    #[test]
    fn buckets_ranking_midnight_and_missing() {
        let (b, ix, mods, game) = fx();
        let d = b.join("crashes/stellaris_1");
        put(&d.join("error.log"), LOG);
        put(&d.join("exception.txt"), "x\nCaught signal 11 (SIGSEGV)\n");
        // Mod C (ghost.txt) is installed but not in the crash list; Mod X is gone.
        put(
            &d.join("meta.yml"),
            "Mod_1: Mod A \nMod_2: Mod B \nMod_3: Mod X \nChecksumStatus: Modified \n",
        );
        let r = diagnose(&d, &ix, &mods, &game, &[]).unwrap();
        assert_eq!((r.total, r.named), (5, 4));
        assert_eq!(r.signal.as_deref(), Some("Caught signal 11 (SIGSEGV)"));
        assert_eq!(r.missing, ["Mod X"]);
        let k = &r.buckets;
        assert_eq!(
            (k.only, k.shared, k.vanilla, k.none, k.inactive),
            (3, 1, 1, 0, 1)
        );
        // last entry is 00:00:05: entries at 23:59:58 (7 s) and 00:00:03 are recent, 23:59:50 (15 s) is not
        let (a, bb) = (&r.ranked[0], &r.ranked[1]);
        assert_eq!((a.name.as_str(), a.only, a.only_recent), ("Mod A", 2, 1));
        // shared is shared for each owner, never credited by load order
        assert_eq!((a.shared, a.shared_recent), (1, 1));
        assert_eq!(
            (bb.name.as_str(), bb.only, bb.only_recent, bb.shared),
            ("Mod B", 1, 1, 1)
        );
        assert!(r.ranked.len() == 2);
        let t = r.text().join("\n");
        assert!(
            t.contains(
                "Most likely Mod A: 2 of 5 entries name files only Mod A ships, 1 in last 10 s."
            ),
            "{t}"
        );
        assert!(t.contains("vanilla (or overridden by a missing mod) 1"));
        assert!(t.contains("(1 of 3): Mod X"));
        assert!(t.contains("no mod information") || t.contains("no mod information;"));
    }

    #[test]
    fn no_missing_mods_plain_vanilla_and_ranking_by_recent_first() {
        let (b, ix, mods, game) = fx();
        let d = b.join("crashes/stellaris_2");
        // B has more entries overall; A has more in the last 10 s.
        let log = "[10:00:00][x.cpp:1]: common/b/only_b.txt\n[10:00:01][x.cpp:1]: common/b/only_b.txt\n[10:00:02][x.cpp:1]: common/b/only_b.txt\n[10:01:00][x.cpp:1]: common/a/only_a.txt gfx/g/vanilla.dds\n";
        put(&d.join("error.log"), log);
        put(&d.join("meta.yml"), "Mod_1: Mod A\nMod_2: Mod B\n");
        let r = diagnose(&d, &ix, &mods, &game, &[]).unwrap();
        assert_eq!(r.ranked[0].name, "Mod A");
        assert!(r.missing.is_empty());
        assert!(
            r.text().join("\n").contains("vanilla 1,") || r.text().join("\n").contains("vanilla 1")
        );
        assert!(!r.text().join("\n").contains("overridden"));
        // live: no meta.yml, fallback ids are active
        let live = b.join("logs");
        put(&live.join("error.log"), log);
        let r = diagnose(&live, &ix, &mods, &game, &["b".into()]).unwrap();
        assert!(r.live && r.ranked.len() == 1 && r.ranked[0].only == 3);
        assert_eq!(list(&b)[0], d);
        assert!(diagnose(&b.join("nope"), &ix, &mods, &game, &[]).is_err());
    }

    #[test]
    fn thousands() {
        assert_eq!(
            [num(0), num(999), num(1000), num(1741), num(1234567)],
            ["0", "999", "1,000", "1,741", "1,234,567"]
        );
    }

    #[test]
    fn rejects_out_of_range_timestamps_and_caps_large_logs() {
        let (b, ix, mods, game) = fx();
        let d = b.join("crashes/bad_clock");
        let log = "[99:99:99][a.cpp:1]: common/a/only_a.txt\n\
                   [24:00:00][a.cpp:2]: common/a/only_a.txt\n\
                   [12:60:00][a.cpp:3]: common/a/only_a.txt\n\
                   [12:00:60][a.cpp:4]: common/a/only_a.txt\n\
                   [00:00:05][a.cpp:5]: common/b/only_b.txt\n";
        put(&d.join("error.log"), log);
        put(&d.join("meta.yml"), "Mod_1: Mod A\nMod_2: Mod B\n");
        assert!(header_secs("[99:99:99][x]").is_none());
        assert!(header_secs("[24:00:00][x]").is_none());
        assert!(header_secs("[12:60:00][x]").is_none());
        assert!(header_secs("[12:00:60][x]").is_none());
        assert_eq!(header_secs("[23:59:59][x]"), Some(86_399));
        let r = diagnose(&d, &ix, &mods, &game, &[]).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.ranked[0].name, "Mod B");

        // Large log (> 8 MB) reads only the tail without failing and attributes tail entries.
        let big_dir = b.join("crashes/huge_log");
        std::fs::create_dir_all(&big_dir).unwrap();
        let mut huge = "[00:00:01][a.cpp:1]: common/a/only_a.txt\n".to_string();
        let pad_line = "x".repeat(1023) + "\n";
        while huge.len() < (MAX_LOG_BYTES as usize) + 64 * 1024 {
            huge.push_str(&pad_line);
        }
        huge.push_str("[00:01:00][a.cpp:2]: common/b/only_b.txt\n");
        std::fs::write(big_dir.join("error.log"), huge).unwrap();
        put(&big_dir.join("meta.yml"), "Mod_1: Mod A\nMod_2: Mod B\n");
        let r = diagnose(&big_dir, &ix, &mods, &game, &[]).unwrap();
        assert_eq!(r.total, 1, "head entry beyond 8 MB cap was skipped");
        assert_eq!(r.ranked.len(), 1);
        assert_eq!(r.ranked[0].name, "Mod B");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_crash_dir_and_log_rejected() {
        use std::os::unix::fs::symlink;
        let (b, ix, mods, game) = fx();
        let ext = b.join("outside_crash");
        put(
            &ext.join("error.log"),
            "[00:00:01][a.cpp:1]: common/a/only_a.txt\n",
        );
        let crashes = b.join("crashes");
        std::fs::create_dir_all(&crashes).unwrap();
        symlink(&ext, crashes.join("linked_crash")).unwrap();
        assert!(list(&b).is_empty(), "symlinked crash dir omitted from list");

        // Real crash dir with symlinked error.log pointing outside is rejected.
        let real = crashes.join("real_crash");
        std::fs::create_dir_all(&real).unwrap();
        symlink(ext.join("error.log"), real.join("error.log")).unwrap();
        assert!(diagnose(&real, &ix, &mods, &game, &[]).is_err());
    }
}
