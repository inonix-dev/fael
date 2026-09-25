//! Write-side path integrity (PLAN-fael-path-integrity chunk 4): `fael add`
//! without `--files` inherits the files this session edited, and every files
//! entry is checked against evidence before the row is written.
//!
//! Core never sees the disk here (PLAN §4) — this module is the binary side:
//! it reads the hook's session edits and the worktree, and core only gets the
//! resulting list through the normal `add_row` path.

use crate::{core, hook};
use std::collections::HashSet;
use std::path::Path;

/// A session stays usable for deriving files while its edit file was written
/// recently — the plan's guess is 2 h.
const ACTIVE_SECS: u64 = 2 * 60 * 60;

/// Every edit any active session recorded in this worktree, oldest first.
/// Lines without a worktree predate it and are kept (they age out with the
/// 2 h window); lines naming another worktree are dropped — without this a
/// row filed in repo A would inherit files touched in repo B.
pub(crate) fn active_edits(root: &Path) -> Vec<(String, i64)> {
    let dir = hook::state_dir().join("sessions");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let here = root.to_string_lossy();
    let mut files: Vec<(u64, std::path::PathBuf)> = vec![];
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "jsonl") {
            continue; // `.seen`, `.tmp`, …
        }
        let age = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok().map(|d| d.as_secs()));
        // an unreadable mtime fails toward inclusion — an old file only
        // contributes edits newer than the last row anyway
        if age.is_none_or(|s| s < ACTIVE_SECS) {
            files.push((age.unwrap_or(0), p));
        }
    }
    // oldest session file first, so the union reads in edit order
    files.sort();
    let mut out = vec![];
    for (_, p) in files {
        for (path, at, worktree) in hook::session_edits(&p) {
            if worktree.as_deref().is_none_or(|w| w == here) {
                out.push((path, at));
            }
        }
    }
    out
}

/// Files for a row filed now: the active edits newer than the session's
/// newest row, order kept, deduped. Empty = the caller keeps the old
/// "files is required" error, so behaviour without a hook session is unchanged.
pub(crate) fn derive(root: &Path, log: &core::Log) -> Vec<String> {
    let last = core::last_row_ms(log, 0);
    let mut seen = HashSet::new();
    let mut out = vec![];
    for (path, at) in active_edits(root) {
        if last.is_none_or(|r| at > r) && seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

/// Check a files list against evidence before the row is written. Accepted
/// silently, in order: anchor · glob · on disk · rename-resolvable · in this
/// session's edits · in git status (shell-made files the edit hook never saw).
///
/// A path with no evidence is rejected only when it is almost surely a typo:
/// a same-directory file on disk within edit distance 2. Anything else is
/// filed anyway with a warning — rows about deleted or not-yet-created files
/// are legitimate (kickoff and doctor, not the write path, judge those).
pub(crate) fn check(
    root: &Path,
    al: &core::Aliases,
    files: &[String],
    edits: &[(String, i64)],
) -> Result<Vec<String>, String> {
    let in_edits: HashSet<&str> = edits.iter().map(|(p, _)| p.as_str()).collect();
    let mut missing: Vec<&str> = vec![];
    for f in files {
        if hook::is_anchor(f) || is_glob(f) || root.join(f).exists() {
            continue;
        }
        if al.forward(f).iter().any(|p| root.join(p).exists()) {
            continue;
        }
        if in_edits.contains(f.as_str()) {
            continue;
        }
        missing.push(f.as_str());
    }
    if missing.is_empty() {
        return Ok(vec![]);
    }
    // one spawn, only when something is missing from disk — the add path
    // already spawns git for the stamp, so this costs nothing on success
    let status = git_status_files(root);
    missing.retain(|f| !status.contains(*f));
    let mut bad: Vec<(&str, String)> = vec![];
    let mut warns: Vec<String> = vec![];
    for f in missing {
        match sibling_suggest(root, f) {
            Some(near) => bad.push((f, near)),
            None => warns.push(format!(
                "warning: {f:?} matches nothing on disk — filed anyway; check the spelling"
            )),
        }
    }
    if !bad.is_empty() {
        let (f, near) = &bad[0];
        let mut msg = format!(
            "rejected: {f:?} matches nothing — not on disk, no rename leads to it, \
not in this session's edits or git status — did you mean {near:?}?"
        );
        if bad.len() > 1 {
            msg.push_str(&format!(" (+{} more)", bad.len() - 1));
        }
        return Err(msg);
    }
    Ok(warns)
}

/// A file glob (`*`, `?`, `[...]`) is a pattern, not a path — `find` matches
/// it the same way, so it always passes the write check.
fn is_glob(f: &str) -> bool {
    f.contains(['*', '?', '['])
}

/// Paths git knows in the worktree but the disk walk above missed: untracked,
/// staged and modified files (`git mv` without commit stages the new name).
/// Empty on any failure — fail-open, the caller just has less evidence.
fn git_status_files(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(s) = crate::git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    ) else {
        return out;
    };
    for e in s.split('\0') {
        // `XY <path>` or `R  <old> -> <new>`
        let p = e.get(3..).unwrap_or("").trim();
        if p.is_empty() {
            continue;
        }
        out.insert(p.rsplit(" -> ").next().unwrap_or(p).to_string());
    }
    out
}

/// A same-directory file on disk close enough that `bad` is almost surely a
/// typo of it — `None` when the parent directory is unreadable or nothing is
/// within distance 2. Only siblings count: a row file one char away may be a
/// genuinely new file (`src/a.rs` exists, `src/b.rs` is planned work), and
/// rejecting that is the false block the plan forbids.
fn sibling_suggest(root: &Path, bad: &str) -> Option<String> {
    let dir = bad.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let sib = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let Ok(rd) = std::fs::read_dir(&sib) else {
        return None;
    };
    let mut best: Option<(usize, String)> = None;
    for e in rd.flatten().take(500) {
        let n = e.file_name().to_string_lossy().replace('\\', "/");
        let cand = if dir.is_empty() {
            n
        } else {
            format!("{dir}/{n}")
        };
        if cand == bad {
            continue;
        }
        let d = distance(bad, &cand);
        if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, cand));
        }
    }
    let (d, cand) = best?;
    (d <= 2 && d < bad.chars().count()).then_some(cand)
}

/// Character Levenshtein, std only — candidates are short paths, never many.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
