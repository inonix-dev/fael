//! The `.fael/cache/aliases.json` file: rename pairs plus the HEAD they
//! were read up to. Gitignored, deletable at any time — the log stays the
//! only source of truth.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// What lives in `.fael/cache/aliases.json`: rename pairs plus the HEAD they
/// were read up to, so the next refresh asks only `head..HEAD`. Gitignored,
/// deletable at any time — the log stays the only source of truth.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Cache {
    pub(crate) v: u8,
    pub(crate) head: String,
    pub(crate) renames: Vec<(String, String)>,
    /// Row paths missing from disk with no blob at `head` — deleted, not
    /// moved. The hook skips them, so a long-deleted file never costs a
    /// spawn per read/edit; refresh recomputes the list.
    #[serde(default)]
    pub(crate) dead: Vec<String>,
}

/// Read the cache. A wrong version or a broken file is "no cache" — the
/// caller rebuilds (or carries on without aliases), never errors.
pub(crate) fn read_cache(fael: &Path) -> Option<Cache> {
    let s = std::fs::read_to_string(fael.join("cache").join("aliases.json")).ok()?;
    let c: Cache = serde_json::from_str(&s).ok()?;
    (c.v == 1).then_some(c)
}

/// Write the cache atomically (tmp + rename) and keep `.fael/.gitignore`
/// naming both `cache/` and `.lock` — the lock file must stay out of git too
/// (issue `01M3CM2P3`). Fail-open: a write error only loses the cache.
pub(crate) fn write_cache(fael: &Path, head: &str, renames: &[(String, String)], dead: &[String]) {
    let dir = fael.join("cache");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let body =
        serde_json::json!({"v": 1, "head": head, "renames": renames, "dead": dead}).to_string();
    let tmp = dir.join(format!(".aliases-{}.tmp", std::process::id()));
    if std::fs::write(&tmp, body.as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("aliases.json"));
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
    ensure_gitignore(fael);
}

/// `.fael/.gitignore` names what `.fael/` keeps out of git. Missing file =
/// create it; present but missing a line = append the line.
fn ensure_gitignore(fael: &Path) {
    const LINES: [&str; 2] = ["cache/", ".lock"];
    let path = fael.join(".gitignore");
    let cur = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = LINES
        .into_iter()
        .filter(|l| !cur.lines().any(|c| c.trim() == *l))
        .collect();
    if missing.is_empty() {
        return;
    }
    if cur.is_empty() {
        let mut body = String::new();
        for l in missing {
            body.push_str(l);
            body.push('\n');
        }
        let _ = std::fs::write(&path, body);
    } else {
        // append only the missing lines, keep what the owner wrote
        let mut extra = String::new();
        if !cur.ends_with('\n') {
            extra.push('\n');
        }
        for l in missing {
            extra.push_str(l);
            extra.push('\n');
        }
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(extra.as_bytes()));
    }
}
