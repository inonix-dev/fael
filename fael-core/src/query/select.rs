use super::Filter;
use super::matching::{file_match, glob, lenient, same_dir, zone};
use crate::{Aliases, Log, Row, anchor, is_alias_row, resolve, to_matches};
use std::cmp::Ordering;
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

/// What `add --urgent` asks for: `Unset` = not urgent, `End` = back of the
/// queue, `Before(id)` = just above that row — the midpoint with the row
/// above it, or half the top value when it is first (the queue starts at 1,
/// so halving never crosses zero).
#[derive(Debug, Default, Clone)]
pub enum Urgent {
    #[default]
    Unset,
    End,
    Before(String),
}

/// What `bump` does to the urgent queue: `Keep` leaves it, `Remove` clears it.
#[derive(Debug, Clone)]
pub enum UrgentChange {
    Keep,
    End,
    Before(String),
    Remove,
}

/// Open issues carrying an urgent number, most urgent first (number asc, id
/// desc breaks ties). Closed and superseded rows left the queue when they
/// left every list.
fn urgent_queue(log: &Log) -> Vec<(f64, &Row)> {
    let hide: HashSet<&str> = closed(log).union(&superseded(log)).copied().collect();
    let mut q: Vec<(f64, &Row)> = log
        .rows
        .iter()
        .filter(|r| !hide.contains(r.id.as_str()) && r.kind == "issue")
        .filter_map(|r| r.urgent_value().map(|u| (u, r)))
        .collect();
    q.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.id.cmp(&a.1.id)));
    q
}

/// The number an `--urgent` / `--urgent-before` ask resolves to. A move never
/// renumbers the other rows — only the moved row is rewritten (through
/// `bump`), so the append-only log stays append-only. The f64 ceiling
/// (~50 midpoints in one gap) needs no rebalance write: ranking still breaks
/// an exact tie by id desc, deterministically.
pub fn resolve_urgent(log: &Log, opt: &Urgent) -> Result<Option<f64>, String> {
    let q = urgent_queue(log);
    match opt {
        Urgent::Unset => Ok(None),
        Urgent::End => Ok(Some(q.last().map_or(1.0, |(u, _)| u + 1.0))),
        Urgent::Before(id) => {
            let t = resolve(log, id)?;
            if t.kind != "issue" {
                return Err(format!(
                    "rejected: the urgent queue holds issues — {id:?} is {}",
                    t.kind
                ));
            }
            let tu = t.urgent_value().ok_or_else(|| {
                format!("rejected: --urgent-before needs an urgent row — {id:?} has no number")
            })?;
            let pos = q.iter().position(|(_, r)| r.id == t.id).ok_or_else(|| {
                format!(
                    "rejected: --urgent-before needs an open row — {id:?} is closed or superseded"
                )
            })?;
            // the nearest number strictly above — a tie with the row above has
            // no gap, so skip past it rather than falling to `tu - 1.0`
            let above = q[..pos]
                .iter()
                .rev()
                .map(|(u, _)| *u)
                .find(|u| *u < tu)
                .unwrap_or(0.0);
            // the queue starts at 1, so halving the top never crosses zero; a
            // hand-written non-positive top falls back to one step above it
            Ok(Some(if above < tu {
                (above + tu) / 2.0
            } else {
                tu - 1.0
            }))
        }
    }
}

/// Freshness from the row alone — callers with a worktree (kickoff) pass the
/// mtime-aware closure instead.
pub fn fresh_ts(r: &Row) -> i64 {
    crate::ts_ms(&r.ts).unwrap_or(0)
}

fn kind_rank(r: &Row) -> u8 {
    match r.kind.as_str() {
        "issue" => 0,
        "decision" => 1,
        "note" => 2,
        _ => 3,
    }
}

/// One ordering for every list (chunk 3): to=reader, urgent, match tier,
/// kind, freshness, id. Importance only comes from explicit signals someone
/// set (`to`, `urgent`) or structure (file match, kind), never from text.
/// `tier` is the push match (exact file > same dir > shared key); every other
/// list passes 0. Deterministic: the same log and reader give the same order
/// on any machine.
pub fn cmp_rows(
    a: &Row,
    b: &Row,
    reader: Option<&str>,
    tier_a: usize,
    tier_b: usize,
    fresh_a: i64,
    fresh_b: i64,
) -> Ordering {
    let mine = |r: &Row| reader.is_some_and(|w| r.to_who().is_some_and(|t| to_matches(t, w)));
    match (mine(a), mine(b)) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    match (a.urgent_value(), b.urgent_value()) {
        (Some(x), Some(y)) => {
            let ord = x.total_cmp(&y);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        (Some(_), None) => return Ordering::Less,
        (None, Some(_)) => return Ordering::Greater,
        (None, None) => {}
    }
    match tier_a.cmp(&tier_b) {
        Ordering::Equal => {}
        ord => return ord,
    }
    match kind_rank(a).cmp(&kind_rank(b)) {
        Ordering::Equal => {}
        ord => return ord,
    }
    match fresh_b.cmp(&fresh_a) {
        Ordering::Equal => {}
        ord => return ord,
    }
    b.id.cmp(&a.id)
}

/// Sort rows under `cmp_rows`, computing each row's tier and freshness once.
/// Push passes its match tier; kickoff its mtime freshness; everyone else the
/// zero tier and `fresh_ts`.
pub fn ranked<'a>(
    rows: Vec<&'a Row>,
    reader: Option<&str>,
    tier: impl Fn(&Row) -> usize,
    fresh: impl Fn(&Row) -> i64,
) -> Vec<&'a Row> {
    let mut keyed: Vec<(&Row, usize, i64)> =
        rows.into_iter().map(|r| (r, tier(r), fresh(r))).collect();
    keyed.sort_by(|a, b| cmp_rows(a.0, b.0, reader, a.1, b.1, a.2, b.2));
    keyed.into_iter().map(|(r, _, _)| r).collect()
}

/// Postgres-style paging over a ranked list: skip `offset`, take `limit`.
/// Returns the page plus the pre-page total, so the cut line can count down
/// from it. Applied by `query()` (find/brief) and the kickoff CLI — never by
/// `find()`/`kickoff()` themselves, so kickoff keeps ranking the full set and
/// push/session-start (whose filters carry no paging) are untouched.
pub fn page(rows: Vec<&Row>, limit: Option<usize>, offset: usize) -> (Vec<&Row>, usize) {
    let total = rows.len();
    let page: Vec<&Row> = rows
        .into_iter()
        .skip(offset)
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    (page, total)
}

/// Rows matching `f`, ranked most actionable first. Closed and superseded
/// rows are hidden unless `f.all`.
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
    let out: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| {
            !hide.contains(r.id.as_str())
                && !is_alias_row(r)
                && f.kind.as_ref().is_none_or(|k| &r.kind == k)
                && f.by.as_ref().is_none_or(|b| &r.by == b)
                && f.to
                    .as_ref()
                    .is_none_or(|t| r.to_who().is_some_and(|w| w == t))
                && f.since.as_ref().is_none_or(|s| r.ts.as_str() >= s.as_str())
                && f.key
                    .as_ref()
                    .is_none_or(|g| r.key.as_deref().is_some_and(|k| glob(g, k)))
                && text.as_ref().is_none_or(|t| {
                    r.text.to_lowercase().contains(t)
                        // lists show titles, so text search finds them too
                        || r.title.as_deref().is_some_and(|ti| ti.to_lowercase().contains(t))
                })
                && (files.is_empty()
                    || r.files.iter().any(|rf| {
                        let rf = lenient(rf);
                        files.iter().any(|q| file_match(q, &rf))
                    }))
        })
        .collect();
    // no reader and no file match here — urgent, kind, freshness, id decide
    ranked(out, None, |_| 0, fresh_ts)
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

/// The session brief (kickoff, and `find` with no filter): the unfiltered
/// find, which already ranks urgent first, then issues, decisions, notes,
/// repo kinds.
pub fn brief<'a>(log: &'a Log, f: &Filter) -> Vec<&'a Row> {
    find(log, f)
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
    let rows: Vec<&Row> = find(log, f)
        .into_iter()
        .filter(|r| !gone(root, r, al))
        .collect();
    // find() is ranked already, and the stable rank keeps that for ties
    ranked(rows, None, |_| 0, fresh)
}

/// The read/edit push: rows about `files`, ranked so the most actionable comes
/// first — urgent, then exact file, same directory, rows sharing a key with an
/// exact hit. Open `issue` before `decision` before the rest, freshest first
/// by row-or-mtime inside each. Closed and superseded rows never push. Each
/// query expands through `al` first, so a row filed under a path that was
/// renamed since still pushes at the new path. The read/edit path never
/// computes reader identity (no git spawn there), so `to` does not reorder
/// the push — session start is where routing lists. Deterministic: the
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
    let out: Vec<&Row> = log
        .rows
        .iter()
        .filter(|r| !hide.contains(r.id.as_str()) && !is_alias_row(r) && tier(r) < 3)
        .collect();
    ranked(out, None, tier, fresh_ts)
}
