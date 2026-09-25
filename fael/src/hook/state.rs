//! Per-machine runtime state, never in `.fael/` (that is shared project
//! data): session edit lists, seen ids, stop-block dedupe, usage — plus the
//! tiny std-only time helpers the hook path uses instead of chrono.

use crate::{core, home};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("FAEL_STATE_DIR")
        && !d.is_empty()
    {
        return PathBuf::from(d);
    }
    home()
        .unwrap_or_else(|| ".".into())
        .join(".local/state/fael")
}

/// Opaque filename for a session id or transcript path (paths are long).
pub(crate) fn session_key(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Keyed by session + worktree so one session across two repos stays apart.
// ponytail: no cleanup of old sessions — prune by mtime if the dir grows.
pub(crate) fn edits_path(session: &str, root: &Path) -> PathBuf {
    let key = session_key(&format!("{session}\0{}", root.to_string_lossy()));
    state_dir().join("sessions").join(format!("{key}.jsonl"))
}

/// Ids already pushed in this session, one per line.
// ponytail: after the client compacts its context the pushed text may be gone, yet the
// row stays "seen" — add a reset on the compact hook if agents miss rows because of it.
pub(crate) fn seen_path(session: &str, root: &Path) -> PathBuf {
    let key = session_key(&format!("{session}\0{}", root.to_string_lossy()));
    state_dir().join("sessions").join(format!("{key}.seen"))
}

/// One `{"path","at"[, "worktree","session"]}` line per edit event, in order —
/// (path, at ms, worktree, session; None for lines written before those were).
/// A torn or unreadable line is skipped; any error is an empty list.
pub(crate) type Edit = (String, i64, Option<String>, Option<String>);
pub(crate) fn session_edits(path: &Path) -> Vec<Edit> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return vec![];
    };
    s.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            Some((
                v["path"].as_str()?.to_string(),
                core::ts_ms(v["at"].as_str()?)?,
                v["worktree"].as_str().map(String::from),
                v["session"].as_str().map(String::from),
            ))
        })
        .collect()
}

/// Append one line per file. Fails open like record_usage. Worktree and
/// session ride along so `fael add` without `--files` can tell which session
/// files are its own, in its repo (the filename hash is one-way).
pub(crate) fn record_edits(path: &Path, worktree: &str, session: &str, files: &[String]) {
    let Some(at) = now_rfc3339() else { return };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        use std::io::Write;
        let body: String = files
            .iter()
            .map(|f| {
                format!(
                    "{}\n",
                    serde_json::json!({"path": f, "at": at, "worktree": worktree, "session": session})
                )
            })
            .collect();
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(body.as_bytes()));
    }
}

/// A file's birthtime (fallback: mtime) as unix ms.
pub(crate) fn file_birth_ms(path: &Path) -> Option<u64> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.created().or_else(|_| md.modified()).ok()?;
    Some(t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as u64)
}

pub(crate) fn now_rfc3339() -> Option<String> {
    systemtime_to_rfc3339(SystemTime::now())
}

fn systemtime_to_rfc3339(st: SystemTime) -> Option<String> {
    let ms = st.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(core::rfc3339(ms))
}
