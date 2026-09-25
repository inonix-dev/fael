//! L1 rename cache (`.fael/cache/aliases.json`) + the git read that fills it.
//! Core (`fael_core::Aliases`) never sees git or the disk; this module is the
//! one place that does. Everything here is fail-open: any error is an empty
//! alias set, i.e. the pre-resolver behaviour.
//!
//! Latency shapes who refreshes: one git spawn costs ~9 ms on this path, so
//! the read/edit hook (`refresh = false`) only reads the cache file and
//! builds it when it is missing entirely (the one over-budget exception the
//! plan allows). `session-start`, `find` and `kickoff` pass `refresh = true`
//! and pick up renames committed mid-session.

use crate::{Repo, core};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// What lives in `.fael/cache/aliases.json`: rename pairs plus the HEAD they
/// were read up to, so the next refresh asks only `head..HEAD`. Gitignored,
/// deletable at any time — the log stays the only source of truth.
#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    v: u8,
    head: String,
    renames: Vec<(String, String)>,
}

/// Aliases for `push` / `find --files` / `kickoff`. `refresh` runs git;
/// without it this is one small file read and no spawn.
pub fn load(r: &Repo, log: &core::Log, refresh: bool) -> core::Aliases {
    let al = core::Aliases::default();
    if !r.cfg.resolve {
        return al;
    }
    // No log anywhere = fael never adopted here: never create `.fael/` for a
    // repo that has none.
    if !r.fael.join("log").is_dir() {
        return al;
    }
    let cached = read_cache(&r.fael);
    let mut renames: Vec<(String, String)> = cached
        .as_ref()
        .map(|c| c.renames.clone())
        .unwrap_or_default();
    let push_pair = |renames: &mut Vec<(String, String)>, p: (String, String)| {
        if !p.0.is_empty() && !p.1.is_empty() && p.0 != p.1 && !renames.contains(&p) {
            renames.push(p);
        }
    };
    if refresh {
        // incremental from the cached head; a bad head (rebase/force-push)
        // falls back to a full rebuild that replaces the cache
        let inc = cached
            .as_ref()
            .filter(|c| !c.head.is_empty())
            .and_then(|c| git_renames(&r.root, Some(&c.head)));
        match inc {
            Some((head, pairs)) => {
                for p in pairs {
                    push_pair(&mut renames, p);
                }
                write_cache(&r.fael, &head, &renames);
            }
            None => {
                if let Some((head, pairs)) = git_renames(&r.root, None) {
                    renames.clear();
                    for p in pairs {
                        push_pair(&mut renames, p);
                    }
                    write_cache(&r.fael, &head, &renames);
                }
            }
        }
    } else if cached.is_none() {
        // first build — the one over-budget exception the plan allows; after
        // this the steady hook path is a file read with no git spawn
        if let Some((head, pairs)) = git_renames(&r.root, None) {
            for p in pairs {
                push_pair(&mut renames, p);
            }
            write_cache(&r.fael, &head, &renames);
        }
    }
    let mut al = core::Aliases::from_pairs(renames);
    // `fael mv` rows are read from the log every time, never cached.
    al.merge(&core::Aliases::from_log(log));
    al
}

/// Read the cache. A wrong version or a broken file is "no cache" — the
/// caller rebuilds (or carries on without aliases), never errors.
fn read_cache(fael: &Path) -> Option<Cache> {
    let s = std::fs::read_to_string(fael.join("cache").join("aliases.json")).ok()?;
    let c: Cache = serde_json::from_str(&s).ok()?;
    (c.v == 1).then_some(c)
}

/// `git rev-parse HEAD`, then `git log -M -z --name-status --diff-filter=R
/// --format=%H [<since>..]HEAD`. The head comes from `rev-parse`, never from
/// the log: `--diff-filter=R` also filters commits, so the first sha listed
/// is the newest *rename* commit, not HEAD. No HEAD (unborn branch, not a
/// repo) is `Some(("", []))` so a cache still gets written and the hook
/// never re-spawns. HEAD unchanged since `since` = `Some` with no pairs, one
/// spawn. `None` = git missing or a bad `since` (rebase/force-push: the
/// caller rebuilds from scratch).
fn git_renames(root: &Path, since: Option<&str>) -> Option<(String, Vec<(String, String)>)> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
    };
    let o = git(&["rev-parse", "--verify", "-q", "HEAD"])?;
    let head = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() || head.is_empty() {
        return Some((String::new(), vec![]));
    }
    if since == Some(head.as_str()) {
        return Some((head, vec![]));
    }
    let range = since.map_or_else(|| head.clone(), |s| format!("{s}..{head}"));
    let o = git(&[
        "log",
        "-M",
        "-z",
        "--name-status",
        "--diff-filter=R",
        "--format=%H",
        &range,
    ])?;
    if !o.status.success() {
        return None;
    }
    Some((head, parse_log(&String::from_utf8_lossy(&o.stdout))))
}

/// Pull `(old, new)` pairs out of `git log -z` output: NUL-separated tokens,
/// `R<score>` followed by the old and new path. `-z` keeps paths raw — without
/// it git C-quotes non-ASCII names and they never match a real path. Sha
/// tokens (and the `\n` git puts before the next status) are skipped.
fn parse_log(out: &str) -> Vec<(String, String)> {
    let mut pairs = vec![];
    let mut it = out.split('\0').map(|t| t.trim_start_matches('\n'));
    while let Some(t) = it.next() {
        let is_rename = t
            .strip_prefix('R')
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if !is_rename {
            continue;
        }
        if let (Some(old), Some(new)) = (it.next(), it.next())
            && !old.is_empty()
            && !new.is_empty()
            && old != new
        {
            pairs.push((old.to_string(), new.to_string()));
        }
    }
    pairs
}

/// Write the cache atomically (tmp + rename) and keep `.fael/.gitignore`
/// naming both `cache/` and `.lock` — the lock file must stay out of git too
/// (issue `01M3CM2P3`). Fail-open: a write error only loses the cache.
fn write_cache(fael: &Path, head: &str, renames: &[(String, String)]) {
    let dir = fael.join("cache");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let body = serde_json::json!({"v": 1, "head": head, "renames": renames}).to_string();
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

#[cfg(test)]
mod tests {
    use super::parse_log;

    #[test]
    fn parses_z_output() {
        // real `git log -z` shape: sha\0, then \n before each status token
        let out = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\0\nR100\0src/a.rs\0src/b.rs\0\
                   bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\0\nR095\0src/ก.rs\0src/ข.rs\0";
        assert_eq!(
            parse_log(out),
            vec![
                ("src/a.rs".to_string(), "src/b.rs".to_string()),
                ("src/ก.rs".to_string(), "src/ข.rs".to_string()),
            ]
        );
    }

    #[test]
    fn skips_non_renames_and_self_pairs() {
        let out = "M\0src/a.rs\0\nR100\0src/a.rs\0src/a.rs\0\nR100\0src/a.rs\0";
        assert!(parse_log(out).is_empty());
    }
}
