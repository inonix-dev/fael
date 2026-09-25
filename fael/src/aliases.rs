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
    let mut renames: Vec<(String, String)> =
        cached.as_ref().map(|c| c.renames.clone()).unwrap_or_default();
    let push_pair = |renames: &mut Vec<(String, String)>, p: (String, String)| {
        if !p.0.is_empty() && !p.1.is_empty() && p.0 != p.1 && !renames.contains(&p) {
            renames.push(p);
        }
    };
    if refresh {
        // incremental from the cached head; a bad head (rebase/force-push)
        // falls back to a full rebuild that replaces the cache
        let inc = cached.as_ref().and_then(|c| git_renames(&r.root, Some(&c.head)));
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

/// `git log -M --name-status --diff-filter=R --format=%H [<since>..HEAD]`.
/// Returns the new head (the first commit listed, i.e. HEAD itself) and the
/// rename pairs. An empty range is `Some` with no pairs — HEAD did not move.
/// `None` = git missing, not a repo, or a bad `since` (rebase/force-push:
/// the caller rebuilds from scratch).
fn git_renames(root: &Path, since: Option<&str>) -> Option<(String, Vec<(String, String)>)> {
    let range = since.map(|s| format!("{s}..HEAD"));
    let mut args = vec![
        "log",
        "-M",
        "--name-status",
        "--diff-filter=R",
        "--format=%H",
    ];
    if let Some(r) = &range {
        args.push(r);
    } else {
        args.push("HEAD");
    }
    let o = Command::new("git").args(&args).current_dir(root).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let (shas, pairs) = parse_log(&String::from_utf8_lossy(&o.stdout));
    let head = shas.into_iter().next().or_else(|| since.map(String::from))?;
    Some((head, pairs))
}

/// Split `git log` output into commit shas (full `%H` lines) and `(old, new)`
/// pairs (`R<score>\t<old>\t<new>`). A tab inside a filename breaks the pair
/// and the line is skipped — vanishingly rare, and skipping only loses one
/// alias, never a row.
fn parse_log(out: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut shas = vec![];
    let mut pairs = vec![];
    for line in out.lines() {
        if line.len() == 40 && line.bytes().all(|b| b.is_ascii_hexdigit()) {
            shas.push(line.to_string());
        } else if let Some(rest) = line.strip_prefix('R') {
            let mut it = rest.split('\t');
            match (it.next(), it.next(), it.next()) {
                (Some(score), Some(old), Some(new))
                    if !score.is_empty()
                        && score.bytes().all(|b| b.is_ascii_digit())
                        && !old.is_empty()
                        && !new.is_empty()
                        && old != new =>
                {
                    pairs.push((old.to_string(), new.to_string()));
                }
                _ => {}
            }
        }
    }
    (shas, pairs)
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
    fn parses_renames_and_shas() {
        let out = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
                   R100\tsrc/a.rs\tsrc/b.rs\n\
                   R095\tsrc/old/x.rs\tsrc/new/x.rs\n\
                   bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n";
        let (shas, pairs) = parse_log(out);
        assert_eq!(shas.len(), 2);
        assert_eq!(
            pairs,
            vec![
                ("src/a.rs".to_string(), "src/b.rs".to_string()),
                ("src/old/x.rs".to_string(), "src/new/x.rs".to_string()),
            ]
        );
    }

    #[test]
    fn skips_non_renames_and_self_pairs() {
        // M lines never appear (--diff-filter=R) but must not parse as pairs
        let out = "M\tsrc/a.rs\nR100\tsrc/a.rs\tsrc/a.rs\nR100\tsrc/a.rs\n";
        assert!(parse_log(out).1.is_empty());
    }
}
