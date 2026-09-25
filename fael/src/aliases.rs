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
//!
//! Thin entry only — the halves live in `aliases/`:
//! `cache` (the JSON file), `git` (the spawns that fill it).

mod cache;
mod git;

use crate::{Repo, core};
use cache::{read_cache, write_cache};
use git::{git_blobs, git_renames, uncommitted_pairs};

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
    // set when git was read: the cache is rewritten below, dead list included
    let mut write: Option<String> = None;
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
                write = Some(head);
            }
            None => {
                if let Some((head, pairs)) = git_renames(&r.root, None) {
                    renames.clear();
                    for p in pairs {
                        push_pair(&mut renames, p);
                    }
                    write = Some(head);
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
            write = Some(head);
        }
    }
    let mut al = core::Aliases::from_pairs(renames.clone());
    // `fael mv` rows are read from the log every time, never cached.
    al.merge(&core::Aliases::from_log(log));
    // Moves git hasn't committed yet: live blob-hash comparison, never cached
    // (an uncommitted tree changes under us). Only a row path missing from
    // disk costs a spawn, and one already known dead (no HEAD blob: deleted,
    // not moved) is skipped — otherwise one old deleted file taxed every
    // read/edit hook (~9 ms per spawn). Fail-open like everything else here.
    let mut missing = al.missing(&r.root, log);
    missing.truncate(200); // caps keep a huge worktree from stalling a hook
    if let Some(head) = write {
        // git was read anyway: settle which missing paths are dead at HEAD
        // (a failed ls-tree marks nothing dead — the hook just keeps asking)
        let blobs = git_blobs(&r.root, &missing);
        let dead: Vec<String> = match &blobs {
            Some(b) => missing
                .iter()
                .filter(|m| !b.contains_key(*m))
                .cloned()
                .collect(),
            None => vec![],
        };
        let blobs = blobs.unwrap_or_default();
        write_cache(&r.fael, &head, &renames, &dead);
        al.merge_pairs(&uncommitted_pairs(&r.root, &missing, &blobs));
    } else {
        let dead = cached.map(|c| c.dead).unwrap_or_default();
        missing.retain(|m| !dead.contains(m));
        if !missing.is_empty()
            && let Some(blobs) = git_blobs(&r.root, &missing)
        {
            al.merge_pairs(&uncommitted_pairs(&r.root, &missing, &blobs));
        }
    }
    al
}
