//! Which files each mod ships, and which of those change the game checksum. UI-free; built once
//! per scan (on a background thread in the GUI) and shared by conflicts, checksum, crash and share.
use crate::mods::{Mod, Value, parse_pdx, read_text};
use std::collections::HashMap;
use std::path::Path;

struct Rule {
    dir: String,
    sub: bool,
    ext: Option<String>,
}

/// The game's checksum_manifest.txt: directories (and extensions) that count for the checksum.
pub struct Manifest(Vec<Rule>);

/// Lowercase, `\` -> `/`: the key under which paths are compared.
pub fn norm(rel: &str) -> String {
    rel.replace('\\', "/").to_lowercase()
}

impl Manifest {
    /// None when the file is missing or lists no directory.
    pub fn load(game_dir: &Path) -> Option<Manifest> {
        let f = parse_pdx(&read_text(&game_dir.join("checksum_manifest.txt"))?);
        // The bare `directory` words are skipped by parse_pdx, so each `name` starts a rule.
        let mut rules: Vec<Rule> = vec![];
        for (k, v) in &f {
            let Value::Scalar(v) = v else { continue };
            match k.as_str() {
                "name" => rules.push(Rule {
                    dir: norm(v).trim_matches('/').to_owned(),
                    sub: false,
                    ext: None,
                }),
                "sub_directories" => rules.last_mut()?.sub = v == "yes",
                "file_extension" => rules.last_mut()?.ext = Some(v.to_lowercase()),
                _ => {}
            }
        }
        (!rules.is_empty()).then_some(Manifest(rules))
    }

    /// Does a file at this game-relative path count for the checksum?
    pub fn matches(&self, rel: &str) -> bool {
        let rel = norm(rel);
        self.0.iter().any(|r| {
            let Some(rest) = rel.strip_prefix(&r.dir).and_then(|x| x.strip_prefix('/')) else {
                return false;
            };
            (r.sub || !rest.contains('/')) && r.ext.as_ref().is_none_or(|e| rel.ends_with(e))
        })
    }

    /// Does replacing this whole directory affect a manifest directory?
    fn hits_dir(&self, rp: &str) -> bool {
        let rp = norm(rp);
        let rp = rp.trim_matches('/');
        self.0.iter().any(|r| {
            r.dir == rp
                || r.dir.starts_with(&format!("{rp}/"))
                || r.sub && rp.strip_prefix(&r.dir).is_some_and(|x| x.starts_with('/'))
        })
    }
}

pub struct ModFiles {
    /// Original case, `/` separators, sorted; only files under a subdirectory (no descriptor.mod, thumbnail).
    pub paths: Vec<String>,
    /// Any path matches the manifest, or a replace_path hits a manifest directory.
    pub checksum: bool,
    /// False if the mod root was missing, unreadable, hit depth limit, or failed to parse as a zip.
    pub complete: bool,
}

pub struct FileIndex {
    pub mods: HashMap<String, ModFiles>,
    /// `norm(path)` -> mod ids, in the order the mods were given.
    pub by_path: HashMap<String, Vec<String>>,
    /// False when checksum_manifest.txt was missing: `checksum` flags are then all false, not "clean".
    pub has_manifest: bool,
}

fn walk(dir: &Path, prefix: &str, depth: u8, out: &mut Vec<String>) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut complete = true;
    for entry in rd {
        let Ok(e) = entry else {
            complete = false;
            continue;
        };
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{}", name)
        };
        let Ok(ft) = e.file_type() else {
            complete = false;
            continue;
        };
        let is_dir = if ft.is_symlink() {
            match std::fs::metadata(e.path()) {
                Ok(m) => m.is_dir(),
                Err(_) => {
                    complete = false;
                    continue;
                }
            }
        } else {
            ft.is_dir()
        };
        if is_dir {
            if depth < 32 {
                complete &= walk(&e.path(), &rel, depth + 1, out);
            } else {
                complete = false;
            }
        } else if rel.contains('/') {
            out.push(rel);
        }
    }
    complete
}

fn files_of(m: &Mod) -> (Vec<String>, bool) {
    let mut out = vec![];
    let complete = match &m.root {
        Some(r) if r.is_dir() => walk(r, "", 0, &mut out),
        Some(r) => match std::fs::File::open(r)
            .ok()
            .and_then(|f| crate::zip::entries(&mut std::io::BufReader::new(f)).ok())
        {
            Some(es) => {
                out.extend(es.into_iter().map(|e| e.name).filter(|n| {
                    !n.ends_with('/')
                        && n.contains('/')
                        && !n.rsplit('/').next().unwrap_or("").starts_with('.')
                }));
                true
            }
            None => false,
        },
        None => false,
    };
    out.sort();
    (out, complete)
}

pub fn build(mods: &[Mod], manifest: Option<&Manifest>) -> FileIndex {
    let (mut ix, mut by_path) = (HashMap::new(), HashMap::<String, Vec<String>>::new());
    for m in mods {
        let (paths, complete) = files_of(m);
        for p in &paths {
            by_path.entry(norm(p)).or_default().push(m.id.clone());
        }
        let checksum = manifest.is_some_and(|mf| {
            paths.iter().any(|p| mf.matches(p)) || m.replace_paths.iter().any(|r| mf.hits_dir(r))
        });
        ix.insert(
            m.id.clone(),
            ModFiles {
                paths,
                checksum,
                complete,
            },
        );
    }
    FileIndex {
        mods: ix,
        by_path,
        has_manifest: manifest.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{put, tmp_dir};

    const MANIFEST: &str = "directory \nname = common\nsub_directories = yes\nfile_extension = .txt\n\ndirectory\nname = events\nsub_directories = yes\n\ndirectory\nname = map\nfile_extension = .shader\n";

    #[test]
    fn manifest_rules() {
        let g = tmp_dir("manifest");
        assert!(Manifest::load(&g).is_none());
        put(&g.join("checksum_manifest.txt"), MANIFEST);
        let m = Manifest::load(&g).unwrap();
        for (p, want) in [
            ("common/a/b.txt", true),
            ("Common\\a\\B.TXT", true),
            ("common/a.png", false),
            ("events/x/y.anything", true),
            ("map/a.shader", true),
            ("map/sub/a.shader", false),
            ("localisation/a.yml", false),
            ("commonx/a.txt", false),
        ] {
            assert_eq!(m.matches(p), want, "{p}");
        }
        assert!(m.hits_dir("common/buildings") && m.hits_dir("common"));
        assert!(!m.hits_dir("gfx") && !m.hits_dir("map/sub"));
    }

    #[test]
    fn build_dirs_zip_and_checksum() {
        let b = tmp_dir("index");
        put(&b.join("m1/descriptor.mod"), "name=\"x\"");
        put(&b.join("m1/common/Foo/a.txt"), "x");
        put(&b.join("m1/gfx/pic.dds"), "x");
        put(&b.join("m1/common/.DS_Store"), "x");
        put(&b.join("m2/common/foo/A.txt"), "x");
        put(&b.join("m3/localisation/en.yml"), "x");
        put(&b.join("m4/readme.txt"), "x");
        // zip root
        let zip = {
            let mut z = vec![];
            let name = b"gfx/z.dds";
            z.extend(b"PK\x03\x04");
            for v in [20u16, 0, 0, 0, 0] {
                z.extend(v.to_le_bytes());
            }
            for v in [0u32, 0, 0] {
                z.extend(v.to_le_bytes());
            }
            z.extend((name.len() as u16).to_le_bytes());
            z.extend(0u16.to_le_bytes());
            z.extend(name);
            let off = z.len() as u32;
            z.extend(b"PK\x01\x02");
            for v in [20u16, 20, 0, 0, 0, 0] {
                z.extend(v.to_le_bytes());
            }
            for v in [0u32, 0, 0] {
                z.extend(v.to_le_bytes());
            }
            z.extend((name.len() as u16).to_le_bytes());
            z.extend([0u8; 8]);
            z.extend(0u32.to_le_bytes());
            z.extend(0u32.to_le_bytes());
            z.extend(name);
            let n = z.len() as u32 - off;
            z.extend(b"PK\x05\x06\0\0\0\0\x01\0\x01\0");
            z.extend(n.to_le_bytes());
            z.extend(off.to_le_bytes());
            z.extend(0u16.to_le_bytes());
            z
        };
        std::fs::write(b.join("m5.zip"), zip).unwrap();
        let md = |id: &str, root: &str, rp: &[&str]| Mod {
            id: id.into(),
            root: Some(b.join(root)),
            replace_paths: rp.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let mods = [
            md("m1", "m1", &[]),
            md("m2", "m2", &[]),
            md("m3", "m3", &[]),
            md("m4", "m4", &[]),
            md("m5", "m5.zip", &[]),
            md("m6", "nowhere", &["events"]),
        ];
        put(&b.join("g/checksum_manifest.txt"), MANIFEST);
        let ix = build(&mods, Manifest::load(&b.join("g")).as_ref());
        assert!(ix.has_manifest);
        assert_eq!(ix.mods["m1"].paths, ["common/Foo/a.txt", "gfx/pic.dds"]);
        assert_eq!(ix.by_path["common/foo/a.txt"], ["m1", "m2"]);
        assert_eq!(ix.mods["m5"].paths, ["gfx/z.dds"]);
        assert!(ix.mods["m4"].paths.is_empty());
        let sums: Vec<_> = ["m1", "m2", "m3", "m4", "m5", "m6"]
            .map(|m| ix.mods[m].checksum)
            .into();
        assert_eq!(sums, [true, true, false, false, false, true]);
        let ix = build(&mods, None);
        assert!(!ix.has_manifest && ix.mods.values().all(|m| !m.checksum));
    }

    #[test]
    fn corrupt_zip_or_missing_dir_marked_incomplete() {
        let b = tmp_dir("index-incomplete");
        put(&b.join("g/checksum_manifest.txt"), MANIFEST);
        put(&b.join("good/gfx/x.dds"), "x");
        put(&b.join("bad.zip"), "not a valid zip archive");
        let mods = [
            Mod {
                id: "good".into(),
                root: Some(b.join("good")),
                ..Default::default()
            },
            Mod {
                id: "badzip".into(),
                root: Some(b.join("bad.zip")),
                ..Default::default()
            },
            Mod {
                id: "missing".into(),
                root: Some(b.join("does_not_exist")),
                ..Default::default()
            },
        ];
        let ix = build(&mods, Manifest::load(&b.join("g")).as_ref());
        assert!(ix.mods["good"].complete);
        assert!(!ix.mods["badzip"].complete);
        assert!(!ix.mods["missing"].complete);
    }
}
