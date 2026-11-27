//! Mod file conflicts, suggested load order and checksum report. Pure; reruns on every playset change.
use crate::index::FileIndex;
use crate::mods::Mod;
use crate::playset::{Entry, Playset};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

pub struct Conflict {
    /// Game-relative path.
    pub path: String,
    /// Enabled owners in load order; the last one wins.
    pub mods: Vec<String>,
}

/// Files shipped by two or more enabled mods, sorted by path. `order` is the enabled mod ids in
/// load order.
pub fn conflicts(ix: &FileIndex, order: &[String]) -> Vec<Conflict> {
    let pos: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut out: Vec<Conflict> = ix
        .by_path
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .filter_map(|(key, ids)| {
            let mut mods: Vec<String> = ids
                .iter()
                .filter(|id| pos.contains_key(id.as_str()))
                .cloned()
                .collect();
            mods.dedup();
            if mods.len() < 2 {
                return None;
            }
            mods.sort_by_key(|id| pos[id.as_str()]);
            // Show the original case; fall back to the lowercase key.
            let path = ix
                .mods
                .get(&mods[0])
                .and_then(|f| f.paths.iter().find(|p| crate::index::norm(p) == *key))
                .unwrap_or(key)
                .clone();
            Some(Conflict { path, mods })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// "`winner` overrides `loser`" for `paths`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub winner: String,
    pub loser: String,
    pub paths: Vec<String>,
}

/// Group conflicts by (winner, loser): every non-last owner of a path loses it to the last one.
/// Biggest pairs first.
pub fn pairs(found: &[Conflict]) -> Vec<Pair> {
    let mut by: HashMap<(&str, &str), Vec<String>> = HashMap::new();
    for c in found {
        let Some((win, losers)) = c.mods.split_last() else {
            continue;
        };
        for l in losers {
            by.entry((win, l)).or_default().push(c.path.clone());
        }
    }
    let mut out: Vec<Pair> = by
        .into_iter()
        .map(|((w, l), paths)| Pair {
            winner: w.into(),
            loser: l.into(),
            paths,
        })
        .collect();
    out.sort_by(|a, b| {
        (b.paths.len(), &a.winner, &a.loser).cmp(&(a.paths.len(), &b.winner, &b.loser))
    });
    out
}

/// Strongly connected components (Kosaraju) over `0..n` given adjacency `deps` (`a -> d`).
/// Each component's members are sorted ascending (preserving original slot order).
fn sccs(n: usize, deps: &[Vec<usize>]) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut rev = vec![vec![]; n];
    for (u, ds) in deps.iter().enumerate() {
        for &v in ds {
            rev[v].push(u);
        }
    }
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for start in 0..n {
        if seen[start] {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        seen[start] = true;
        while let Some((u, idx)) = stack.last_mut() {
            let u = *u;
            if *idx < deps[u].len() {
                let v = deps[u][*idx];
                *idx += 1;
                if !seen[v] {
                    seen[v] = true;
                    stack.push((v, 0));
                }
            } else {
                stack.pop();
                order.push(u);
            }
        }
    }
    let mut comp = vec![usize::MAX; n];
    let mut groups: Vec<Vec<usize>> = vec![];
    for &start in order.iter().rev() {
        if comp[start] != usize::MAX {
            continue;
        }
        let cid = groups.len();
        let mut members = vec![];
        let mut stack = vec![start];
        comp[start] = cid;
        while let Some(u) = stack.pop() {
            members.push(u);
            for &v in &rev[u] {
                if comp[v] == usize::MAX {
                    comp[v] = cid;
                    stack.push(v);
                }
            }
        }
        members.sort_unstable();
        groups.push(members);
    }
    // Sort groups by their smallest original slot index and renumber `comp` accordingly.
    groups.sort_by_key(|g| g[0]);
    for (cid, g) in groups.iter().enumerate() {
        for &u in g {
            comp[u] = cid;
        }
    }
    (groups, comp)
}

/// Stable topological sort of the enabled mods on their declared dependencies (a dependency loads
/// before its dependent). Only enabled slots are permuted; disabled entries stay put. Strongly
/// connected dependency cycles are sorted as atomic units preserving their internal order.
/// Returns the new entries and one reason per change (or cycle note).
pub fn suggest(p: &Playset, mods: &[Mod]) -> (Vec<Entry>, Vec<String>) {
    let by_id: HashMap<&str, &Mod> = mods.iter().map(|m| (m.id.as_str(), m)).collect();
    let slots: Vec<usize> = (0..p.mods.len())
        .filter(|&i| p.mods[i].enabled && by_id.contains_key(p.mods[i].id.as_str()))
        .collect();
    let name = |s: usize| by_id[p.mods[slots[s]].id.as_str()].name.as_str();
    // deps[a] = sequence numbers (into `slots`) that must load before a.
    let deps: Vec<Vec<usize>> = (0..slots.len())
        .map(|a| {
            let want = &by_id[p.mods[slots[a]].id.as_str()].dependencies;
            (0..slots.len())
                .filter(|&b| b != a && want.iter().any(|d| d == name(b)))
                .collect()
        })
        .collect();
    let (groups, comp) = sccs(slots.len(), &deps);
    let mut reasons = vec![];
    for g in &groups {
        if g.len() > 1 {
            let mut noted = false;
            for &a in g {
                for &d in &deps[a] {
                    if comp[d] == comp[a] && a < d {
                        reasons.push(format!(
                            "{} and {} depend on each other; order kept",
                            name(a),
                            name(d)
                        ));
                        noted = true;
                    }
                }
            }
            if !noted {
                let list: Vec<&str> = g.iter().map(|&u| name(u)).collect();
                reasons.push(format!(
                    "{} depend on each other; order kept",
                    list.join(", ")
                ));
            }
        }
    }
    // Condensation DAG: component c depends on component cd (cd != c) if any member does.
    let mut comp_deps: Vec<Vec<usize>> = vec![vec![]; groups.len()];
    for (c, members) in groups.iter().enumerate() {
        for &u in members {
            for &d in &deps[u] {
                let cd = comp[d];
                if cd != c && !comp_deps[c].contains(&cd) {
                    comp_deps[c].push(cd);
                }
            }
        }
    }
    // Stable topological sort on components (ready component with smallest original slot first).
    let mut placed = vec![false; groups.len()];
    let mut comp_seq: Vec<usize> = Vec::with_capacity(groups.len());
    while let Some(c) =
        (0..groups.len()).find(|&c| !placed[c] && comp_deps[c].iter().all(|&cd| placed[cd]))
    {
        placed[c] = true;
        comp_seq.push(c);
    }
    let mut seq: Vec<usize> = Vec::with_capacity(slots.len());
    for c in comp_seq {
        seq.extend(&groups[c]);
    }
    for &a in &seq {
        for &d in &deps[a] {
            if comp[d] != comp[a] && d > a {
                reasons.push(format!(
                    "{} moved after its dependency {}",
                    name(a),
                    name(d)
                ));
            }
        }
    }
    let mut entries = p.mods.clone();
    for (i, &a) in seq.iter().enumerate() {
        entries[slots[i]] = p.mods[slots[a]].clone();
    }
    (entries, reasons)
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checksum {
    /// Mod ids.
    pub changing: Vec<String>,
    pub clean: Vec<String>,
    /// None = unknown.
    pub achievements: Option<bool>,
    /// Why `achievements` is unknown; None when it is known.
    pub unknown_reason: Option<UnknownReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownReason {
    /// checksum_manifest.txt is missing.
    NoManifest,
    /// The manifest exists but an enabled mod could not be fully indexed.
    Incomplete,
}

/// Which enabled mods change the game checksum. Official DLC never turns achievements off, even
/// with `affects_checksum = yes` (every CK3 DLC): that flag only changes the multiplayer checksum. Without checksum_manifest.txt nothing can
/// be judged: `achievements` is None and no mod is called clean. Mods that failed to index are
/// never called clean and leave `achievements` as None unless another mod/DLC already breaks it.
pub fn checksum_report(ix: &FileIndex, p: &Playset, mods: &[Mod]) -> Checksum {
    let known: HashSet<&str> = mods.iter().map(|m| m.id.as_str()).collect();
    let mut c = Checksum::default();
    if !ix.has_manifest {
        c.unknown_reason = Some(UnknownReason::NoManifest);
        return c;
    }
    let mut incomplete = false;
    for e in p
        .mods
        .iter()
        .filter(|e| e.enabled && known.contains(e.id.as_str()))
    {
        match ix.mods.get(&e.id) {
            Some(f) if f.checksum => c.changing.push(e.id.clone()),
            Some(f) if f.complete => c.clean.push(e.id.clone()),
            _ => incomplete = true,
        }
    }
    c.achievements = if !c.changing.is_empty() {
        Some(false)
    } else if incomplete {
        c.unknown_reason = Some(UnknownReason::Incomplete);
        None
    } else {
        Some(true)
    };
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{put, tmp_dir};
    use crate::index::{Manifest, build};

    fn md(id: &str, name: &str, root: &std::path::Path, deps: &[&str]) -> Mod {
        Mod {
            id: id.into(),
            name: name.into(),
            root: Some(root.join(id)),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn pset(spec: &[(&str, bool)]) -> Playset {
        Playset {
            name: "P".into(),
            mods: spec
                .iter()
                .map(|(id, on)| Entry {
                    id: id.to_string(),
                    enabled: *on,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn ids(es: &[Entry]) -> Vec<&str> {
        es.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn conflicts_last_wins_disabled_excluded_and_pairs() {
        let b = tmp_dir("cf");
        for m in ["a", "b", "c"] {
            put(&b.join(m).join("common/x/Shared.txt"), "x");
        }
        put(&b.join("a/gfx/only_ab.dds"), "x");
        put(&b.join("b/GFX/Only_AB.dds"), "x");
        put(&b.join("c/localisation/own.yml"), "x");
        let mods = [
            md("a", "A", &b, &[]),
            md("b", "B", &b, &[]),
            md("c", "C", &b, &[]),
        ];
        let ix = build(&mods, None);
        let order = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let f = conflicts(&ix, &order(&["c", "a", "b"]));
        assert_eq!(f.len(), 2);
        let shared = f.iter().find(|c| c.path == "common/x/Shared.txt").unwrap();
        assert_eq!(shared.mods, ["c", "a", "b"]);
        let pr = pairs(&f);
        let got: Vec<_> = pr
            .iter()
            .map(|p| (p.winner.as_str(), p.loser.as_str(), p.paths.len()))
            .collect();
        assert_eq!(got, [("b", "a", 2), ("b", "c", 1)]);
        // c disabled (absent from the order): only a/b conflict, b wins.
        let f = conflicts(&ix, &order(&["b", "a"]));
        assert_eq!(f.len(), 2);
        assert!(f.iter().all(|c| c.mods == ["b", "a"]));
        assert!(conflicts(&ix, &order(&["a"])).is_empty());
    }

    #[test]
    fn suggest_stable_topo_enabled_slots_only() {
        let m = [
            md("a", "A", &std::path::PathBuf::new(), &["C"]),
            md("b", "B", &std::path::PathBuf::new(), &[]),
            md("c", "C", &std::path::PathBuf::new(), &[]),
            md("d", "D", &std::path::PathBuf::new(), &["A"]),
        ];
        // b disabled between a and c; d disabled-free.
        let p = pset(&[("a", true), ("b", false), ("c", true), ("d", true)]);
        let (e, why) = suggest(&p, &m);
        assert_eq!(ids(&e), ["c", "b", "a", "d"]);
        assert!(!e[1].enabled && e[0].enabled);
        assert_eq!(why, ["A moved after its dependency C"]);
        // Already fine: unchanged, no reasons.
        let (e, why) = suggest(&pset(&[("c", true), ("a", true), ("d", true)]), &m);
        assert_eq!(ids(&e), ["c", "a", "d"]);
        assert!(why.is_empty());
    }

    #[test]
    fn suggest_keeps_order_on_cycle() {
        let r = std::path::PathBuf::new();
        let m = [
            md("a", "A", &r, &["B"]),
            md("b", "B", &r, &["A"]),
            md("c", "C", &r, &[]),
        ];
        let (e, why) = suggest(&pset(&[("a", true), ("b", true), ("c", true)]), &m);
        assert_eq!(ids(&e), ["a", "b", "c"]);
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("depend on each other"), "{why:?}");
    }

    #[test]
    fn checksum_manifest_none_and_dlc() {
        let b = tmp_dir("cs");
        put(
            &b.join("g/checksum_manifest.txt"),
            "directory\nname = common\nsub_directories = yes\n",
        );
        put(&b.join("a/common/x.txt"), "x");
        put(&b.join("b/gfx/x.dds"), "x");
        let mods = [md("a", "A", &b, &[]), md("b", "B", &b, &[])];
        let mf = Manifest::load(&b.join("g"));
        let ix = build(&mods, mf.as_ref());
        let p = pset(&[("a", true), ("b", true)]);
        let c = checksum_report(&ix, &p, &mods);
        assert_eq!((c.changing, c.clean), (vec!["a".into()], vec!["b".into()]));
        assert_eq!(c.achievements, Some(false));
        let p = pset(&[("a", false), ("b", true)]);
        let c = checksum_report(&ix, &p, &mods);
        assert_eq!(c.achievements, Some(true));
        // No manifest: unknown, nobody is clean.
        let c = checksum_report(&build(&mods, None), &p, &mods);
        assert!(c.achievements.is_none() && c.clean.is_empty() && c.changing.is_empty());
        assert_eq!(c.unknown_reason, Some(UnknownReason::NoManifest));
    }

    #[test]
    fn suggest_cycle_with_external_dep_preserves_internal_order() {
        let r = std::path::PathBuf::new();
        // A <-> B cycle, and A also depends on C (initially [A, B, C]).
        // Must move C before the {A, B} unit -> [C, A, B], never reversing A and B to [B, C, A].
        let m = [
            md("a", "A", &r, &["B", "C"]),
            md("b", "B", &r, &["A"]),
            md("c", "C", &r, &[]),
        ];
        let p = pset(&[("a", true), ("b", true), ("c", true)]);
        let (e, why) = suggest(&p, &m);
        assert_eq!(ids(&e), ["c", "a", "b"]);
        assert!(why.iter().any(|w| w.contains("depend on each other")));
        assert!(
            why.iter()
                .any(|w| w.contains("moved after its dependency C"))
        );

        // Applying suggest again to [C, A, B] leaves order completely unchanged.
        let p2 = Playset {
            name: "P".into(),
            mods: e,
            ..Default::default()
        };
        let (e2, _) = suggest(&p2, &m);
        assert_eq!(ids(&e2), ["c", "a", "b"]);
    }

    #[test]
    fn checksum_incomplete_mod_not_marked_clean() {
        let b = tmp_dir("cs-incomplete");
        put(
            &b.join("g/checksum_manifest.txt"),
            "directory\nname = common\nsub_directories = yes\n",
        );
        put(&b.join("ok/gfx/x.dds"), "x");
        put(&b.join("corrupt.zip"), "not a zip");
        put(&b.join("chg/common/x.txt"), "x");
        let mods = [
            md("ok", "OK", &b, &[]),
            Mod {
                id: "bad".into(),
                name: "Bad".into(),
                root: Some(b.join("corrupt.zip")),
                ..Default::default()
            },
            md("chg", "Chg", &b, &[]),
        ];
        let ix = build(&mods, Manifest::load(&b.join("g")).as_ref());
        let p = pset(&[("ok", true), ("bad", true)]);
        let c = checksum_report(&ix, &p, &mods);
        assert_eq!(c.clean, ["ok"]);
        assert!(c.changing.is_empty());
        assert_eq!(
            c.achievements, None,
            "incomplete mod makes achievements unknown"
        );
        assert_eq!(
            c.unknown_reason,
            Some(UnknownReason::Incomplete),
            "manifest exists: the reason is the index, not a missing manifest"
        );
        assert_eq!(
            serde_json::to_value(&c).unwrap()["unknownReason"],
            "incomplete"
        );

        // If another enabled mod is known to change checksum, achievements is definitively false.
        let p2 = pset(&[("ok", true), ("bad", true), ("chg", true)]);
        let c2 = checksum_report(&ix, &p2, &mods);
        assert_eq!(c2.achievements, Some(false));
    }
}
