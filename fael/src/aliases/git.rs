//! The git reads behind the cache: committed renames from `git log -M`,
//! uncommitted moves from a live HEAD-blob comparison. Batched spawns,
//! fail-open throughout.

use std::path::Path;
use std::process::Command;

/// `git rev-parse HEAD`, then `git log -M -z --name-status --diff-filter=R
/// --format=%H [<since>..]HEAD`. The head comes from `rev-parse`, never from
/// the log: `--diff-filter=R` also filters commits, so the first sha listed
/// is the newest *rename* commit, not HEAD. No HEAD (unborn branch, not a
/// repo) is `Some(("", []))` so a cache still gets written and the hook
/// never re-spawns. HEAD unchanged since `since` = `Some` with no pairs, one
/// spawn. `None` = git missing or a bad `since` (rebase/force-push: the
/// caller rebuilds from scratch).
pub(crate) fn git_renames(
    root: &Path,
    since: Option<&str>,
) -> Option<(String, Vec<(String, String)>)> {
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

/// Pairs for moves git hasn't committed yet (`mv a b`, `git mv a b` without
/// the commit). For every path an open row names that is gone from the disk,
/// compare its HEAD blob against untracked and staged-new files with the same
/// extension. One spawn per git question, batched; anything failing is no
/// pairs (fail-open). Caps keep a huge worktree from stalling a hook.
pub(crate) fn uncommitted_pairs(
    root: &Path,
    missing: &[String],
    blobs: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    if blobs.is_empty() {
        return vec![];
    }
    let mut news = git_new_files(root);
    if news.is_empty() {
        return vec![];
    }
    news.truncate(1000);
    let mut pairs = vec![];
    // hash only the new files whose extension matches some missing old path
    for old in missing.iter().filter(|o| blobs.contains_key(*o)) {
        let ext = extension(old);
        let cands: Vec<&String> = news.iter().filter(|n| extension(n) == ext).collect();
        if cands.is_empty() {
            continue;
        }
        if let Some(hashes) = git_hash_object(root, &cands)
            && let Some(want) = blobs.get(old)
        {
            for (n, h) in cands.iter().zip(hashes.iter()) {
                if h == want && *n != old {
                    pairs.push((old.clone(), (*n).clone()));
                }
            }
        }
    }
    pairs
}

/// The file-name extension (`rs` for `src/a.rs`, `""` for `Makefile`) — a
/// move keeps it, so it bounds which new files get hashed per missing old.
fn extension(p: &str) -> &str {
    p.rsplit('/')
        .next()
        .unwrap_or(p)
        .rsplit_once('.')
        .map(|(_, e)| e)
        .unwrap_or("")
}

/// HEAD blob per path, one spawn: `git ls-tree HEAD -z -- <paths>`. `None` =
/// no HEAD (unborn branch, not a repo) or git failing — fail-open.
pub(crate) fn git_blobs(
    root: &Path,
    paths: &[String],
) -> Option<std::collections::HashMap<String, String>> {
    let mut args = vec!["ls-tree", "HEAD", "-z", "--"];
    args.extend(paths.iter().map(String::as_str));
    let o = Command::new("git")
        .args(&args)
        .current_dir(root)
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let mut blobs = std::collections::HashMap::new();
    for e in String::from_utf8_lossy(&o.stdout).split('\0') {
        // `100644 blob <sha>\t<path>`
        if let Some((meta, path)) = e.split_once('\t')
            && meta.split(' ').nth(1) == Some("blob")
            && let Some(sha) = meta.split(' ').nth(2)
        {
            blobs.insert(path.to_string(), sha.to_string());
        }
    }
    Some(blobs)
}

/// Untracked files plus staged-new ones (`git mv` without commit stages the
/// new name) — where an uncommitted move's target lives. NUL-separated, so
/// non-ASCII names survive.
fn git_new_files(root: &Path) -> Vec<String> {
    let mut out = vec![];
    for args in [
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
        &[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=A",
            "--",
        ][..],
    ] {
        if let Ok(o) = Command::new("git").args(args).current_dir(root).output()
            && o.status.success()
        {
            out.extend(
                String::from_utf8_lossy(&o.stdout)
                    .split('\0')
                    .filter(|p| !p.is_empty())
                    .map(String::from),
            );
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Blob hashes for worktree files, one spawn, order kept (caller zips).
fn git_hash_object(root: &Path, files: &[&String]) -> Option<Vec<String>> {
    let mut args = vec!["hash-object", "--"];
    args.extend(files.iter().map(|s| s.as_str()));
    let o = Command::new("git")
        .args(&args)
        .current_dir(root)
        .output()
        .ok()?;
    (o.status.success()).then(|| {
        String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(String::from)
            .collect()
    })
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
