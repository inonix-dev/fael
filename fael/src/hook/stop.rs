//! The stop event: block the turn when the session did work (edits after
//! the newest row, or commits when the edit hook saw nothing) but filed no
//! row — or when the assistant announced a bug with no issue row since.

use super::markers::{bug_signal_from_transcript, has_bug_marker};
use super::protocol::{Event, Reply, ctx};
use super::state::{edits_path, file_birth_ms, now_rfc3339, session_edits, session_key, state_dir};
use super::usage::record_usage;
use crate::{core, git};
use std::path::{Path, PathBuf};

pub(crate) fn stop(e: &Event) -> Reply {
    let no = || Reply {
        block: false,
        reason: None,
        context: None,
    };
    let c = match ctx(e) {
        Some(c) => c,
        None => return no(),
    };
    if e.stop_active {
        return no();
    }
    // no log anywhere under .fael/ = fael never adopted here — allow before
    // spending a git spawn or a transcript read (decide_stop agrees: !has_log
    // never blocks)
    let log_path = c.repo.fael.join("log");
    if !(log_path.is_dir() && walk_jsonl(&log_path).next().is_some()) {
        return no();
    }
    // session start: an RFC 3339 time, or a transcript file's birthtime.
    // Recency compares run at ms precision (`since_ms`) — whole seconds race
    // with rows filed just before the session start; the `since` string stays
    // second-precision for `git log --since`, which only parses that far.
    let (since, since_ms) = match e.session.as_deref() {
        // an RFC 3339 start time (neutral callers without a transcript)
        Some(s) => match core::ts_ms(s) {
            Some(ms) => (since_secs(ms), ms),
            None => match file_birth_ms(Path::new(s)) {
                // a transcript file — birthtime (fallback: mtime) is the start
                Some(ms) => (since_secs(ms as i64), ms as i64),
                None => return no(),
            },
        },
        None => return no(),
    };
    let root = &c.repo.root;
    // edits count only after the session's newest row — a row filed early
    // does not cover hours of work after it
    let last_row = core::last_row_ms(&c.log, since_ms);
    let recorded = session_edits(&edits_path(&c.session, root));
    let mut edits: Vec<String> = vec![];
    for (path, at, ..) in &recorded {
        if last_row.is_none_or(|r| *at > r) && !edits.contains(path) {
            edits.push(path.clone());
        }
    }
    // commits only when the edit hook saw nothing (e.g. edits via a shell) —
    // the one git spawn left on this path
    let commits: Vec<String> = if recorded.is_empty() {
        git(root, &["log", "--since", &since, "--format=%h %s"])
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    } else {
        vec![]
    };
    // bug rule: a marker in the transcript tail with no issue row since start
    let bug_signal = match (&e.text, e.session.as_deref()) {
        (Some(text), _) => has_bug_marker(text),
        (None, Some(t)) if Path::new(t).is_file() => {
            bug_signal_from_transcript(Path::new(t), since_ms)
        }
        _ => None,
    };
    let bug_row_since = c
        .log
        .rows
        .iter()
        .any(|r| r.kind == "issue" && core::ts_ms(&r.ts).is_some_and(|ms| ms >= since_ms));
    let reason = core::decide_stop(&core::StopFacts {
        stop_active: false,
        edits,
        commits,
        new_row: last_row.is_some(),
        has_log: true,
        bug_signal: bug_signal.clone(),
        bug_row_since,
    });
    let Some(reason) = reason else { return no() };
    // once per session per worktree per problem — the second end lets through.
    // A new row opens one more work block, for edits made after it.
    let kind = bug_signal
        .map(|m| format!("bug:{m}"))
        .unwrap_or_else(|| format!("work:{}", last_row.unwrap_or(0)));
    if stop_blocked_before(&c.session, &root.to_string_lossy(), &kind) {
        return no();
    }
    // the reason lands in context like any push; stats reads these back to
    // count how many blocks were followed by a row
    let event = if kind.starts_with("bug:") {
        "stop-bug"
    } else {
        "stop-work"
    };
    record_usage(&c.client, event, root, &reason, &[]);
    Reply {
        block: true,
        reason: Some(reason),
        context: None,
    }
}

/// Floor to whole seconds for `git log --since` — flooring can only include
/// a commit from the start second, never drop one.
fn since_secs(ms: i64) -> String {
    let s = core::rfc3339((ms.max(0) as u64) / 1000 * 1000);
    s.replacen(".000Z", "Z", 1)
}

fn walk_jsonl(dir: &Path) -> impl Iterator<Item = PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    std::iter::from_fn(move || {
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "jsonl") {
                    return Some(p);
                }
            }
        }
        None
    })
}

/// True when this session already blocked for this worktree + kind — else
/// record the block and return false. Empty session = no dedupe (block).
fn stop_blocked_before(session: &str, worktree: &str, kind: &str) -> bool {
    if session.is_empty() {
        return false;
    }
    let path = state_dir()
        .join("stop-block")
        .join(format!("{}.jsonl", session_key(session)));
    if let Ok(s) = std::fs::read_to_string(&path) {
        for line in s.lines() {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue, // a torn line must not lose the rest
            };
            if v["worktree"] == *worktree && v["kind"] == *kind {
                return true;
            }
        }
    }
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        use std::io::{Read, Seek, SeekFrom, Write};
        let row = serde_json::json!({
            "ts": now_rfc3339().unwrap_or_default(),
            "worktree": worktree, "kind": kind,
        });
        let mut f = match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(_) => return false,
        };
        // seal a torn tail so the new row starts on its own line
        let seal = f
            .seek(SeekFrom::End(0))
            .map(|n| {
                if n == 0 {
                    return false;
                }
                let mut last = [0u8];
                f.seek(SeekFrom::End(-1)).is_ok()
                    && f.read_exact(&mut last).is_ok()
                    && last[0] != b'\n'
            })
            .unwrap_or(false);
        let _ = writeln!(f, "{}{row}", if seal { "\n" } else { "" });
    }
    false
}
