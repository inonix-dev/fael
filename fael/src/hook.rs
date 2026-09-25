//! `fael hook <stop|session-start|read|edit> [--client c]` — stdin in, stdout out.
//! The decision (`core::decide_stop`, `core::push`) is written once; each
//! adapter only parses its client's JSON and renders the answer back.
//! No `--client` = the neutral protocol from SPEC §9: Event in, Reply out.
//! Adapters: `claude`, `codex` (same hook shape; codex hands stop its last
//! message and edits as apply_patch). OpenCode's plugin speaks neutral.
//!
//! The hook always exits 0. Any internal error is an empty Reply (let the
//! turn through) — a memory tool must never break the agent's tool call.

use crate::{Filter, Repo, core, git, read, repo_at};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

/// Neutral Event (SPEC §9) — also the shape every adapter normalises to.
#[derive(Debug, Default, Deserialize)]
struct Event {
    #[serde(default)]
    cwd: Option<String>,
    /// stop: a transcript path (birthtime = session start) or an RFC 3339
    /// start time · edit: send the same string as stop — it keys the session's
    /// edit list · read: unused (reuse suppression is the client's job)
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default, alias = "stop_hook_active")]
    stop_active: bool,
    /// stop: the assistant's text since the session start, for the issue rule.
    /// Any client that can see its own messages sends it; without it fael
    /// falls back to reading `session` as a Claude-format transcript.
    #[serde(default)]
    text: Option<String>,
}

/// Neutral Reply (SPEC §9).
#[derive(Debug, Serialize)]
struct Reply {
    block: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

pub fn cmd(event: &str, client: Option<String>) -> ExitCode {
    let mut stdin = String::new();
    if std::io::stdin().read_to_string(&mut stdin).is_err() {
        return fail_open(event, client.as_deref());
    }
    match client.as_deref() {
        None => neutral(event, &stdin),
        Some(c @ ("claude" | "codex")) => claude(event, &stdin, c),
        Some(_) => ExitCode::SUCCESS, // unknown client: fail open, print nothing
    }
}

/// Parse broke or wrong event — still exit 0, with the shape the caller reads.
fn fail_open(_event: &str, client: Option<&str>) -> ExitCode {
    if client.is_none() {
        println!("{}", serde_json::json!({"block": false}));
    }
    ExitCode::SUCCESS
}

// --- neutral ---

fn neutral(event: &str, stdin: &str) -> ExitCode {
    let e: Event = match serde_json::from_str(stdin) {
        Ok(e) => e,
        Err(_) => return fail_open(event, None),
    };
    let reply = match event {
        "stop" => stop(&e),
        "session-start" => session_start(&e),
        "read" | "edit" => push(&e, event),
        _ => {
            eprintln!("fael hook: unknown event {event:?} — want stop|session-start|read|edit");
            Reply {
                block: false,
                reason: None,
                context: None,
            }
        }
    };
    println!(
        "{}",
        serde_json::to_string(&reply).unwrap_or(r#"{"block":false}"#.into())
    );
    ExitCode::SUCCESS
}

// --- claude ---

#[derive(Debug, Default, Deserialize)]
struct ClaudeBase {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    transcript_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ClaudeStop {
    #[serde(flatten)]
    base: ClaudeBase,
    #[serde(default)]
    stop_hook_active: bool,
    /// codex only: the turn's final assistant message
    #[serde(default)]
    last_assistant_message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ClaudeTool {
    #[serde(flatten)]
    base: ClaudeBase,
    #[serde(default)]
    tool_input: ToolInput,
}

#[derive(Debug, Default, Deserialize)]
struct ToolInput {
    /// NotebookEdit names it notebook_path
    #[serde(default, alias = "notebook_path")]
    file_path: Option<String>,
    /// codex apply_patch: the patch text
    #[serde(default)]
    command: Option<String>,
}

/// Paths named by an apply_patch body (`*** Add/Update/Delete File: p`,
/// `*** Move to: p`).
fn patch_files(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|l| {
            [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|h| l.strip_prefix(h))
        })
        .map(|p| p.trim().to_string())
        .collect()
}

/// Claude Code and Codex: same stdin fields, same reply JSON.
fn claude(event: &str, stdin: &str, client: &str) -> ExitCode {
    let client = Some(client.to_string());
    let codex = client.as_deref() == Some("codex");
    match event {
        "stop" => {
            let p: ClaudeStop = serde_json::from_str(stdin).unwrap_or_default();
            let e = Event {
                cwd: p.base.cwd,
                session: p.base.transcript_path.or(p.base.session_id),
                stop_active: p.stop_hook_active,
                // codex transcripts are not claude-format: always hand the
                // text over, so stop never falls back to parsing the file
                text: if codex {
                    Some(p.last_assistant_message.unwrap_or_default())
                } else {
                    None
                },
                client,
                ..Event::default()
            };
            if let Some(reason) = stop(&e).reason {
                println!(
                    "{}",
                    serde_json::json!({"decision": "block", "reason": reason})
                );
            }
            ExitCode::SUCCESS
        }
        "session-start" => {
            let p: ClaudeBase = serde_json::from_str(stdin).unwrap_or_default();
            let e = Event {
                cwd: p.cwd,
                session: p.session_id,
                client,
                ..Event::default()
            };
            if let Some(ctx) = session_start(&e).context {
                println!(
                    "{}",
                    serde_json::json!({"hookSpecificOutput": {
                        "hookEventName": "SessionStart", "additionalContext": ctx}})
                );
            }
            ExitCode::SUCCESS
        }
        "read" | "edit" => {
            let p: ClaudeTool = serde_json::from_str(stdin).unwrap_or_default();
            let e = Event {
                cwd: p.base.cwd,
                // same key as stop, which needs the transcript path
                session: p.base.transcript_path.or(p.base.session_id),
                files: match (codex, p.tool_input.command) {
                    (true, Some(patch)) => patch_files(&patch),
                    _ => p.tool_input.file_path.into_iter().collect(),
                },
                client,
                ..Event::default()
            };
            if let Some(ctx) = push(&e, event).context {
                println!(
                    "{}",
                    serde_json::json!({"hookSpecificOutput": {
                        "hookEventName": "PostToolUse", "additionalContext": ctx}})
                );
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("fael hook: unknown event {event:?} — want stop|session-start|read|edit");
            ExitCode::SUCCESS
        }
    }
}

// --- shared context ---

struct Ctx {
    repo: Repo,
    log: core::Log,
    client: String,
    session: String,
}

/// Resolve cwd → repo + log. `None` = fail open (not a repo, no cwd, …).
fn ctx(e: &Event) -> Option<Ctx> {
    let cwd = e
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    let repo = repo_at(&cwd).ok()?;
    let log = read(&repo);
    Some(Ctx {
        repo,
        log,
        client: e.client.clone().unwrap_or_else(|| "neutral".into()),
        session: e.session.clone().unwrap_or_default(),
    })
}

// --- stop ---

fn stop(e: &Event) -> Reply {
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
    for (path, at) in &recorded {
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

/// Apply `f` unless the entry is a `scheme:ref` anchor (anchors are opaque,
/// never filesystem paths).
fn unless_anchor(f: &str, resolve: impl FnOnce(&str) -> String) -> String {
    if f.contains("://") || !f.contains(':') {
        return resolve(f);
    }
    // cheap anchor check without reaching into core: scheme of ≥2 lower chars
    let scheme = f.split(':').next().unwrap_or("");
    if scheme.len() >= 2
        && scheme.as_bytes()[0].is_ascii_lowercase()
        && scheme
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'+' | b'.' | b'-'))
    {
        f.to_string()
    } else {
        resolve(f)
    }
}

/// Per-machine runtime state, never in `.fael/` (that is shared project data).
/// Keyed by session + worktree so one session across two repos stays apart.
// ponytail: no cleanup of old sessions — prune by mtime if the dir grows.
fn edits_path(session: &str, root: &Path) -> PathBuf {
    let key = session_key(&format!("{session}\0{}", root.to_string_lossy()));
    state_dir().join("sessions").join(format!("{key}.jsonl"))
}

/// One `{"path","at"}` line per edit event, in order — (path, at ms). A torn
/// or unreadable line is skipped; any error is an empty list.
fn session_edits(path: &Path) -> Vec<(String, i64)> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return vec![];
    };
    s.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            Some((
                v["path"].as_str()?.to_string(),
                core::ts_ms(v["at"].as_str()?)?,
            ))
        })
        .collect()
}

/// Append one line per file. Fails open like record_usage.
fn record_edits(path: &Path, files: &[String]) {
    let Some(at) = now_rfc3339() else { return };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        use std::io::Write;
        let body: String = files
            .iter()
            .map(|f| format!("{}\n", serde_json::json!({"path": f, "at": at})))
            .collect();
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(body.as_bytes()));
    }
}

/// Opaque filename for a session id or transcript path (paths are long).
fn session_key(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

// --- session-start ---

/// The one line that makes agents report (decision mugea7lt) — the hook only
/// catches what an agent says, this is what gets it said. Same line in the MCP
/// `add` description and the installed skill.
const ISSUE_LINE: &str = "- fael: saw something broken, inconsistent or likely to break? \
`fael add issue \"<what>\" --files <path>` right there — do not wait for the end of the task";

fn session_start(e: &Event) -> Reply {
    let no = || Reply {
        block: false,
        reason: None,
        context: None,
    };
    let c = match ctx(e) {
        Some(c) => c,
        None => return no(),
    };
    let rows = core::brief(&c.log, &Filter::default());
    let adopted = c.repo.fael.join("log").is_dir();
    let mut context = match (rows.is_empty(), adopted) {
        (true, false) => None,
        (true, true) => Some(format!("{ISSUE_LINE}\n")),
        (false, _) => Some(format!(
            "{}{ISSUE_LINE}\n",
            core::render(&c.log, &rows, c.repo.cfg.kickoff_tokens)
        )),
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
        &rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
    );
    Reply {
        block: false,
        reason: None,
        context: Some(context),
    }
}

fn check_ignore_hit(root: &Path) -> bool {
    std::process::Command::new("git")
        .args(["check-ignore", "-q", ".fael/log"])
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// --- read / edit push ---

fn push(e: &Event, event: &str) -> Reply {
    let no = || Reply {
        block: false,
        reason: None,
        context: None,
    };
    let c = match ctx(e) {
        Some(c) => c,
        None => return no(),
    };
    // normalize through core — outside the repo falls away, never errors out
    let mut files = vec![];
    for f in &e.files {
        // the client sends whatever the OS gave it (`/var/…` vs `/private/var/…`);
        // resolve symlinks while the repo root is already resolved, or the
        // lexical strip in normalize_files reads the file as outside the repo
        let f = unless_anchor(f, |f| {
            let p = if Path::new(f).is_absolute() {
                PathBuf::from(f)
            } else {
                c.repo.cwd.join(f)
            };
            std::fs::canonicalize(&p)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| f.to_string())
        });
        if let Ok(mut n) =
            core::normalize_files(std::slice::from_ref(&f), &c.repo.cwd, &c.repo.root)
        {
            files.append(&mut n);
        }
    }
    if files.is_empty() {
        return no();
    }
    // only adopted repos — stop never blocks without a log anyway
    if event == "edit" && !c.session.is_empty() && c.repo.fael.join("log").is_dir() {
        record_edits(&edits_path(&c.session, &c.repo.root), &files);
    }
    let rows = core::push(&c.log, &files);
    if rows.is_empty() {
        return no();
    }
    let body = core::render(&c.log, &rows, c.repo.cfg.push_tokens);
    let context = format!("fael mem for {}:\n{body}", files.join(", "));
    record_usage(
        &c.client,
        event,
        &c.repo.root,
        &context,
        &rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
    );
    Reply {
        block: false,
        reason: None,
        context: Some(context),
    }
}

// --- usage + stats (SPEC §8) ---

fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("FAEL_STATE_DIR")
        && !d.is_empty()
    {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/state/fael")
}

/// Every injection into context, per machine — never in git. Fails open:
/// a usage write never fails the command it rode along with.
fn record_usage(client: &str, event: &str, repo: &Path, text: &str, ids: &[String]) {
    let row = serde_json::json!({
        "ts": now_rfc3339().unwrap_or_default(),
        "repo": repo.to_string_lossy(),
        "client": client,
        "event": event,
        "bytes": text.len(),
        "est_tokens": core::est_tokens(text),
        "ids": ids,
    });
    let path = state_dir().join("usage.jsonl");
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{row}"));
    }
}

pub fn stats(json: bool) -> Result<(), String> {
    let path = state_dir().join("usage.jsonl");
    let Ok(s) = std::fs::read_to_string(&path) else {
        println!("fael: no usage recorded yet");
        return Ok(());
    };
    let mut n = 0usize;
    let (mut bytes, mut toks) = (0usize, 0usize);
    let mut by_event: HashMap<String, (usize, usize)> = HashMap::new();
    let mut by_client: HashMap<String, (usize, usize)> = HashMap::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    // (repo, "stop-work"|"stop-bug", ms) — checked against each repo's log below
    let mut blocks: Vec<(String, String, i64)> = vec![];
    for line in s.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        n += 1;
        let b = v["bytes"].as_u64().unwrap_or(0) as usize;
        let t = v["est_tokens"].as_u64().unwrap_or(0) as usize;
        bytes += b;
        toks += t;
        let ev = v["event"].as_str().unwrap_or("?").to_string();
        let cl = v["client"].as_str().unwrap_or("?").to_string();
        if ev.starts_with("stop-")
            && let (Some(repo), Some(ms)) =
                (v["repo"].as_str(), v["ts"].as_str().and_then(core::ts_ms))
        {
            blocks.push((repo.to_string(), ev.clone(), ms));
        }
        by_event
            .entry(ev)
            .and_modify(|e| {
                e.0 += 1;
                e.1 += t;
            })
            .or_insert((1, t));
        by_client
            .entry(cl)
            .and_modify(|e| {
                e.0 += 1;
                e.1 += t;
            })
            .or_insert((1, t));
        for id in v["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i.as_str())
        {
            *by_id.entry(id.into()).or_insert(0) += 1;
        }
    }
    if n == 0 {
        println!("fael: no usage recorded yet");
        return Ok(());
    }
    // did a row follow each block? work: any add/close · bug: an issue row.
    // ponytail: rows from anyone count — per-session attribution needs `session` on rows
    let mut logs: HashMap<String, core::Log> = HashMap::new();
    let mut outcome: HashMap<String, (usize, usize)> = HashMap::new();
    for (repo, ev, ms) in &blocks {
        let log = logs
            .entry(repo.clone())
            .or_insert_with(|| core::read(&Path::new(repo).join(".fael")));
        let followed = if ev == "stop-bug" {
            log.rows
                .iter()
                .any(|r| r.kind == "issue" && core::ts_ms(&r.ts).is_some_and(|t| t >= *ms))
        } else {
            core::last_row_ms(log, *ms).is_some()
        };
        let e = outcome.entry(ev.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 += followed as usize;
    }
    if json {
        let top: Vec<_> = {
            let mut v: Vec<_> = by_id.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            v.into_iter()
                .take(10)
                .map(|(id, c)| serde_json::json!({"id": id, "pushes": c}))
                .collect()
        };
        println!(
            "{}",
            serde_json::json!({
                "events": n, "bytes": bytes, "est_tokens": toks,
                "by_event": by_event.iter().map(|(k, (c, t))| (k.clone(), serde_json::json!({"events": c, "est_tokens": t}))).collect::<serde_json::Map<String,_>>(),
                "by_client": by_client.iter().map(|(k, (c, t))| (k.clone(), serde_json::json!({"events": c, "est_tokens": t}))).collect::<serde_json::Map<String,_>>(),
                "top_rows": top,
                "stop_blocks": outcome.iter().map(|(k, (b, f))| (k.clone(), serde_json::json!({"blocks": b, "followed_by_row": f}))).collect::<serde_json::Map<String,_>>(),
            })
        );
        return Ok(());
    }
    println!(
        "fael usage ({}): {} injections · {} bytes · ~{} tokens into context",
        path.display(),
        n,
        bytes,
        toks
    );
    let mut ev: Vec<_> = by_event.iter().collect();
    ev.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    for (k, (c, t)) in ev {
        println!("  {k}: ×{c} (~{t} tokens)");
    }
    let mut cl: Vec<_> = by_client.iter().collect();
    cl.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    for (k, (c, t)) in cl {
        println!("  client {k}: ×{c} (~{t} tokens)");
    }
    let mut ids: Vec<_> = by_id.iter().collect();
    ids.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (id, c) in ids.into_iter().take(10) {
        println!("  row {id}: pushed ×{c}");
    }
    let mut oc: Vec<_> = outcome.iter().collect();
    oc.sort();
    for (k, (b, f)) in oc {
        println!("  {k}: {b} block(s) → {f} followed by a row");
    }
    Ok(())
}

// --- bug markers (port of fapony's bug-markers.ts, std only) ---

/// Free-text announcement phrases, never symptom words — a false fire costs a
/// Stop-hook block. Ported without a regex crate to keep the hook path std +
/// serde_json only; the behaviour matches on the common phrases.
const BUG_PHRASES_LATIN: [&str; 12] = [
    "found a bug",
    "found the bug",
    "found bug",
    "found a real bug",
    "found the real bug",
    "this is a bug",
    "that is a bug",
    "it is a bug",
    "it's a bug",
    "bug confirmed",
    "confirmed bug",
    "confirmed a bug",
];

/// Thai phrases are caseless — they match in the lowercased haystack too.
const BUG_PHRASES_THAI: [&str; 5] = ["เจอบั๊ก", "พบว่าเป็นบั๊ก", "เจอว่าเป็นบั๊ก", "บั๊กที่เจอ", "บั๊กที่พบ"];

const NEGATIONS: [&str; 7] = ["ไม่", "จะ", "ถ้า", "อาจ", "not", "no", "if"];

/// Problems short of a confirmed bug — something inconsistent, or expected to
/// break. Reporting these on the spot is the point (an agent has no reason to
/// stay quiet), so a false fire is the accepted cost: one block per session.
/// "might"/"อาจ" are the claim here, not a negation — only a flat denial
/// ("no mismatch", "ไม่มีความเสี่ยง") cancels.
const RISK_PHRASES: [&str; 18] = [
    "inconsistent",
    "inconsistency",
    "mismatch",
    "doesn't match",
    "does not match",
    "out of sync",
    "might break",
    "could break",
    "will break",
    "likely to break",
    "ไม่ตรงกัน",
    "ไม่สอดคล้อง",
    "ขัดแย้งกัน",
    "อาจพัง",
    "น่าจะพัง",
    "อาจมีปัญหา",
    "น่าจะมีปัญหา",
    "มีความเสี่ยง",
];

const RISK_NEGATIONS: [&str; 3] = ["ไม่", "not", "no"];

/// The matched marker phrase, or `None`. Every match is checked against a
/// negation window so "ไม่พบบั๊กใหม่ แต่เจอบั๊กที่ X" still fires on the second.
/// The search runs on the lowercased text throughout, so byte indices always
/// belong to the string they slice.
fn has_bug_marker(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    // phrase lists
    for p in BUG_PHRASES_LATIN.into_iter().chain(BUG_PHRASES_THAI) {
        if let Some(i) = lower.find(p)
            && !negated(&lower, i, &NEGATIONS)
        {
            return Some(lower[i..i + p.len()].to_string());
        }
    }
    for p in RISK_PHRASES {
        if let Some(i) = lower.find(p)
            && !negated(&lower, i, &RISK_NEGATIONS)
        {
            return Some(p.to_string());
        }
    }
    // `bug…:` — "**Bug (cause…):**" (same line, optional paren group)
    let mut from = 0;
    while let Some(i) = lower[from..].find("bug") {
        let i = from + i;
        if word_boundary(&lower, i, 3)
            && colon_after(&lower, i + 3)
            && !negated(&lower, i, &NEGATIONS)
        {
            return Some("bug:".to_string());
        }
        from = i + 3;
    }
    None
}

fn word_boundary(s: &str, i: usize, len: usize) -> bool {
    let b = s.as_bytes();
    let left = i == 0 || !b[i - 1].is_ascii_alphanumeric();
    let right = b.get(i + len).is_none_or(|c| !c.is_ascii_alphanumeric());
    left && right
}

fn colon_after(s: &str, mut i: usize) -> bool {
    let b = s.as_bytes();
    while b.get(i).is_some_and(|c| *c == b' ' || *c == b'\t') {
        i += 1;
    }
    if b.get(i) == Some(&b'(') {
        // skip to the closing paren on this line
        while let Some(c) = b.get(i) {
            i += 1;
            if *c == b')' {
                break;
            }
            if *c == b'\n' {
                return false;
            }
        }
        while b.get(i).is_some_and(|c| *c == b' ' || *c == b'\t') {
            i += 1;
        }
    }
    // a colon before the line ends
    s[i..]
        .split('\n')
        .next()
        .is_some_and(|l| l.trim_end().ends_with(':'))
}

/// The ~15 chars before the match end in a negation word.
fn negated(lower: &str, i: usize, negations: &[&str]) -> bool {
    let before: String = lower[..i]
        .chars()
        .rev()
        .take(15)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let t = before.trim_end().to_lowercase();
    negations.iter().any(|n| {
        t.ends_with(n)
            && (n.chars().all(|c| !c.is_ascii_alphabetic())
                || t[..t.len() - n.len()]
                    .chars()
                    .last()
                    .is_none_or(|c| !c.is_alphabetic()))
    })
}

/// Scan assistant text in a Claude transcript tail for a bug marker. Reads
/// only the last 200 KB (and nothing over 10 MB) — the hook must not stall
/// turn-end. Returns the first matched phrase.
fn bug_signal_from_transcript(path: &Path, since_ms: i64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    if size > 10 * 1024 * 1024 {
        return None;
    }
    let tail = size.min(200 * 1024);
    f.seek(SeekFrom::Start(size - tail)).ok()?;
    let mut buf = vec![0u8; tail as usize];
    f.read_exact(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split('\n').peekable();
    if tail < size {
        lines.next(); // first line is partial
    }
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let m = &v["message"];
        if m["role"] != "assistant" {
            continue;
        }
        // Claude Code stamps each line at the top level, not inside `message`
        if let Some(created) = v["timestamp"].as_str()
            && let Some(ms) = core::ts_ms(created)
            && ms < since_ms
        {
            continue;
        }
        for b in m["content"].as_array().into_iter().flatten() {
            if b["type"] != "text" {
                continue;
            }
            if let Some(t) = b["text"].as_str()
                && let Some(hit) = has_bug_marker(t)
            {
                return Some(hit);
            }
        }
    }
    None
}

// --- tiny time helpers (std only: no chrono on the hook path) ---
// Parsing lives in core (`ts_ms`); formatting a unix-ms time too (`rfc3339`).

/// A file's birthtime (fallback: mtime) as unix ms.
fn file_birth_ms(path: &Path) -> Option<u64> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.created().or_else(|_| md.modified()).ok()?;
    Some(t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn now_rfc3339() -> Option<String> {
    systemtime_to_rfc3339(SystemTime::now())
}

fn systemtime_to_rfc3339(st: SystemTime) -> Option<String> {
    let ms = st.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(core::rfc3339(ms))
}

#[cfg(test)]
mod tests {
    use super::has_bug_marker;

    #[test]
    fn markers_catch_bugs_and_risks_not_denials() {
        for hit in [
            "I found a bug in login",
            "doc กับโค้ดไม่ตรงกัน",
            "config and schema are out of sync",
            "this might break the importer",
            "ตรงนี้อาจมีปัญหาตอน merge",
        ] {
            assert!(has_bug_marker(hit).is_some(), "{hit}");
        }
        for miss in [
            "no bug found",
            "no mismatch left",
            "ไม่มีความเสี่ยง",
            "ถ้าเจอบั๊กให้บอก",
            "all tests pass",
        ] {
            assert!(has_bug_marker(miss).is_none(), "{miss}");
        }
    }
}
