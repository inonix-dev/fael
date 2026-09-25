//! fael CLI — add · close · find · keys · kickoff over fael-core.
//! Output for people and agents is one markdown line per row, cut to a token budget;
//! `--json` prints one JSON row per line, uncut, for programs. `fael mcp` serves the same
//! add/close/find over stdio (see mcp.rs).

mod hook;
mod mcp;

use fael_core::{self as core, Config, Filter, Log, Row};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage:
  fael add <kind> \"<text>\" --files a,b [--key k] [--supersedes id]
  fael close <id> \"<why>\"
  fael find [text] [--files a,b] [--key glob] [--kind k] [--since yyyy-mm[-dd]] [--by writer] [--all]
  fael keys [glob]
   fael kickoff [file|anchor]
   fael hook <stop|session-start|read|edit> [--client c]   stdin in, stdout out; always exits 0
   fael stats                  tokens fael has put into context, per machine
   fael mcp                      MCP server on stdio
 every command takes --json";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(argv: Vec<String>) -> Result<ExitCode, String> {
    let a = Args::parse(argv)?;
    let cmd = a.pos.first().map(String::as_str).unwrap_or("");
    let rest = a.pos.get(1..).unwrap_or_default();
    match (cmd, rest) {
        ("add", [kind, text]) => add(&a, kind, text).map(|()| ExitCode::SUCCESS),
        ("close", [id, why]) => close(&a, id, why).map(|()| ExitCode::SUCCESS),
        ("find", [] | [_]) => find(&a, rest.first()).map(|()| ExitCode::SUCCESS),
        ("keys", [] | [_]) => keys(&a, rest.first()).map(|()| ExitCode::SUCCESS),
        ("kickoff", [] | [_]) => kickoff(&a, rest.first()).map(|()| ExitCode::SUCCESS),
        ("hook", [event]) => Ok(hook::cmd(event, a.one("client"))),
        ("stats", []) => hook::stats(a.has("json")).map(|()| ExitCode::SUCCESS),
        ("mcp", []) => mcp::serve().map(|()| ExitCode::SUCCESS),
        _ => Err(USAGE.into()),
    }
}

/// Positionals + `--flag value` / `--flag=value`; `--` ends flags.
struct Args {
    pos: Vec<String>,
    flags: HashMap<String, Vec<String>>,
}

impl Args {
    fn parse(argv: Vec<String>) -> Result<Args, String> {
        let mut a = Args {
            pos: vec![],
            flags: HashMap::new(),
        };
        let mut it = argv.into_iter();
        while let Some(s) = it.next() {
            if s == "--" {
                a.pos.extend(it.by_ref());
                break;
            }
            let Some(name) = s.strip_prefix("--") else {
                a.pos.push(s);
                continue;
            };
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (name.to_string(), None),
            };
            match name.as_str() {
                "all" | "json" => {
                    a.flags.entry(name).or_default();
                }
                "files" | "key" | "supersedes" | "kind" | "since" | "by" | "client" => {
                    let v = inline
                        .or_else(|| it.next())
                        .ok_or(format!("--{name} needs a value"))?;
                    a.flags.entry(name).or_default().push(v);
                }
                _ => return Err(format!("unknown flag --{name}\n{USAGE}")),
            }
        }
        Ok(a)
    }

    fn has(&self, f: &str) -> bool {
        self.flags.contains_key(f)
    }

    fn one(&self, f: &str) -> Option<String> {
        self.flags.get(f).and_then(|v| v.last()).cloned()
    }

    /// `--files a,b --files c` → [a, b, c]
    fn files(&self) -> Vec<String> {
        self.flags
            .get("files")
            .into_iter()
            .flatten()
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
}

pub(crate) struct Repo {
    pub(crate) root: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) fael: PathBuf,
    pub(crate) cfg: Config,
}

pub(crate) fn repo() -> Result<Repo, String> {
    let cwd = std::env::current_dir()
        .and_then(|d| d.canonicalize())
        .map_err(|e| format!("cwd: {e}"))?;
    repo_at(&cwd)
}

/// The same resolution from an explicit directory — what hooks pass.
pub(crate) fn repo_at(cwd: &Path) -> Result<Repo, String> {
    // normalize_files is lexical, so root must be symlink-resolved like cwd
    let cwd = cwd.canonicalize().map_err(|e| format!("cwd: {e}"))?;
    // ponytail: walk up for `.git` (dir, or file for worktree/submodule) instead
    // of spawning `git rev-parse` — that spawn was ~13 of a hook's ~15 ms.
    // Ignores GIT_DIR/GIT_WORK_TREE/core.worktree; spawn git if those matter.
    let find = |name: &str| cwd.ancestors().find(|d| d.join(name).exists()).map(Path::to_path_buf);
    let root = find(".git").or_else(|| find(".fael")).unwrap_or_else(|| cwd.clone());
    let fael = root.join(".fael");
    let cfg = config(&fael.join("config.toml"))?;
    Ok(Repo {
        root,
        cwd,
        fael,
        cfg,
    })
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !s.is_empty()).then_some(s)
}

/// `.fael/config.toml` — every field optional; missing file = defaults, a broken file = error.
fn config(path: &Path) -> Result<Config, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct File {
        kinds: Vec<String>,
        key_domains: Vec<String>,
        budget: Budget,
        warn: Warn,
        limit: Limit,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Budget {
        kickoff_tokens: Option<usize>,
        find_tokens: Option<usize>,
        push_tokens: Option<usize>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Warn {
        row_tokens: Option<usize>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Limit {
        row_bytes: Option<usize>,
    }
    let Ok(s) = std::fs::read_to_string(path) else {
        return Ok(Config::default());
    };
    let f: File = toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
    let d = Config::default();
    Ok(Config {
        kinds: f.kinds,
        key_domains: f.key_domains,
        row_bytes: f.limit.row_bytes.unwrap_or(d.row_bytes),
        kickoff_tokens: f.budget.kickoff_tokens.unwrap_or(d.kickoff_tokens),
        find_tokens: f.budget.find_tokens.unwrap_or(d.find_tokens),
        push_tokens: f.budget.push_tokens.unwrap_or(d.push_tokens),
        warn_row_tokens: f.warn.row_tokens.unwrap_or(d.warn_row_tokens),
    })
}

/// Read the log; skipped lines go to stderr as one summary, never fail the command.
pub(crate) fn read(r: &Repo) -> Log {
    let log = core::read(&r.fael);
    if let Some(first) = log.warnings.first() {
        eprintln!(
            "fael: {} log line(s) skipped — first: {first}",
            log.warnings.len()
        );
    }
    log
}

/// Writer id from git identity; no email → hostname hash, with a warning.
fn writer(r: &Repo) -> String {
    let name = git(&r.root, &["config", "user.name"]).unwrap_or_default();
    let email = git(&r.root, &["config", "user.email"]);
    let host = Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if email.is_none() {
        eprintln!("fael: git user.email is not set — writer id hashes the hostname instead");
    }
    core::writer_id(&name, email.as_deref(), &host)
}

/// `branch` and `sha` are filled in here — never asked of the agent.
fn stamp(row: &mut Row, r: &Repo) {
    if let Some(b) = git(&r.root, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        row.extra.insert("branch".into(), b.into());
    }
    if let Some(s) = git(&r.root, &["rev-parse", "--short", "HEAD"]) {
        row.extra.insert("sha".into(), s.into());
    }
}

fn written(a: &Args, r: &Repo, row: &Row, path: &Path) {
    if a.has("json") {
        println!("{}", row.to_line());
    } else {
        let rel = path.strip_prefix(&r.root).unwrap_or(path);
        println!("{} → {}", row.id, rel.display());
    }
}

fn add(a: &Args, kind: &str, text: &str) -> Result<(), String> {
    let r = repo()?;
    let (row, path, warns) = add_row(
        &r,
        kind,
        text,
        &a.files(),
        a.one("key"),
        a.one("supersedes"),
    )?;
    warns.iter().for_each(|w| eprintln!("{w}"));
    written(a, &r, &row, &path);
    Ok(())
}

/// Normalise, stamp, validate, append — shared by the CLI and MCP. Returns the non-fatal warnings.
fn add_row(
    r: &Repo,
    kind: &str,
    text: &str,
    files: &[String],
    key: Option<String>,
    supersedes: Option<String>,
) -> Result<(Row, PathBuf, Vec<String>), String> {
    let files = core::normalize_files(files, &r.cwd, &r.root)?;
    let mut row = Row::new(&writer(r), kind, text, files);
    row.key = key;
    let log = read(r);
    if let Some(s) = supersedes {
        row.supersedes = Some(core::resolve(&log, &s)?.id.clone());
    }
    stamp(&mut row, r);
    let path = core::add(&r.fael, &row, &r.cfg)?;
    let warns = core::warnings(&row, &log, &r.cfg);
    Ok((row, path, warns))
}

fn close(a: &Args, id: &str, why: &str) -> Result<(), String> {
    let r = repo()?;
    let (row, path, warns) = close_row(&r, id, why)?;
    warns.iter().for_each(|w| eprintln!("{w}"));
    written(a, &r, &row, &path);
    Ok(())
}

fn close_row(r: &Repo, id: &str, why: &str) -> Result<(Row, PathBuf, Vec<String>), String> {
    let log = read(r);
    let target = core::resolve(&log, id)?;
    let mut warns = vec![];
    if core::closed(&log).contains(target.id.as_str()) {
        warns.push(format!(
            "fael: {} is already closed — closing it again",
            target.id
        ));
    }
    let mut row = Row::close(&writer(r), &target.id, why);
    stamp(&mut row, r);
    let path = core::close(&r.fael, &row, &r.cfg)?;
    Ok((row, path, warns))
}

fn find(a: &Args, text: Option<&String>) -> Result<(), String> {
    let r = repo()?;
    let f = Filter {
        text: text.cloned(),
        files: core::normalize_files(&a.files(), &r.cwd, &r.root)?,
        key: a.one("key"),
        kind: a.one("kind"),
        since: a.one("since"),
        by: a.one("by"),
        all: a.has("all"),
    };
    let log = read(&r);
    let (rows, budget) = query(&r, &log, &f);
    show(a, &log, &rows, budget)
}

/// No filter = the session brief under the kickoff budget; otherwise find under the find budget.
fn query<'a>(r: &Repo, log: &'a Log, f: &Filter) -> (Vec<&'a Row>, usize) {
    if f.is_empty() && !f.all {
        (core::brief(log, f), r.cfg.kickoff_tokens)
    } else {
        (core::find(log, f), r.cfg.find_tokens)
    }
}

fn kickoff(a: &Args, anchor: Option<&String>) -> Result<(), String> {
    let r = repo()?;
    let f = Filter {
        files: core::normalize_files(&Vec::from_iter(anchor.cloned()), &r.cwd, &r.root)?,
        ..Filter::default()
    };
    let log = read(&r);
    show(a, &log, &core::brief(&log, &f), r.cfg.kickoff_tokens)
}

fn show(a: &Args, log: &Log, rows: &[&Row], budget: usize) -> Result<(), String> {
    if rows.is_empty() {
        eprintln!("fael: no rows match");
    } else if a.has("json") {
        rows.iter().for_each(|r| println!("{}", r.to_line()));
    } else {
        print!("{}", core::render(log, rows, budget));
    }
    Ok(())
}

fn keys(a: &Args, pattern: Option<&String>) -> Result<(), String> {
    let r = repo()?;
    let log = read(&r);
    let ks = core::keys(&log, pattern.map(String::as_str));
    if ks.is_empty() {
        eprintln!("fael: no keys yet");
    }
    for k in ks {
        if a.has("json") {
            println!(
                "{}",
                serde_json::json!({"key": k.key, "count": k.count, "last": k.last})
            );
        } else {
            println!(
                "- {} ×{} (last {})",
                k.key,
                k.count,
                k.last.get(..10).unwrap_or(&k.last)
            );
        }
    }
    Ok(())
}
