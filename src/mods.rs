//! Clausewitz / VDF parsing, mod and DLC scanning, version matching. UI-free.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq)]
pub(crate) enum Tok {
    Str(String),
    Atom(String),
    Eq,
    Open,
    Close,
}

/// Tokens with their byte ranges into `text` (BOM offset kept; a `Str` range includes its
/// quotes). The flag is true when a quote was never closed.
pub(crate) fn tokens(text: &str) -> (Vec<(Tok, Range<usize>)>, bool) {
    let (mut out, mut unterminated) = (vec![], false);
    let start = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut it = text[start..]
        .char_indices()
        .map(|(i, c)| (i + start, c))
        .peekable();
    while let Some((i, c)) = it.next() {
        let mut end = i + c.len_utf8();
        match c {
            '#' => {
                for (_, c) in it.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '=' => out.push((Tok::Eq, i..end)),
            '{' => out.push((Tok::Open, i..end)),
            '}' => out.push((Tok::Close, i..end)),
            '"' => {
                let (mut s, mut closed) = (String::new(), false);
                while let Some((j, c)) = it.next() {
                    end = j + c.len_utf8();
                    match c {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match it.peek() {
                            Some(&(k, n)) if n == '\\' || n == '"' => {
                                s.push(n);
                                end = k + 1;
                                it.next();
                            }
                            _ => s.push('\\'),
                        },
                        c => s.push(c),
                    }
                }
                unterminated |= !closed;
                out.push((Tok::Str(s), i..end));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = String::from(c);
                while let Some(&(j, n)) = it.peek() {
                    if n.is_whitespace() || "={}\"#".contains(n) {
                        break;
                    }
                    s.push(n);
                    end = j + n.len_utf8();
                    it.next();
                }
                out.push((Tok::Atom(s), i..end));
            }
        }
    }
    (out, unterminated)
}

#[derive(Debug, PartialEq)]
pub enum Value {
    Scalar(String),
    List(Vec<String>),
}

/// Top-level `key = value` / `key = { a b }` pairs. Nested blocks are flattened into the list.
pub fn parse_pdx(text: &str) -> Vec<(String, Value)> {
    let toks: Vec<Tok> = tokens(text).0.into_iter().map(|t| t.0).collect();
    let (mut out, mut i) = (vec![], 0);
    while i < toks.len() {
        let key = match &toks[i] {
            Tok::Str(s) | Tok::Atom(s) if toks.get(i + 1) == Some(&Tok::Eq) => s.clone(),
            _ => {
                i += 1;
                continue;
            }
        };
        i += 2;
        match toks.get(i) {
            Some(Tok::Str(s) | Tok::Atom(s)) => {
                out.push((key, Value::Scalar(s.clone())));
                i += 1;
            }
            Some(Tok::Open) => {
                i += 1;
                let (mut depth, mut items) = (1, vec![]);
                while i < toks.len() && depth > 0 {
                    match &toks[i] {
                        Tok::Open => depth += 1,
                        Tok::Close => depth -= 1,
                        Tok::Str(s) | Tok::Atom(s) if depth == 1 => items.push(s.clone()),
                        _ => {}
                    }
                    i += 1;
                }
                out.push((key, Value::List(items)));
            }
            _ => {}
        }
    }
    out
}

/// Every `"path" "<string>"` pair in a libraryfolders.vdf.
pub fn vdf_paths(text: &str) -> Vec<PathBuf> {
    let toks: Vec<Tok> = tokens(text).0.into_iter().map(|t| t.0).collect();
    toks.windows(2)
        .filter_map(|w| match w {
            [Tok::Str(k), Tok::Str(v)] if k == "path" => Some(PathBuf::from(v)),
            _ => None,
        })
        .collect()
}

fn scalar(f: &[(String, Value)], key: &str) -> Option<String> {
    f.iter().find_map(|(k, v)| match v {
        Value::Scalar(s) if k == key => Some(s.clone()),
        _ => None,
    })
}

fn list(f: &[(String, Value)], key: &str) -> Vec<String> {
    f.iter()
        .find_map(|(k, v)| match v {
            Value::List(l) if k == key => Some(l.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
pub struct Mod {
    /// Entry in dlc_load.json, e.g. "mod/ugc_123.mod".
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub supported_version: Option<String>,
    /// Mod names this mod depends on.
    pub dependencies: Vec<String>,
    /// Descriptor text to write to <data>/mod/<id file> at launch (workshop-only mods).
    pub needs_descriptor: Option<String>,
    /// Mod directory, or the archive file for `archive=` mods (the file index treats a file as a zip).
    pub root: Option<PathBuf>,
    /// `remote_file_id`, else the n of `ugc_<n>.mod`.
    pub remote_id: Option<String>,
    /// Every `replace_path` scalar.
    pub replace_paths: Vec<String>,
    /// `picture=` scalar from the descriptor, if any.
    pub picture: Option<String>,
}

impl Mod {
    /// True when the mod declares a supported_version that the game version does not match.
    pub fn mismatch(&self, game_raw: Option<&str>) -> bool {
        matches!((&self.supported_version, game_raw), (Some(s), Some(g)) if !version_matches(s, g))
    }
}

fn mod_from(
    id: String,
    text: &str,
    needs_descriptor: Option<String>,
    root: Option<PathBuf>,
) -> Mod {
    let f = parse_pdx(text);
    let name = scalar(&f, "name").unwrap_or_else(|| id.clone());
    let remote_id = scalar(&f, "remote_file_id").or_else(|| {
        id.strip_prefix("mod/ugc_")?
            .strip_suffix(".mod")
            .map(str::to_owned)
    });
    let replace_paths = f
        .iter()
        .filter_map(|(k, v)| match v {
            Value::Scalar(s) if k == "replace_path" => Some(s.clone()),
            _ => None,
        })
        .collect();
    Mod {
        root,
        remote_id,
        replace_paths,
        picture: scalar(&f, "picture"),
        name,
        version: scalar(&f, "version"),
        supported_version: scalar(&f, "supported_version"),
        dependencies: list(&f, "dependencies"),
        needs_descriptor,
        id,
    }
}

/// Strip leading `v`, compare dotted parts pairwise; `*` on either side matches anything.
pub fn version_matches(supported: &str, game_raw: &str) -> bool {
    let parts = |s: &str| {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    parts(supported)
        .iter()
        .zip(parts(game_raw))
        .all(|(s, g)| s == "*" || g == "*" || *s == g)
}

/// Remove path=/archive= lines and append `path="<abs_dir>"`.
pub fn synth_descriptor(text: &str, abs_dir: &Path) -> String {
    let is_key = |l: &str, k: &str| {
        l.trim()
            .strip_prefix(k)
            .is_some_and(|r| r.trim_start().starts_with('='))
    };
    let mut s: String = text
        .trim_start_matches('\u{feff}')
        .lines()
        .filter(|l| !is_key(l, "path") && !is_key(l, "archive"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = abs_dir.to_string_lossy().replace('\\', "/");
    s.push_str(&format!("\npath=\"{dir}\"\n"));
    s
}

/// Descriptors are not always UTF-8 (old Windows-1252 mods); read lossily rather than drop them.
pub(crate) fn read_text(p: &Path) -> Option<String> {
    fs::read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn descriptor_target(f: &[(String, Value)], data_dir: &Path) -> Option<PathBuf> {
    let p = PathBuf::from(scalar(f, "path").or_else(|| scalar(f, "archive"))?);
    Some(if p.is_relative() { data_dir.join(p) } else { p })
}

/// Workshop items are authoritative for `mod/ugc_<id>.mod`; then <data>/mod/*.mod
/// whose target still exists.
pub fn scan_mods(data_dir: &Path, workshop: Option<&Path>) -> Vec<Mod> {
    let (mut out, mut seen) = (vec![], HashSet::new());
    let docs = data_dir.join("mod");
    // ponytail: synchronous scan on the caller's thread; move to a background task if 1000+ mods stutter startup.
    for dir in workshop.map(sorted_entries).unwrap_or_default() {
        let Some(n) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(text) = read_text(&dir.join("descriptor.mod")) else {
            continue;
        };
        let current = read_text(&docs.join(format!("ugc_{n}.mod")))
            .and_then(|t| descriptor_target(&parse_pdx(&t), data_dir))
            .filter(|c| c.starts_with(&dir) && c.exists());
        // An existing descriptor pointing at this dir (or an archive inside it) is kept as is.
        let needs = current.is_none().then(|| synth_descriptor(&text, &dir));
        let root = current.filter(|c| c.is_file()).unwrap_or(dir.clone());
        let id = format!("mod/ugc_{n}.mod");
        seen.insert(id.clone());
        out.push(mod_from(id, &text, needs, Some(root)));
    }
    for p in sorted_entries(&docs) {
        let Some(file) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let id = format!("mod/{file}");
        if p.extension().is_none_or(|e| e != "mod") || seen.contains(&id) {
            continue;
        }
        let Some(text) = read_text(&p) else {
            continue;
        };
        if let Some(t) = descriptor_target(&parse_pdx(&text), data_dir).filter(|t| t.exists()) {
            out.push(mod_from(id, &text, None, Some(t)));
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct Dlc {
    /// Game-dir-relative, e.g. "dlc/dlc014_utopia/dlc014.dlc".
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub thumbnail: Option<String>,
    pub affects_checksum: bool,
}

/// Physically installed DLCs only: dlc/*.dlc and dlc/*/*.dlc.
pub fn scan_dlcs(game_dir: &Path) -> Vec<Dlc> {
    let root = game_dir.join("dlc");
    let mut files = vec![];
    for p in sorted_entries(&root) {
        if p.is_dir() {
            files.extend(sorted_entries(&p).into_iter().filter(|f| f.is_file()));
        } else {
            files.push(p);
        }
    }
    let mut out: Vec<Dlc> = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "dlc"))
        .filter_map(|f| {
            let rel = f.strip_prefix(game_dir).ok()?;
            let id = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let f = parse_pdx(&read_text(f)?);
            // Metadata without its payload archive is not an installed DLC.
            if scalar(&f, "archive").is_some_and(|a| !game_dir.join(a).exists()) {
                return None;
            }
            let name = scalar(&f, "name");
            Some(Dlc {
                affects_checksum: scalar(&f, "affects_checksum").is_some_and(|v| v == "yes"),
                category: scalar(&f, "category"),
                thumbnail: scalar(&f, "thumbnail"),
                name: name.unwrap_or_else(|| id.clone()),
                id,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Verify that every relative path component from `root` to `path` is not a symlink,
/// and return the canonicalized path only if it stays strictly inside canonical `root`.
fn check_contained(root: &Path, path: &Path, want_dir: bool) -> Option<PathBuf> {
    let rel = path.strip_prefix(root).ok()?;
    let mut cur = root.to_path_buf();
    let mut saw_normal = false;
    for comp in rel.components() {
        match comp {
            Component::Normal(seg) => {
                saw_normal = true;
                cur.push(seg);
                let meta = fs::symlink_metadata(&cur).ok()?;
                if meta.file_type().is_symlink() {
                    return None;
                }
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    if !saw_normal {
        return None;
    }
    let meta = fs::symlink_metadata(&cur).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    if want_dir {
        if !meta.file_type().is_dir() {
            return None;
        }
    } else if !meta.file_type().is_file() {
        return None;
    }
    let c_root = fs::canonicalize(root).ok()?;
    let c_path = fs::canonicalize(&cur).ok()?;
    if !c_path.starts_with(&c_root) || c_path == c_root {
        return None;
    }
    Some(c_path)
}

/// Canonicalized regular file strictly inside `root`, rejecting symlinks along the relative path.
pub fn contained_file(root: &Path, path: &Path) -> Option<PathBuf> {
    check_contained(root, path, false)
}

/// Canonicalized directory strictly inside `root`, rejecting symlinks along the relative path.
pub fn contained_dir(root: &Path, path: &Path) -> Option<PathBuf> {
    check_contained(root, path, true)
}

/// Read a contained regular file at open time, verifying no symlink or swap occurred and capping size.
pub fn read_contained_file(root: &Path, path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let c_path = contained_file(root, path)?;
    let pre = fs::symlink_metadata(path).ok()?;
    if pre.file_type().is_symlink() || !pre.file_type().is_file() || pre.len() > max_bytes {
        return None;
    }
    let f = fs::File::open(&c_path).ok()?;
    let opened = f.metadata().ok()?;
    if !opened.is_file() || opened.len() > max_bytes {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (pre.dev(), pre.ino()) != (opened.dev(), opened.ino()) {
            return None;
        }
        let post = fs::symlink_metadata(path).ok()?;
        if post.file_type().is_symlink() || (post.dev(), post.ino()) != (opened.dev(), opened.ino())
        {
            return None;
        }
    }
    let mut buf = Vec::with_capacity((opened.len() as usize).min(1 << 20));
    f.take(max_bytes + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > max_bytes {
        return None;
    }
    Some(buf)
}

/// Resolve a plain file name directly inside `dir` (rejects separators, `..`, `.`, symlinks, and non-files).
pub fn plain_child(dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let mut comps = Path::new(name).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => {
            let p = dir.join(name);
            contained_file(dir, &p)?;
            Some(p)
        }
        _ => None,
    }
}

/// Mod thumbnail path inside the mod directory: descriptor `picture=` when that file exists,
/// else `thumbnail.{png,jpg,jpeg}`. Only plain file names; always None for archive-backed mods.
pub fn picture(m: &Mod) -> Option<PathBuf> {
    let dir = m.root.as_ref().filter(|r| r.is_dir())?;
    m.picture
        .as_deref()
        .into_iter()
        .chain(["thumbnail.png", "thumbnail.jpg", "thumbnail.jpeg"])
        .find_map(|name| plain_child(dir, name))
}

/// Path to `../../appworkshop_<app>.acf` for a Steam workshop content dir (`.../workshop/content/<app>`).
pub fn workshop_acf_path(workshop: &Path) -> Option<PathBuf> {
    let app = workshop.file_name()?.to_str()?;
    let dir = workshop.parent()?.parent()?;
    Some(dir.join(format!("appworkshop_{app}.acf")))
}

/// Parse `WorkshopItemsInstalled` entries from a Steam `appworkshop_<app>.acf` file into
/// `workshop_id -> (size_bytes, timeupdated_secs)`.
pub fn parse_workshop_meta(text: &str) -> HashMap<String, (u64, u64)> {
    let toks: Vec<Tok> = tokens(text).0.into_iter().map(|t| t.0).collect();
    let mut out: HashMap<String, (u64, u64)> = HashMap::new();
    let mut stack: Vec<String> = vec![];
    let mut i = 0;
    while i < toks.len() {
        match (&toks[i], toks.get(i + 1)) {
            (Tok::Str(k) | Tok::Atom(k), Some(Tok::Open)) => {
                stack.push(k.clone());
                i += 2;
            }
            (Tok::Open, _) => {
                stack.push(String::new());
                i += 1;
            }
            (Tok::Close, _) => {
                stack.pop();
                i += 1;
            }
            (Tok::Str(k) | Tok::Atom(k), Some(Tok::Str(v) | Tok::Atom(v))) => {
                if stack.len() >= 2
                    && stack[stack.len() - 2].eq_ignore_ascii_case("WorkshopItemsInstalled")
                {
                    let item = &stack[stack.len() - 1];
                    if !item.is_empty() {
                        let entry = out.entry(item.clone()).or_insert((0, 0));
                        if k.eq_ignore_ascii_case("size") {
                            if let Ok(n) = v.parse::<u64>() {
                                entry.0 = n;
                            }
                        } else if k.eq_ignore_ascii_case("timeupdated")
                            && let Ok(n) = v.parse::<u64>()
                        {
                            entry.1 = n;
                        }
                    }
                }
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

/// Workshop `(size, timeupdated)` per workshop id from `../../appworkshop_<app>.acf`.
pub fn workshop_meta(workshop: &Path) -> HashMap<String, (u64, u64)> {
    let Some(p) = workshop_acf_path(workshop) else {
        return HashMap::new();
    };
    let Some(text) = read_text(&p) else {
        return HashMap::new();
    };
    parse_workshop_meta(&text)
}

fn walk_size(dir: &Path, depth: u8) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_file() {
            if let Ok(m) = e.metadata() {
                total = total.saturating_add(m.len());
            }
        } else if ft.is_dir() && depth < 32 {
            total = total.saturating_add(walk_size(&e.path(), depth + 1));
        }
    }
    total
}

/// Total size in bytes of a mod root (directory tree or single archive file).
pub fn disk_size(root: &Path) -> u64 {
    match fs::metadata(root) {
        Ok(m) if m.is_file() => m.len(),
        Ok(m) if m.is_dir() => walk_size(root, 0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{put, tmp_dir};

    const DESC: &str = "name=\"Planetary Shields\"\nversion=\"4.6.*\"\nsupported_version=\"v4.6.*\"\npath=\"/nonexistent/x\"\nremote_file_id=\"1623423360\"\n# note\ntags={\n\t\"Gameplay\"\n\t\"Balance\"\n}\ndependencies={\n\t\"UI Overhaul Dynamic\"\n}\n";

    #[test]
    fn parse_real_descriptor() {
        let f = parse_pdx(DESC);
        assert_eq!(scalar(&f, "name").unwrap(), "Planetary Shields");
        assert_eq!(scalar(&f, "supported_version").unwrap(), "v4.6.*");
        assert_eq!(list(&f, "tags"), ["Gameplay", "Balance"]);
        assert_eq!(list(&f, "dependencies"), ["UI Overhaul Dynamic"]);
        let f = parse_pdx("\u{feff}name = \"A\" # c\nflag = yes\nx = { a { b } c }");
        assert_eq!(scalar(&f, "name").unwrap(), "A");
        assert_eq!(scalar(&f, "flag").unwrap(), "yes");
        assert_eq!(list(&f, "x"), ["a", "c"]);
    }

    #[test]
    fn token_spans() {
        let t = "\u{feff}a = \"b c\" # x\n{ é=\"q\\\"r\" }";
        let (toks, open) = tokens(t);
        assert!(!open);
        let slices: Vec<&str> = toks.iter().map(|(_, r)| &t[r.clone()]).collect();
        assert_eq!(
            slices,
            ["a", "=", "\"b c\"", "{", "é", "=", "\"q\\\"r\"", "}"]
        );
        assert_eq!(toks[6].0, Tok::Str("q\"r".into()));
        let (toks, open) = tokens("k = \"never closed\nx");
        assert!(open && toks.len() == 3);
        assert_eq!(tokens("").0.len(), 0);
    }

    #[test]
    fn new_descriptor_fields() {
        let d =
            "name=\"M\"\nreplace_path=\"history/a\"\nreplace_path=\"gfx\"\nremote_file_id=\"77\"";
        let m = mod_from("mod/ugc_5.mod".into(), d, None, None);
        assert_eq!(
            (m.remote_id.as_deref(), m.replace_paths.as_slice()),
            (
                Some("77"),
                &["history/a".to_string(), "gfx".to_string()][..]
            )
        );
        assert_eq!(
            mod_from("mod/ugc_5.mod".into(), "name=\"x\"", None, None)
                .remote_id
                .as_deref(),
            Some("5")
        );
        assert_eq!(
            mod_from("mod/local.mod".into(), "name=\"x\"", None, None).remote_id,
            None
        );
    }

    #[test]
    fn vdf_two_libraries_and_escapes() {
        let t = r#""libraryfolders" { "0" { "path" "/a/Steam" "apps" { "281990" "5" } }
            "1" { "path" "D:\\Games\\Steam Library" } }"#;
        assert_eq!(
            vdf_paths(t),
            [
                PathBuf::from("/a/Steam"),
                PathBuf::from(r"D:\Games\Steam Library")
            ]
        );
    }

    #[test]
    fn version_table() {
        for (s, g, want) in [
            ("v4.6.*", "v4.6.0", true),
            ("4.6.*", "v4.6.3", true),
            ("v4.5.*", "v4.6.0", false),
            ("v4.*", "v4.6.0", true),
            ("v4.6", "v4.6.2", true),
            ("v4.6.1", "v4.6.0", false),
            ("*", "v4.6.0", true),
            ("v3.14.*", "v4.6.0", false),
        ] {
            assert_eq!(version_matches(s, g), want, "{s} vs {g}");
        }
        let m = mod_from("mod/a.mod".into(), DESC, None, None);
        assert!(!m.mismatch(Some("v4.6.0")) && m.mismatch(Some("v4.7.0")) && !m.mismatch(None));
        assert!(!mod_from("mod/b.mod".into(), "name=\"x\"", None, None).mismatch(Some("v9")));
    }

    #[test]
    fn synth_has_one_path_line() {
        let s = synth_descriptor(
            "name=\"X\"\npath=\"/old\"\narchive=\"a.zip\"",
            Path::new(r"C:\ws\1"),
        );
        assert_eq!(s.matches("path=").count(), 1);
        assert!(
            s.contains("path=\"C:/ws/1\"") && !s.contains("archive") && s.contains("name=\"X\"")
        );
    }

    #[test]
    fn scan_workshop_docs_stale_and_dedupe() {
        let b = tmp_dir("scan");
        let (data, ws) = (b.join("data"), b.join("ws"));
        // workshop-only (no docs descriptor) -> needs_descriptor
        put(
            &ws.join("100/descriptor.mod"),
            "name=\"WorkshopOnly\"\nsupported_version=\"v4.6.*\"",
        );
        // workshop + docs descriptor pointing at the workshop dir -> not rewritten, listed once
        put(&ws.join("200/descriptor.mod"), "name=\"Both\"");
        put(
            &data.join("mod/ugc_200.mod"),
            &format!(
                "name=\"Both stale name\"\npath=\"{}\"",
                ws.join("200").display()
            ),
        );
        // docs descriptor pointing elsewhere -> workshop wins, rewritten
        put(&ws.join("300/descriptor.mod"), "name=\"Moved\"");
        put(
            &data.join("mod/ugc_300.mod"),
            "name=\"Moved\"\npath=\"/gone/300\"",
        );
        // local mod, relative path
        std::fs::create_dir_all(data.join("mod/local_dir")).unwrap();
        put(
            &data.join("mod/local.mod"),
            "name=\"Local\"\npath=\"mod/local_dir\"",
        );
        // archive= mod
        put(&data.join("mod/zip.zip"), "z");
        put(
            &data.join("mod/zip.mod"),
            "name=\"Zip\"\narchive=\"mod/zip.zip\"",
        );
        // stale: target missing
        put(
            &data.join("mod/stale.mod"),
            "name=\"Stale\"\npath=\"/does/not/exist\"",
        );
        put(&data.join("mod/readme.txt"), "x");

        let mods = scan_mods(&data, Some(&ws));
        let get = |id: &str| mods.iter().find(|m| m.id == id);
        assert_eq!(mods.len(), 5);
        assert!(get("mod/ugc_100.mod").unwrap().needs_descriptor.is_some());
        assert_eq!(get("mod/ugc_200.mod").unwrap().name, "Both");
        assert!(get("mod/ugc_200.mod").unwrap().needs_descriptor.is_none());
        assert!(get("mod/ugc_300.mod").unwrap().needs_descriptor.is_some());
        assert!(get("mod/local.mod").is_some() && get("mod/zip.mod").is_some());
        assert!(get("mod/stale.mod").is_none());
        assert_eq!(get("mod/ugc_100.mod").unwrap().root, Some(ws.join("100")));
        assert_eq!(get("mod/ugc_200.mod").unwrap().root, Some(ws.join("200")));
        assert_eq!(
            get("mod/local.mod").unwrap().root,
            Some(data.join("mod/local_dir"))
        );
        assert_eq!(
            get("mod/zip.mod").unwrap().root,
            Some(data.join("mod/zip.zip"))
        );
        assert_eq!(scan_mods(&b.join("nothing"), None).len(), 0);
    }

    #[test]
    fn dlcs_depth_one_and_two() {
        let g = tmp_dir("dlc");
        put(
            &g.join("dlc/dlc014_utopia/dlc014.dlc"),
            "name = \"Utopia\"\narchive = \"dlc/dlc014_utopia/dlc014.zip\"\naffects_checksum = yes",
        );
        put(&g.join("dlc/flat.dlc"), "name = \"Flat\"");
        put(&g.join("dlc/dlc014_utopia/readme.txt"), "x");
        put(&g.join("dlc/dlc014_utopia/dlc014.zip"), "z");
        put(
            &g.join("dlc/dlc015_x/dlc015.dlc"),
            "name = \"Gone\"\narchive = \"dlc/dlc015_x/dlc015.zip\"",
        );
        let d = scan_dlcs(&g);
        let ids: Vec<_> = d.iter().map(|d| (d.id.as_str(), d.name.as_str())).collect();
        assert_eq!(
            ids,
            [
                ("dlc/flat.dlc", "Flat"),
                ("dlc/dlc014_utopia/dlc014.dlc", "Utopia")
            ]
        );
        assert_eq!(
            d.iter().map(|d| d.affects_checksum).collect::<Vec<_>>(),
            [false, true]
        );
        assert!(scan_dlcs(&g.join("none")).is_empty());
    }

    #[test]
    fn dlc_category_thumbnail_picture_and_workshop_acf() {
        let b = tmp_dir("meta");
        put(
            &b.join("game/dlc/dlc014_utopia/dlc014.dlc"),
            "name = \"Utopia\"\narchive = \"dlc/dlc014_utopia/dlc014.zip\"\ncategory = \"expansion\"\nthumbnail = \"thumb.png\"\n",
        );
        put(&b.join("game/dlc/dlc014_utopia/dlc014.zip"), "z");
        let dlcs = scan_dlcs(&b.join("game"));
        assert_eq!(dlcs.len(), 1);
        assert_eq!(dlcs[0].category.as_deref(), Some("expansion"));
        assert_eq!(dlcs[0].thumbnail.as_deref(), Some("thumb.png"));

        // workshop_meta parses real appworkshop_<app>.acf shape and ignores WorkshopItemDetails
        let ws = b.join("steam/steamapps/workshop/content/281990");
        std::fs::create_dir_all(&ws).unwrap();
        let acf = r#""AppWorkshop"
{
	"appid"		"281990"
	"SizeOnDisk"		"237895615"
	"WorkshopItemsInstalled"
	{
		"1623423360"
		{
			"size"		"236163907"
			"timeupdated"		"1795567077"
			"manifest"		"5459455486775665037"
		}
		"3090328185"
		{
			"size"		"1093428"
			"timeupdated"		"1782072748"
		}
	}
	"WorkshopItemDetails"
	{
		"1623423360"
		{
			"size"		"999"
			"timeupdated"		"1"
		}
		"999999"
		{
			"timeupdated"		"2"
		}
	}
}"#;
        put(
            &b.join("steam/steamapps/workshop/appworkshop_281990.acf"),
            acf,
        );
        let meta = workshop_meta(&ws);
        assert_eq!(meta.len(), 2);
        assert_eq!(meta.get("1623423360"), Some(&(236_163_907, 1_795_567_077)));
        assert_eq!(meta.get("3090328185"), Some(&(1_093_428, 1_782_072_748)));
        assert!(!meta.contains_key("999999"));

        // picture(): descriptor picture=, thumbnail.* fallback, never leaves the dir, None for archive
        let mdir = b.join("mod_dir");
        put(&mdir.join("thumbnail.png"), "png");
        put(&mdir.join("custom.jpg"), "jpg");
        put(&b.join("escape.png"), "secret");
        let base_mod = Mod {
            id: "mod/a.mod".into(),
            root: Some(mdir.clone()),
            ..Default::default()
        };
        assert_eq!(picture(&base_mod), Some(mdir.join("thumbnail.png")));
        assert_eq!(
            picture(&Mod {
                picture: Some("custom.jpg".into()),
                ..base_mod.clone()
            }),
            Some(mdir.join("custom.jpg"))
        );
        // declared but missing, traversal, and nested names all fall back to thumbnail.png
        for bad in ["Impregnate Spouse.jpg", "../escape.png", "sub/a.png", ""] {
            assert_eq!(
                picture(&Mod {
                    picture: Some(bad.into()),
                    ..base_mod.clone()
                }),
                Some(mdir.join("thumbnail.png")),
                "{bad}"
            );
        }
        let jpeg_only = b.join("jpeg_mod");
        put(&jpeg_only.join("thumbnail.jpeg"), "j");
        assert_eq!(
            picture(&Mod {
                root: Some(jpeg_only.clone()),
                ..base_mod.clone()
            }),
            Some(jpeg_only.join("thumbnail.jpeg"))
        );
        put(&jpeg_only.join("thumbnail.jpg"), "j");
        assert_eq!(
            picture(&Mod {
                root: Some(jpeg_only.clone()),
                ..base_mod.clone()
            }),
            Some(jpeg_only.join("thumbnail.jpg"))
        );
        let zip_file = b.join("archive.zip");
        put(&zip_file, "12345");
        assert_eq!(
            picture(&Mod {
                root: Some(zip_file.clone()),
                ..base_mod
            }),
            None
        );

        // disk_size for dir and single file
        assert_eq!(disk_size(&zip_file), 5);
        assert_eq!(disk_size(&mdir), 6);
        assert_eq!(disk_size(&b.join("nope")), 0);
    }

    #[test]
    fn workshop_descriptor_pointing_at_missing_archive_is_rebuilt() {
        let b = tmp_dir("ws-missing-arch");
        let (data, ws) = (b.join("data"), b.join("ws"));
        put(&ws.join("400/descriptor.mod"), "name=\"Archived\"");
        put(
            &data.join("mod/ugc_400.mod"),
            &format!(
                "name=\"Archived\"\narchive=\"{}\"",
                ws.join("400/deleted.zip").display()
            ),
        );
        let mods = scan_mods(&data, Some(&ws));
        let m = mods.iter().find(|m| m.id == "mod/ugc_400.mod").unwrap();
        assert!(
            m.needs_descriptor.is_some(),
            "missing archive inside workshop dir must force descriptor rewrite"
        );
        assert_eq!(m.root, Some(ws.join("400")));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_containment_rejects_links_and_swaps() {
        use std::os::unix::fs::symlink;
        let b = tmp_dir("symlink-mod");
        let root = b.join("mod_root");
        let outside = b.join("secret.png");
        put(&outside, "TOP_SECRET");
        put(&root.join("ok.png"), "OK");
        symlink(&outside, root.join("link.png")).unwrap();

        assert_eq!(plain_child(&root, "ok.png"), Some(root.join("ok.png")));
        assert_eq!(plain_child(&root, "link.png"), None);
        assert_eq!(
            read_contained_file(&root, &root.join("link.png"), 1024),
            None
        );
        assert_eq!(
            read_contained_file(&root, &root.join("ok.png"), 1024).as_deref(),
            Some(b"OK".as_slice())
        );

        // Symlinked subdirectory escaping root is also rejected.
        let ext_dir = b.join("ext_dir");
        put(&ext_dir.join("pic.png"), "EXT");
        symlink(&ext_dir, root.join("assets")).unwrap();
        assert_eq!(contained_dir(&root, &root.join("assets")), None);
        assert_eq!(contained_file(&root, &root.join("assets/pic.png")), None);
    }
}
