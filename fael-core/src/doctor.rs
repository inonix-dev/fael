//! `fael doctor [--fix]` — self-healing per SPEC §11.
//!
//! Reading never fails and `doctor` without `--fix` only reports (exit 1 when
//! an `Error` is present, so CI can gate on it). `--fix` repairs what it can —
//! broken lines and torn tails move to `.fael/quarantine/` (never deleted),
//! conflict markers are stripped, encoding is normalised, the
//! `merge=union` line is added — every repair through tmp + rename under the
//! `.fael/.lock`. What `--fix` cannot repair (duplicates → `compact`; legacy
//! rows without `files` → never invent files) stays report-only.

use crate::log::{collect_files, is_marker, lock, month_of, tmp_rename};
use crate::{MONTH_MAX, Row, now_ms, rfc3339, ulid};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Only `Error` fails `doctor`; `Info` is reported and never fails `--fix`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Broken,
    Torn,
    Conflict,
    Union,
    Duplicate,
    Encoding,
    NoFiles,
    MultiFael,
    Ignored,
    NoLog,
    Future,
    Oversize,
    /// open rows whose files all no longer exist — they never push again
    Gone,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub kind: Kind,
    pub severity: Severity,
    /// true when `--fix` repairs this (everything else is report-only).
    pub fixable: bool,
    /// The log file this is about, when there is one — `--fix` groups
    /// content repairs by it instead of parsing `detail`.
    pub file: Option<PathBuf>,
    pub detail: String,
}

impl Problem {
    fn error(kind: Kind, fixable: bool, file: Option<PathBuf>, detail: String) -> Problem {
        Problem {
            kind,
            severity: Severity::Error,
            fixable,
            file,
            detail,
        }
    }

    fn info(kind: Kind, detail: String) -> Problem {
        Problem {
            kind,
            severity: Severity::Info,
            fixable: false,
            file: None,
            detail,
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
}

impl Report {
    /// Anything that fails `doctor` (exit 1) — and what `--fix` must clear.
    pub fn errors(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
    }
}

/// Scan `.fael/log/**` plus the repo around it. `log_ignored` comes from the
/// adapter (`git check-ignore` — core never spawns git); `month` is the
/// current UTC `yyyy-mm` (injected so tests don't depend on the clock).
pub fn scan(fael: &Path, root: &Path, log_ignored: bool, month: &str) -> Report {
    let mut r = Report::default();
    let log = fael.join("log");
    let files: Vec<PathBuf> = collect_files(&log)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    // The monorepo case is checked even before adoption — it is exactly the
    // fapony lesson (several memory dirs, every hint went silent).
    let others = multi_fael(root);
    if !others.is_empty() {
        r.problems.push(Problem::info(
            Kind::MultiFael,
            format!(
                "{} .fael/ dir(s) under this repo ({}); reads resolve from the touched file upward",
                others.len(),
                others.join(", ")
            ),
        ));
    }
    if files.is_empty() {
        // Like the session-start hook: warning about a missing log is noise.
        // The union line and friends only matter once fael is adopted.
        r.problems.push(Problem::info(
            Kind::NoLog,
            "no .fael/log in this worktree — fael never adopted here, nothing to check".into(),
        ));
        return r;
    }
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut no_files = 0usize;
    let mut no_files_example = String::new();
    for f in &files {
        scan_file(
            f,
            &mut r,
            &mut ids,
            &mut no_files,
            &mut no_files_example,
            month,
        );
    }
    let mut dupes: Vec<(&String, &usize)> = ids.iter().filter(|(_, n)| **n > 1).collect();
    dupes.sort();
    for (id, n) in dupes {
        r.problems.push(Problem::error(
            Kind::Duplicate,
            false,
            None,
            format!("id {id} appears {n}× — read dedupes it, `fael compact` rewrites it away"),
        ));
    }
    if no_files > 0 {
        r.problems.push(Problem::info(
            Kind::NoFiles,
            format!(
                "{no_files} row(s) without `files` (legacy) — read fine, ranked last, never invented; e.g. {no_files_example}"
            ),
        ));
    }
    // the cause of leftover conflict markers: without union every merge conflicts
    let attrs = root.join(".gitattributes");
    let has_union = std::fs::read_to_string(&attrs)
        .map(|s| s.lines().any(|l| l.contains("merge=union")))
        .unwrap_or(false);
    if !has_union {
        r.problems.push(Problem::error(
            Kind::Union,
            true,
            None,
            format!(
                "{} has no `merge=union` line — concurrent appends conflict instead of merging",
                attrs.display()
            ),
        ));
    }
    if log_ignored {
        r.problems.push(Problem::error(
            Kind::Ignored,
            false,
            None,
            ".fael/log is gitignored — rows stay on this machine, `fael doctor` always reports it"
                .into(),
        ));
    }
    r
}

/// One file's share of the scan — line numbers match `log::parse` exactly
/// (1-based over complete lines; the unterminated tail is reported as torn).
fn scan_file(
    path: &Path,
    r: &mut Report,
    ids: &mut HashMap<String, usize>,
    no_files: &mut usize,
    no_files_example: &mut String,
    month: &str,
) {
    let name = path.to_string_lossy().into_owned();
    let file = Some(path.to_path_buf());
    let Ok(bytes) = std::fs::read(path) else {
        r.problems.push(Problem::error(
            Kind::Broken,
            false,
            file,
            format!("{name}: unreadable — skipped on read"),
        ));
        return;
    };
    if bytes.starts_with(b"\xef\xbb\xbf")
        || bytes.windows(2).any(|w| w == b"\r\n")
        || String::from_utf8(bytes.clone()).is_err()
    {
        r.problems.push(Problem::error(
            Kind::Encoding,
            true,
            file.clone(),
            format!("{name}: BOM / CRLF / invalid UTF-8 — normalised in memory on read"),
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut lines: Vec<&str> = text.split('\n').collect();
    let tail = lines.pop().unwrap_or("");
    let mut broken = vec![];
    let mut markers = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if is_marker(line) {
            markers += 1;
            continue;
        }
        match serde_json::from_str::<Row>(line) {
            Ok(row) => {
                if !row.id.is_empty() {
                    *ids.entry(row.id.clone()).or_insert(0) += 1;
                }
                // close-shaped rows carry no `files` by design — only adds count
                if row.files.is_empty()
                    && row.reference.as_deref().is_none_or(|t| t.trim().is_empty())
                {
                    *no_files += 1;
                    if no_files_example.is_empty() {
                        *no_files_example = format!("{}:{}", name, i + 1);
                    }
                }
            }
            Err(_) => broken.push(i + 1),
        }
    }
    if !broken.is_empty() {
        r.problems.push(Problem::error(
            Kind::Broken,
            true,
            file.clone(),
            format!(
                "{name}:{}: {} broken line(s) skipped on read",
                fmt_nums(&broken),
                broken.len()
            ),
        ));
    }
    if markers > 0 {
        r.problems.push(Problem::error(
            Kind::Conflict,
            true,
            file.clone(),
            format!(
                "{name}: {markers} merge-conflict marker line(s) — rows on both sides are kept"
            ),
        ));
    }
    if !tail.trim().is_empty() {
        r.problems.push(Problem::error(
            Kind::Torn,
            true,
            file.clone(),
            format!("{name}: torn last line (no `\\n`) ignored on read, sealed on write"),
        ));
    }
    if let Some(m) = month_of(path)
        && m.as_str() > month
    {
        r.problems.push(Problem::info(
            Kind::Future,
            format!("{name}: month {m} is in the future (clock skew) — read normally"),
        ));
    }
    if path.metadata().map(|m| m.len()).unwrap_or(0) >= MONTH_MAX {
        r.problems.push(Problem::error(
            Kind::Oversize,
            false,
            file,
            format!("{name}: ≥ 50 MiB, appends refuse — run `fael compact`"),
        ));
    }
}

fn fmt_nums(ns: &[usize]) -> String {
    const MAX: usize = 8;
    let mut s = ns
        .iter()
        .take(MAX)
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if ns.len() > MAX {
        s.push_str(&format!(",+{}", ns.len() - MAX));
    }
    s
}

/// Every `.fael` dir under `root` except `root/.fael` itself — the monorepo
/// case. Skips `.git`, `target` and `node_modules` (never project memory).
fn multi_fael(root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".fael" {
                if p != root.join(".fael") {
                    out.push(
                        p.strip_prefix(root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
                continue; // never descend into a log dir
            }
            if [".git", "target", "node_modules"].contains(&name.as_str()) {
                continue;
            }
            stack.push(p);
        }
    }
    out.sort();
    out
}

/// Repair every fixable problem in `report`, under the `.fael/.lock`.
/// Returns one line per action taken. Report-only problems are left alone.
pub fn fix(fael: &Path, root: &Path, report: &Report) -> Result<Vec<String>, String> {
    let _guard = lock(fael)?;

    let mut done = vec![];
    // group content repairs per file so each file is rewritten once
    let mut files: HashSet<PathBuf> = HashSet::new();
    for p in &report.problems {
        match p.kind {
            Kind::Broken | Kind::Torn | Kind::Conflict | Kind::Encoding => {
                if let Some(f) = p.file.clone().filter(|f| f.is_file()) {
                    files.insert(f);
                }
            }
            Kind::Union => {
                let attrs = root.join(".gitattributes");
                let mut s = std::fs::read_to_string(&attrs).unwrap_or_default();
                if !s.lines().any(|l| l.contains("merge=union")) {
                    if !s.is_empty() && !s.ends_with('\n') {
                        s.push('\n');
                    }
                    s.push_str(".fael/log/**/*.jsonl merge=union\n");
                    tmp_rename(&attrs, s.as_bytes())?;
                    done.push(format!("{}: added the merge=union line", attrs.display()));
                }
            }
            _ => {}
        }
    }
    let mut files: Vec<PathBuf> = files.into_iter().collect();
    files.sort();
    for f in files {
        if let Some(action) = repair_file(fael, &f)? {
            done.push(action);
        }
    }
    Ok(done)
}

/// Rewrite one log file without its broken lines and marker lines, with
/// encoding normalised; the removed bytes move to `quarantine/` (the torn
/// tail too). Returns the action line, or None when nothing changed.
fn repair_file(fael: &Path, path: &Path) -> Result<Option<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(&text)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines: Vec<&str> = text.split('\n').collect();
    let tail = lines.pop().unwrap_or("");
    // re-derive what's broken/markers post-normalisation (same rules as scan)
    let mut broken: HashSet<usize> = HashSet::new();
    let mut markers = 0usize;
    let mut kept = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || is_marker(t) {
            if is_marker(t) {
                markers += 1;
            }
            continue;
        }
        if serde_json::from_str::<Row>(line).is_err() {
            broken.insert(i + 1);
        } else {
            kept += 1;
        }
    }
    if broken.is_empty() && markers == 0 && tail.trim().is_empty() && is_clean(&bytes) {
        return Ok(None);
    }
    let mut out = String::new();
    let mut moved = vec![];
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || is_marker(t) {
            continue;
        }
        if broken.contains(&(i + 1)) {
            moved.push(line.to_string());
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !tail.trim().is_empty() {
        moved.push(tail.trim().to_string());
    }
    if !moved.is_empty() {
        let q = quarantine(fael, path)?;
        let mut body = moved.join("\n");
        body.push('\n');
        std::fs::write(&q, body).map_err(|e| format!("{}: {e}", q.display()))?;
    }
    tmp_rename(path, out.as_bytes())?;
    Ok(Some(format!(
        "{}: {} line(s) to quarantine, {} marker(s) stripped, {} row(s) kept",
        path.display(),
        moved.len(),
        markers,
        kept
    )))
}

fn is_clean(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"\xef\xbb\xbf")
        && !bytes.windows(2).any(|w| w == b"\r\n")
        && String::from_utf8(bytes.to_vec()).is_ok()
}

/// `.fael/quarantine/<log.path.with.dots>.<ULID>.jsonl` — removed bytes are
/// never deleted, and quarantine never lives under `log/` so reads ignore it.
fn quarantine(fael: &Path, path: &Path) -> Result<PathBuf, String> {
    let dir = fael.join("quarantine");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let rel = path
        .strip_prefix(fael)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(['/', '\\'], ".");
    let q = dir.join(format!("{rel}.{}.jsonl", ulid()));
    Ok(q)
}

/// The current UTC month — what the CLI passes as `scan`'s `month`.
pub fn current_month() -> String {
    rfc3339(now_ms())[..7].to_string()
}
