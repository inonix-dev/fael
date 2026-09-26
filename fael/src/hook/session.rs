//! The session-start event: kickoff rows for the repo plus the report line,
//! and the one-line warning when `.fael/log` is gitignored by mistake.

use super::protocol::{Event, Reply, ctx};
use super::state::{session_key, state_dir};
use super::usage::record_usage;
use crate::{Filter, aliases, core, home};
use std::path::{Path, PathBuf};

/// The one line that makes agents report (decision mugea7lt) — the hook only
/// catches what an agent says, this is what gets it said. Same line in the MCP
/// `add` description and the installed skill.
const ISSUE_LINE: &str = "- fael: saw something broken, inconsistent or likely to break? \
`fael add issue \"<what>\" --files <path>` right there — do not wait for the end of the task";

pub(crate) fn session_start(e: &Event) -> Reply {
    let no = || Reply {
        block: false,
        reason: None,
        context: None,
    };
    let c = match ctx(e) {
        Some(c) => c,
        None => return no(),
    };
    // once per session: pick up renames committed since the last session, so
    // the read/edit push (which never spawns git) resolves them — and kickoff
    // keeps rows whose files were merely renamed
    let al = aliases::load(&c.repo, &c.log, true);
    // PLAN-fael-direction chunk 2: issues `to` this reader are the reader's
    // job — listed in full above the count line; the count covers the rest.
    // `to` never changes push or find. Reader identity needs git, which
    // session-start already spawns (aliases refresh above, check-ignore
    // below); read/edit never compute it (SPEC fail examples).
    let reader = crate::writer(&c.repo);
    let mut mine: Vec<&core::Row> = vec![];
    let mut rest = 0;
    for r in core::find(
        &c.log,
        &Filter {
            kind: Some("issue".into()),
            ..Filter::default()
        },
    ) {
        match r.to_who() {
            Some(t) if core::to_matches(t, &reader) => mine.push(r),
            _ => rest += 1,
        }
    }
    let decisions: Vec<_> = if c.repo.cfg.session_decisions == 0 {
        vec![]
    } else {
        core::kickoff(
            &c.log,
            &Filter {
                kind: Some("decision".into()),
                ..Filter::default()
            },
            &c.repo.root,
            &al,
        )
        .into_iter()
        .take(c.repo.cfg.session_decisions)
        .collect()
    };
    let mut body = core::render(&c.log, &mine, c.repo.cfg.kickoff_tokens);
    body.push_str(&core::render(&c.log, &decisions, c.repo.cfg.kickoff_tokens));
    if rest > 0 {
        body.push_str(&format!(
            "fael: {} {}open {} — fael find --kind issue (MCP find kind=issue); each also pushes when you touch its file\n",
            rest,
            if mine.is_empty() { "" } else { "more " },
            if rest == 1 { "issue" } else { "issues" },
        ));
    }
    let adopted = c.repo.fael.join("log").is_dir();
    let mut context = match (body.is_empty(), adopted) {
        (true, false) => None,
        (true, true) => Some(format!("{ISSUE_LINE}\n")),
        (false, _) => Some(format!("{body}{ISSUE_LINE}\n")),
    };
    // SPEC §11: the cheap check — one line, only when there is a problem.
    // Skipped while no log exists yet: warning about an empty missing log is
    // noise, and it saves a git spawn on every session start.
    if adopted && check_ignore_hit(&c.repo.root) {
        let warn = "fael: .fael/log is gitignored — rows stay on this machine, run fael doctor";
        context = Some(match context {
            Some(c) => format!("{c}{warn}\n"),
            None => format!("{warn}\n"),
        });
    }
    let Some(context) = context else { return no() };
    record_usage(
        &c.client,
        "session-start",
        &c.repo.root,
        &context,
        &mine
            .iter()
            .chain(decisions.iter())
            .map(|r| r.id.clone())
            .collect::<Vec<_>>(),
    );
    Reply {
        block: false,
        reason: None,
        context: Some(context),
    }
}

/// `git check-ignore` is ~8 of session-start's ~10 ms, so its answer is cached
/// per worktree, keyed by the mtime+size of every file that can change it.
// ponytail: stamps root and .fael .gitignore, info/exclude, the default global
// ignore and ~/.gitconfig (a moved core.excludesFile) — edits inside a custom
// excludesFile or a worktree's common info/exclude are missed until another
// stamp moves; `fael doctor` always asks git.
fn check_ignore_hit(root: &Path) -> bool {
    let home = home().unwrap_or_default();
    let xdg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    let stamp: String = [
        root.join(".gitignore"),
        root.join(".fael/.gitignore"),
        root.join(".fael/log/.gitignore"),
        root.join(".git/info/exclude"),
        xdg.join("git/ignore"),
        home.join(".gitconfig"),
    ]
    .iter()
    .map(|p| match std::fs::metadata(p) {
        Ok(m) => {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
            format!("{}.{},", t.map_or(0, |t| t.as_nanos()), m.len())
        }
        Err(_) => "-,".into(),
    })
    .collect();
    let cache = state_dir()
        .join("ignore")
        .join(session_key(&root.to_string_lossy()));
    if let Some(hit) = std::fs::read_to_string(&cache)
        .ok()
        .and_then(|s| s.strip_prefix(&stamp).map(|v| v == "1"))
    {
        return hit;
    }
    let hit = ignore_source(root).is_some_and(|s| !deliberate(&s));
    let _ = std::fs::create_dir_all(cache.parent().unwrap_or(root));
    let _ = std::fs::write(&cache, format!("{stamp}{}", u8::from(hit)));
    hit
}

/// Which ignore rule keeps `.fael/log` out of git (`git check-ignore -v`'s
/// `<source>:<line>:<pattern>\t<path>`), or `None` when it is tracked.
pub(crate) fn ignore_source(root: &Path) -> Option<String> {
    crate::git(root, &["check-ignore", "-v", ".fael/log"])
}

/// `.git/info/exclude` is local to this clone and never shared, so ignoring the log
/// there is a choice (a public repo keeping its memory private), not a mistake.
pub(crate) fn deliberate(source: &str) -> bool {
    source.replace('\\', "/").contains("info/exclude:")
}
