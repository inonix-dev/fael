//! L2 path aliases — the seam between the log and where files live now.
//! Pure data: core never runs git or touches the disk. The binary fills this
//! from `git log -M` (`.fael/cache/aliases.json`) and from `fael mv` rows,
//! and a future cloud sync can fill it from its own history.
//!
//! A rename only *adds* matches, so a wrong pair pushes one row too many and
//! never hides one. Swapped names (`a↔b`) therefore push both sides.

use crate::{Log, anchor};
use std::collections::HashSet;

/// Old → new path pairs (`(old, new)`), deduped, insertion order.
#[derive(Debug, Default, Clone)]
pub struct Aliases {
    pairs: Vec<(String, String)>,
}

impl Aliases {
    /// Build from rename pairs; empties, self-pairs and duplicates fall away.
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Aliases {
        let mut a = Aliases::default();
        a.merge_pairs(&pairs);
        a
    }

    /// `fael mv` aliases live in the log itself (the source of truth — never
    /// the cache). A moved row carries `moved: {from, to}` and no kind/files.
    pub fn from_log(log: &Log) -> Aliases {
        let mut a = Aliases::default();
        for r in &log.rows {
            let got = r
                .extra
                .get("moved")
                .and_then(|m| Some((m.get("from")?.as_str()?, m.get("to")?.as_str()?)));
            if let Some((from, to)) = got
                && !from.is_empty()
                && !to.is_empty()
                && from != to
            {
                a.merge_pairs(&[(from.to_string(), to.to_string())]);
            }
        }
        a
    }

    /// Add pairs, skipping empties, self-pairs and ones already held.
    pub fn merge_pairs(&mut self, pairs: &[(String, String)]) {
        for (o, n) in pairs {
            if !o.is_empty()
                && !n.is_empty()
                && o != n
                && !self.pairs.contains(&(o.clone(), n.clone()))
            {
                self.pairs.push((o.clone(), n.clone()));
            }
        }
    }

    /// Add another set's pairs.
    pub fn merge(&mut self, other: &Aliases) {
        self.merge_pairs(&other.pairs);
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Query path → itself plus every past path that became it, following
    /// chains (`a→b→c` queried at `c` gives `[c, b, a]`). Directory queries
    /// map across a renamed directory the same way. Anchors match exactly
    /// only (a ref is opaque, `/` in it is not a directory), and globs pass
    /// through untouched — `find` matches those itself.
    pub fn expand(&self, q: &str) -> Vec<String> {
        if self.pairs.is_empty() {
            return vec![q.to_string()];
        }
        let q = q.trim_end_matches('/');
        if q.contains(['*', '?', '[']) {
            return vec![q.to_string()];
        }
        let anchored = anchor(q).is_some();
        let mut seen = HashSet::new();
        let mut out = vec![];
        let mut stack = vec![q.to_string()];
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            out.push(cur.clone());
            for (o, n) in &self.pairs {
                if cur == *n {
                    stack.push(o.clone());
                } else if anchored {
                    // exact only — see above
                } else if under(&cur, n) {
                    // the query sits inside a renamed directory
                    stack.push(format!("{o}{}", &cur[n.len()..]));
                } else if let Some(dir) = under_parent(&cur, n, o) {
                    // the query IS a directory above the new path
                    stack.push(dir);
                }
            }
        }
        out
    }

    /// `expand` over many queries, order kept, duplicates dropped.
    pub fn expand_all(&self, qs: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = vec![];
        for q in qs {
            for e in self.expand(q) {
                if seen.insert(e.clone()) {
                    out.push(e);
                }
            }
        }
        out
    }

    /// A row's path → itself plus every path it was renamed to, following all
    /// pairs (not just the first match), so a revert `a→b→a` then `a→c`
    /// still reaches `c` whatever order the pairs were cached in.
    pub fn forward(&self, f: &str) -> Vec<String> {
        let anchored = anchor(f).is_some();
        let mut seen = HashSet::new();
        let mut out = vec![];
        let mut stack = vec![f.to_string()];
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            for (o, n) in &self.pairs {
                if cur == *o {
                    stack.push(n.clone());
                } else if !anchored && under(&cur, o) {
                    stack.push(format!("{n}{}", &cur[o.len()..]));
                }
            }
            out.push(cur);
        }
        out
    }

    /// A row's path → where it lives now (`None` = no rename known, or the
    /// renames cycle so there is no single answer). Follows chains to the end.
    pub fn current(&self, f: &str) -> Option<String> {
        let anchored = anchor(f).is_some();
        let mut cur = f.to_string();
        let mut visited = HashSet::from([cur.clone()]);
        loop {
            let mut next = None;
            for (o, n) in &self.pairs {
                if cur == *o {
                    next = Some(n.clone());
                    break;
                }
                if !anchored && under(&cur, o) {
                    next = Some(format!("{n}{}", &cur[o.len()..]));
                    break;
                }
            }
            match next {
                None => return if cur == f { None } else { Some(cur) },
                Some(nx) => {
                    if !visited.insert(nx.clone()) {
                        return None; // a↔b: no single answer
                    }
                    cur = nx;
                }
            }
        }
    }
}

/// `path` is strictly inside directory `dir` (`dir/x`, never `dir` itself and
/// never `dir2/x` — the `/` boundary matters).
fn under(path: &str, dir: &str) -> bool {
    path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/'
}

/// The query is a parent directory of the new path: derive the matching old
/// directory, but only when the old file ends with the same rest (a pure
/// directory move — a file that also changed name says nothing about its
/// neighbours). `cur = "src/new"`, `(o, n) = ("src/old/x.rs", "src/new/x.rs")`
/// → `Some("src/old")`.
fn under_parent(cur: &str, n: &str, o: &str) -> Option<String> {
    if !under(n, cur) {
        return None;
    }
    let rest = &n[cur.len()..]; // starts with '/'
    o.strip_suffix(rest).map(String::from)
}

/// A moved row (`moved`, no kind) is an alias carrier, never a result —
/// readers that don't know it must skip it (format.md §Readers).
pub fn is_alias_row(r: &crate::Row) -> bool {
    r.extra.contains_key("moved")
}
