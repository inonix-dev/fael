//! Write-side path integrity (PLAN-fael-path-integrity chunk 4): `fael add`
//! without `--files` inherits the files this session edited, and every files
//! entry is checked against evidence before the row is written.
//!
//! Core never sees the disk here (PLAN §4) — this module is the binary side:
//! it reads the hook's session edits and the worktree, and core only gets the
//! resulting list through the normal `add_row` path.

use crate::{core, hook};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Optional fields for `add_row` — bundled so the arg count stays under the lint.
pub(crate) struct AddOpts {
    pub key: Option<String>,
    pub to: Option<String>,
    pub title: Option<String>,
    pub urgent: core::Urgent,
    pub supersedes: Option<String>,
    pub force: bool,
}

/// Normalise files against cwd, then core's add path — shared by the CLI and MCP.
/// No files: inherit the files this session edited (after the newest row);
/// still empty without a hook session, and core keeps rejecting that.
/// Every entry is checked against evidence (disk · renames · session edits ·
/// git status) before the row is written.
pub(crate) fn add_row(
    r: &crate::Repo,
    kind: &str,
    text: &str,
    files: &[String],
    opts: AddOpts,
) -> Result<(core::Row, PathBuf, Vec<String>), String> {
    let AddOpts {
        key,
        to,
        title,
        urgent,
        supersedes,
        force,
    } = opts;
    let mut files = core::normalize_files(files, &r.cwd, &r.root)?;
    let log = crate::read(r);
    if files.is_empty() {
        files = derive(&r.root, &log);
    }
    let mut warns = check(
        &r.root,
        &crate::aliases::load(r, &log, true),
        &files,
        &active_edits(&r.root),
        force,
    )?;
    let st = crate::stamp(r);
    let mut row = core::Row::new(&st.by, kind, text, files);
    row.key = key;
    // a headline lists show; the body stays in `text` for `find <id>` / `--full`
    row.title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    // everything identity-like is lowercase: `--to Delamind` stores `delamind`
    row.to = to
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    // the queue position resolves against the open issues (`--urgent` = back,
    // `--urgent-before` = just above that row); core rejects non-issues
    row.urgent = core::resolve_urgent(&log, &urgent)?;
    let (row, path, mut core_warns) =
        core::add_row(&r.fael, &log, &r.cfg, &st, row, supersedes.as_deref())?;
    warns.append(&mut core_warns);
    Ok((row, path, warns))
}

/// `fael bump <id>` — change routing/urgency as a new version: same
/// kind/text/files/key, new `to`/`urgent`, superseding the old row. At most
/// one of `--urgent` (back of the queue), `--urgent-before <id>` (just above
/// that row), `--not-urgent` (leave the queue); none keeps the old number.
pub(crate) fn bump(
    r: &crate::Repo,
    a: &crate::Args,
    id: &str,
) -> Result<(core::Row, PathBuf, Vec<String>), String> {
    let urgent = match (a.has("urgent"), a.one("urgent-before"), a.has("not-urgent")) {
        (false, None, false) => core::UrgentChange::Keep,
        (true, None, false) => core::UrgentChange::End,
        (false, Some(t), false) => core::UrgentChange::Before(t),
        (false, None, true) => core::UrgentChange::Remove,
        _ => {
            return Err(
                "rejected: bump takes at most one of --urgent, --urgent-before, --not-urgent"
                    .into(),
            );
        }
    };
    let log = crate::read(r);
    core::bump_row(
        &r.fael,
        &log,
        &r.cfg,
        &crate::stamp(r),
        id,
        a.one("to"),
        urgent,
    )
}

/// A session stays usable for deriving files while its edit file was written
/// recently — the plan's guess is 2 h.
const ACTIVE_SECS: u64 = 2 * 60 * 60;

/// Each active session file in this worktree with its edits, oldest file
/// first. Lines without a worktree predate it and are kept (they age out with
/// the 2 h window); lines naming another worktree are dropped — without this
/// a row filed in repo A would inherit files touched in repo B.
fn active_sessions(root: &Path) -> Vec<Vec<hook::Edit>> {
    let dir = hook::state_dir().join("sessions");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let here = root.to_string_lossy();
    let mut files: Vec<(u64, PathBuf)> = vec![];
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
    // oldest session file first (largest age), so the union reads in edit order
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .filter_map(|(_, p)| {
            let edits: Vec<_> = hook::session_edits(&p)
                .into_iter()
                .filter(|(_, _, w, _)| w.as_deref().is_none_or(|w| w == here))
                .collect();
            (!edits.is_empty()).then_some(edits)
        })
        .collect()
}

/// Every edit any active session recorded in this worktree — evidence for
/// `check`, where another session's edit only widens what passes.
pub(crate) fn active_edits(root: &Path) -> Vec<(String, i64)> {
    active_sessions(root)
        .into_iter()
        .flatten()
        .map(|(path, at, ..)| (path, at))
        .collect()
}

/// Files for a row filed now: the caller's own session edits newer than the
/// newest row, order kept, deduped. The caller's session is
/// `CLAUDE_CODE_SESSION_ID` — the hook keys Claude by transcript path, so an
/// edit line matches on that file's stem too; without it, only a single
/// active session counts — two agents in one checkout must never file rows on
/// each other's files. Empty = the caller keeps the old "files is required"
/// error, so behaviour without a hook session is unchanged.
// ponytail: the cutoff is the newest row by anyone (as the stop hook does) —
// another agent's row can hide older edits, which fails toward "files is
// required", never toward wrong files. Rows would need a session to do better.
pub(crate) fn derive(root: &Path, log: &core::Log) -> Vec<String> {
    let mut sessions = active_sessions(root);
    let mine = match std::env::var("CLAUDE_CODE_SESSION_ID") {
        Ok(id) if !id.is_empty() => sessions.into_iter().find(|e| {
            e.iter().any(|(.., s)| {
                s.as_deref()
                    .is_some_and(|s| s == id || Path::new(s).file_stem().is_some_and(|f| *f == *id))
            })
        }),
        _ if sessions.len() == 1 => sessions.pop(),
        _ => None,
    };
    let last = core::last_row_ms(log, 0);
    let mut seen = HashSet::new();
    let mut out = vec![];
    for (path, at, ..) in mine.unwrap_or_default() {
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
/// A planned file can sit one char from a real one (`b.rs` next to `a.rs`),
/// so `force` turns the rejection into a warning.
pub(crate) fn check(
    root: &Path,
    al: &core::Aliases,
    files: &[String],
    edits: &[(String, i64)],
    force: bool,
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
            Some(near) if !force => bad.push((f, near)),
            _ => warns.push(format!(
                "warning: {f:?} matches nothing on disk — filed anyway; check the spelling"
            )),
        }
    }
    if !bad.is_empty() {
        let (f, near) = &bad[0];
        let mut msg = format!(
            "rejected: {f:?} matches nothing — not on disk, no rename leads to it, \
not in this session's edits or git status — did you mean {near:?}? \
(a file you have not created yet: add --force)"
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
    // `-z` entries are `XY <path>`; a rename or copy (`R`/`C` in X) is
    // followed by its source as a bare entry — the new name is the evidence
    let mut it = s.split('\0');
    while let Some(e) = it.next() {
        let Some(p) = e.get(3..).filter(|p| !p.is_empty()) else {
            continue;
        };
        if e.starts_with(['R', 'C']) {
            it.next();
        }
        out.insert(p.to_string());
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
