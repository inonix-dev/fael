use super::{Kind, Problem, Report};
use crate::log::{collect_files, is_marker, month_of};
use crate::{MONTH_MAX, Row, is_alias_row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
                // close-shaped and alias-carrier rows carry no `files` by design — only adds count
                if row.files.is_empty()
                    && row.reference.as_deref().is_none_or(|t| t.trim().is_empty())
                    && !is_alias_row(&row)
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
