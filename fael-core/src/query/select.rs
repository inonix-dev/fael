use super::Filter;
use super::matching::{file_match, glob, lenient, same_dir, zone};
use crate::{Aliases, Log, Row, anchor, is_alias_row};
use std::collections::HashSet;
use std::path::Path;

/// Ids of closed rows (a close row, or a compact row's `closed` field).
pub fn closed(log: &Log) -> HashSet<&str> {
    let mut h: HashSet<&str> = log
        .closes
        .iter()
        .filter_map(|c| c.reference.as_deref())
        .collect();
    h.extend(
        log.rows
            .iter()
            .filter(|r| r.extra.contains_key("closed"))
            .map(|r| r.id.as_str()),
    );
    h
}

/// Ids some newer row names in `supersedes`.
pub fn superseded(log: &Log) -> HashSet<&str> {
    log.rows
        .iter()
        .filter_map(|r| r.supersedes.as_deref())
        .collect()
}

/// Rows matching `f`, newest first by id. Closed and superseded rows are hidden unless `f.all`.
pub fn find<'a>(log: &'a Log, f: &Filter) -> Vec<&'a Row> {
    let hide: HashSet<&str> = if f.all {
        HashSet::new()
    } else {
        closed(log).union(&superseded(log)).copied().collect()
    };
    let text = f.text.as_ref().map(|t| t.to_lowercase());
    let files: Vec<String> = f
        .files
        .iter()
        .map(|q| lenient(q).trim_end_matches('/').to_string())
        .collect();
    let mut out: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| {
            !hide.contains(r.id.as_str())
                && !is_alias_row(r)
                && f.kind.as_ref().is_none_or(|k| &r.kind == k)
                && f.by.as_ref().is_none_or(|b| &r.by == b)
                && f.since.as_ref().is_none_or(|s| r.ts.as_str() >= s.as_str())
                && f.key
                    .as_ref()
                    .is_none_or(|g| r.key.as_deref().is_some_and(|k| glob(g, k)))
                && text
                    .as_ref()
                    .is_none_or(|t| r.text.to_lowercase().contains(t))
                && (files.is_empty()
                    || r.files.iter().any(|rf| {
                        let rf = lenient(rf);
                        files.iter().any(|q| file_match(q, &rf))
                    }))
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// Every file the row names is a path that no longer exists under `root`,
/// even through `al` (a rename only *adds* a present path, so a wrong pair
/// keeps one row too many and never hides one). Anchors never go. A row with
/// no files is never gone. Such rows never push, so kickoff drops them too.
pub fn gone(root: &Path, r: &Row, al: &Aliases) -> bool {
    !r.files.is_empty()
        && r.files
            .iter()
            .all(|f| anchor(f).is_none() && al.forward(f).iter().all(|p| !root.join(p).exists()))
}

/// The session brief (kickoff, and `find` with no filter): open issues, then decisions, then
/// notes, then repo kinds — newest first inside each.
pub fn brief<'a>(log: &'a Log, f: &Filter) -> Vec<&'a Row> {
    let mut rows = find(log, f);
    // stable sort keeps newest-first inside each kind
    rows.sort_by_key(|r| match r.kind.as_str() {
        "issue" => 0,
        "decision" => 1,
        "note" => 2,
        _ => 3,
    });
    rows
}

/// What a session opens with (`fael kickoff`, the session-start hook): the brief minus
/// rows whose files are gone, open issues first, then everything else by how fresh it is —
/// the newer of the row itself and the last change to any of its files. So an old decision
/// about a file nobody touches sinks, and one about the file changed yesterday rises.
// ponytail: file mtime is the "current work" signal — no git spawn on session start; a fresh
// clone or checkout resets mtimes, then the order falls back to roughly newest-row first.
pub fn kickoff<'a>(log: &'a Log, f: &Filter, root: &Path, al: &Aliases) -> Vec<&'a Row> {
    let fresh = |r: &Row| {
        let row_ms = crate::ts_ms(&r.ts).unwrap_or(0);
        r.files
            .iter()
            // freshness follows the rename: the old path is gone, the new one
            // is what the worktree last touched
            .flat_map(|f| al.forward(f))
            .filter_map(|f| {
                std::fs::metadata(root.join(f))
                    .and_then(|m| m.modified())
                    .ok()
            })
            .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .fold(row_ms, i64::max)
    };
    let mut rows: Vec<(&Row, i64)> = find(log, f)
        .into_iter()
        .filter(|r| !gone(root, r, al))
        .map(|r| (r, fresh(r)))
        .collect();
    // find() is newest-id first, and the stable sort keeps that for ties
    rows.sort_by_key(|(r, ms)| (r.kind != "issue", std::cmp::Reverse(*ms)));
    rows.into_iter().map(|(r, _)| r).collect()
}

/// The read/edit push: rows about `files`, ranked so the most actionable comes
/// first — exact file, then same directory, then rows sharing a key with an
/// exact hit. Open `issue` before `decision` before the rest, newest first by
/// `id` inside each. Closed and superseded rows never push. Each query expands
/// through `al` first, so a row filed under a path that was renamed since
/// still pushes at the new path. Deterministic: the
/// same log and query give the same order on any machine. The caller cuts the
/// result to the push budget with `render`.
pub fn push<'a>(log: &'a Log, files: &[String], al: &Aliases) -> Vec<&'a Row> {
    let hide: HashSet<&str> = closed(log).union(&superseded(log)).copied().collect();
    let queries: Vec<String> = al.expand_all(
        &files
            .iter()
            .map(|q| lenient(q).trim_end_matches('/').to_string())
            .collect::<Vec<_>>(),
    );
    if queries.is_empty() {
        return vec![];
    }
    // keys of the exact hits — tier 2 shares one of these
    let mut hit_keys: HashSet<&str> = HashSet::new();
    for r in &log.rows {
        if hide.contains(r.id.as_str()) {
            continue;
        }
        let rf: Vec<String> = r.files.iter().map(|f| lenient(f)).collect();
        if queries.iter().any(|q| rf.iter().any(|f| zone(q, f))) {
            hit_keys.extend(r.key.as_deref());
        }
    }
    let tier = |r: &Row| {
        let rf: Vec<String> = r.files.iter().map(|f| lenient(f)).collect();
        if queries.iter().any(|q| rf.iter().any(|f| zone(q, f))) {
            return 0;
        }
        if queries.iter().any(|q| rf.iter().any(|f| same_dir(q, f))) {
            return 1;
        }
        if r.key.as_deref().is_some_and(|k| hit_keys.contains(k)) {
            return 2;
        }
        3
    };
    let mut out: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| !hide.contains(r.id.as_str()) && !is_alias_row(r) && tier(r) < 3)
        .collect();
    // newest first, then stable sort keeps it inside each rank
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out.sort_by_key(|r| {
        (
            tier(r),
            match r.kind.as_str() {
                "issue" => 0,
                "decision" => 1,
                _ => 2,
            },
        )
    });
    out
}
