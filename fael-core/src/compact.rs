//! `fael compact` — fold past months into immutable per-writer files (SPEC §6).
//!
//! Only files nobody appends to any more are rewritten: `<writer>/<yyyy-mm>`
//! month files older than the current month (and older than `--before`).
//! Never the current month, never `compact.*` or `_import/*` (immutable).
//! Each close folds into its row as `"closed":{"id","ts","by","text"}` — the
//! one place a row carries its own status — rows sort by id, sources are
//! deleted after the rewrite lands. `--prune` additionally drops rows that
//! are closed and whose files are all gone from the worktree.

use crate::log::{collect_files, dedupe_ids, lock, month_of, parse, tmp_rename};
use crate::{Row, anchor, ulid};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Opts {
    pub writer: Option<String>,
    pub before: Option<String>,
    /// Drop rows that are closed **and** whose files are all gone.
    pub prune: bool,
}

#[derive(Debug, Default)]
pub struct WriterReport {
    pub writer: String,
    pub rows: usize,
    pub folded: usize,
    pub pruned: usize,
    /// Closes whose target wasn't in the compacted months — carried forward
    /// untouched so nothing is lost.
    pub carried: usize,
    pub deleted: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub writers: Vec<WriterReport>,
}

/// Rewrite eligible months. `month` is the current UTC `yyyy-mm` (injected so
/// tests don't depend on the clock); `root` is only read for `--prune`
/// existence checks. Errors when a source file has lines `read` would skip —
/// run `fael doctor --fix` first, so no byte is ever dropped silently.
pub fn compact(fael: &Path, root: &Path, opts: &Opts, month: &str) -> Result<Report, String> {
    let _guard = lock(fael)?;
    let log = fael.join("log");
    // writer → eligible month files (sorted); writers filtered by --writer
    let mut by_writer: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for f in collect_files(&log) {
        if f.extension().is_none_or(|x| x != "jsonl") {
            continue;
        }
        let Some(m) = month_of(&f) else { continue };
        if m.as_str() >= month {
            continue; // the current month (and any clock-skew future one) stays append-only
        }
        if opts.before.as_ref().is_some_and(|b| m.as_str() >= b.as_str()) {
            continue;
        }
        let rel = f.strip_prefix(&log).map_err(|e| e.to_string())?;
        let mut parts = rel.components();
        let (Some(w), Some(_)) = (parts.next(), parts.next()) else { continue };
        if parts.next().is_some() {
            continue; // only direct children of a writer folder
        }
        let writer = w.as_os_str().to_string_lossy().into_owned();
        if writer.starts_with('_') || writer.starts_with('.') {
            continue;
        }
        if opts.writer.as_ref().is_some_and(|w| *w != writer) {
            continue;
        }
        by_writer.entry(writer).or_default().push(f);
    }
    if by_writer.is_empty() {
        return Err("fael: no past months to compact — month files older than the current one".into());
    }
    let mut writers: Vec<String> = by_writer.keys().cloned().collect();
    writers.sort();
    let mut report = Report::default();
    for writer in writers {
        let files = &by_writer[&writer];
        let (mut rows, mut closes) = load(files)?;
        dedupe_ids(&mut rows);
        dedupe_ids(&mut closes);
        let folded = fold(&mut rows, &closes);
        // closes that resolved are gone; the rest ride along untouched
        let resolved: HashSet<&str> = folded.iter().map(|(_, c)| c.id.as_str()).collect();
        let carried: Vec<Row> = closes.into_iter().filter(|c| !resolved.contains(c.id.as_str())).collect();
        let pruned = if opts.prune { prune(&mut rows, root) } else { 0 };
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let stamp = ulid();
        let dir = log.join(&writer);
        let mut body = String::new();
        for r in &rows {
            body.push_str(&r.to_line());
            body.push('\n');
        }
        let out = dir.join(format!("compact.{stamp}.jsonl"));
        tmp_rename(&out, body.as_bytes())?;
        let carried_n = carried.len();
        if !carried.is_empty() {
            let mut cbody = String::new();
            for c in &carried {
                cbody.push_str(&c.to_line());
                cbody.push('\n');
            }
            tmp_rename(&dir.join(format!("compact.{stamp}.close.jsonl")), cbody.as_bytes())?;
        }
        let mut deleted = vec![];
        for f in files {
            std::fs::remove_file(f).map_err(|e| format!("{}: {e}", f.display()))?;
            deleted.push(f.strip_prefix(fael).unwrap_or(f).display().to_string());
        }
        report.writers.push(WriterReport {
            writer,
            rows: rows.len(),
            folded: folded.len(),
            pruned,
            carried: carried_n,
            deleted,
        });
    }
    Ok(report)
}

/// Parse every source through the same reader `find` uses. A warning means a
/// line `read` would skip — compact refuses to drop it silently.
fn load(files: &[PathBuf]) -> Result<(Vec<Row>, Vec<Row>), String> {
    let (mut rows, mut closes) = (vec![], vec![]);
    let mut warnings = vec![];
    for f in files {
        let name = f.to_string_lossy().into_owned();
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", name))?;
        let w0 = warnings.len();
        if name.ends_with(".close.jsonl") {
            parse(&bytes, &name, &mut closes, &mut warnings);
        } else {
            parse(&bytes, &name, &mut rows, &mut warnings);
        }
        if warnings.len() > w0 {
            return Err(format!(
                "{name}: {} line(s) `read` would skip ({}) — run `fael doctor --fix` first, compact never drops a byte",
                warnings.len() - w0,
                warnings[w0]
            ));
        }
    }
    Ok((rows, closes))
}

/// Fold each close into its row; returns the (row id, close) pairs that
/// resolved — shared with `import`, which folds legacy fapony closes the
/// same way. A close that names no row, or names one already closed, is
/// carried forward instead.
pub(crate) fn fold(rows: &mut [Row], closes: &[Row]) -> Vec<(String, Row)> {
    let mut done = vec![];
    for c in closes {
        let Some(target) = c.reference.as_deref().filter(|t| !t.trim().is_empty()) else { continue };
        // exact id first, else a unique prefix like `resolve`
        let mut idx = rows.iter().position(|r| r.id == *target);
        if idx.is_none() && !target.is_empty() {
            let mut pre = rows.iter().enumerate().filter(|(_, r)| {
                r.id.get(..target.len()).is_some_and(|p| p.eq_ignore_ascii_case(target))
            });
            idx = match (pre.next(), pre.next()) {
                (Some((i, _)), None) => Some(i),
                _ => None,
            };
        }
        if let Some(r) = idx.map(|i| &mut rows[i])
            && !r.extra.contains_key("closed")
        {
            r.extra.insert(
                "closed".into(),
                serde_json::json!({"id": c.id, "ts": c.ts, "by": c.by, "text": c.text}),
            );
            done.push((r.id.clone(), c.clone()));
        }
    }
    done
}

/// Drop rows that are closed and whose files are all gone. Anchors are opaque
/// (never filesystem paths) and keep their row; rows without `files` are
/// legacy and are never pruned.
fn prune(rows: &mut Vec<Row>, root: &Path) -> usize {
    let before = rows.len();
    rows.retain(|r| {
        if !r.extra.contains_key("closed") || r.files.is_empty() {
            return true;
        }
        let mut all_gone = true;
        for f in &r.files {
            if anchor(f).is_some() || root.join(f).exists() {
                all_gone = false;
                break;
            }
        }
        !all_gone
    });
    before - rows.len()
}
