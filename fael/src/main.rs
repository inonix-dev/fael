//! fael CLI — add · close · find · keys · kickoff over fael-core.
//! Output for people and agents is one markdown line per row, cut to a token budget;
//! `--json` prints one JSON row per line, uncut, for programs. `fael mcp` serves the same
//! add/close/find over stdio (see mcp.rs).

mod aliases;
mod find;
mod hook;
mod install;
mod maintain;
mod mcp;
mod write;

use fael_core::{self as core, Config, Log, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage:
  fael add <kind> \"<text>\" [--files a,b] [--key k] [--title t] [--to who] [--urgent|--urgent-before id] [--supersedes id] [--force]
      (no --files = the files this session edited, as the edit hook recorded;
       --title = the ≤15-word headline lists show, the body is pulled by id;
       --force files a path that looks like a typo of an existing one)
  fael close <id> \"<why>\"
  fael bump <id> [--to who] [--urgent|--urgent-before id|--not-urgent]
      (same text/files, new version — text and files never change through bump)
  fael find [text|id] [--files a,b] [--key glob] [--kind k] [--since yyyy-mm[-dd]] [--by writer] [--to who] [--all] [--full] [--limit N] [--offset M]
      (an exact id or unique prefix pulls that row's body; --full shows every body;
       a cut list prints the exact next call — rerun it with the new --offset)
  fael keys [glob]
  fael kickoff [file|anchor] [--full] [--limit N] [--offset M]
  fael mv <old> <new>           record a move git can't see (anchors, uncommitted rewrites)
  fael hook <stop|session-start|read|edit> [--client c]   stdin in, stdout out; always exits 0
  fael stats                  tokens fael has put into context, per machine
  fael doctor [--fix]
  fael compact [--writer id] [--before yyyy-mm] [--prune]
  fael import <path> [--map old/=new/]
  fael mcp                      MCP server on stdio
  fael install [--client claude|codex|opencode] [--dry-run] [--replace-fapony]
  fael help | fael --help | fael <cmd> --help
  fael --version
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
    if matches!(argv.as_slice(), [v] if v == "--version" || v == "-V") {
        println!("fael {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    // `fael help`, `fael --help`, `fael <cmd> --help` — usage on stdout, exit 0
    if argv.first().is_some_and(|c| c == "help") || argv.iter().any(|x| x == "--help" || x == "-h")
    {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let a = Args::parse(argv)?;
    let cmd = a.pos.first().map(String::as_str).unwrap_or("");
    let rest = a.pos.get(1..).unwrap_or_default();
    match (cmd, rest) {
        ("add", [kind, text]) => add(&a, kind, text).map(|()| ExitCode::SUCCESS),
        ("close", [id, why]) => close(&a, id, why).map(|()| ExitCode::SUCCESS),
        ("bump", [id]) => bump(&a, id).map(|()| ExitCode::SUCCESS),
        ("find", [] | [_]) => find::find(&a, rest.first()).map(|()| ExitCode::SUCCESS),
        ("keys", [] | [_]) => find::keys(&a, rest.first()).map(|()| ExitCode::SUCCESS),
        ("kickoff", [] | [_]) => find::kickoff(&a, rest.first()).map(|()| ExitCode::SUCCESS),
        ("mv", [old, new]) => mv(&a, old, new).map(|()| ExitCode::SUCCESS),
        ("hook", [event]) => Ok(hook::cmd(event, a.one("client"))),
        ("stats", []) => hook::stats(a.has("json")).map(|()| ExitCode::SUCCESS),
        ("doctor", []) => maintain::doctor(&a),
        ("compact", []) => maintain::compact(&a),
        ("import", [src]) => maintain::import(&a, src),
        ("mcp", []) => mcp::serve().map(|()| ExitCode::SUCCESS),
        ("install", []) => install::cmd(a.one("client"), a.has("dry-run"), a.has("replace-fapony"))
            .map(|()| ExitCode::SUCCESS),
        _ => Err(USAGE.into()),
    }
}

/// Positionals + `--flag value` / `--flag=value`; `--` ends flags.
pub(crate) struct Args {
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
                "all" | "force" | "json" | "dry-run" | "replace-fapony" | "fix" | "prune"
                | "urgent" | "not-urgent" | "full" => {
                    a.flags.entry(name).or_default();
                }
                "files" | "key" | "supersedes" | "kind" | "since" | "by" | "client" | "writer"
                | "before" | "map" | "to" | "title" | "urgent-before" | "limit" | "offset" => {
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

    pub(crate) fn has(&self, f: &str) -> bool {
        self.flags.contains_key(f)
    }

    pub(crate) fn one(&self, f: &str) -> Option<String> {
        self.flags.get(f).and_then(|v| v.last()).cloned()
    }

    /// `--map a=b --map c=d` → all values, in order.
    fn many(&self, f: &str) -> Vec<String> {
        self.flags.get(f).cloned().unwrap_or_default()
    }

    /// `--files a,b --files c` → [a, b, c]
    pub(crate) fn files(&self) -> Vec<String> {
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

    /// `--limit N` / `--offset M` for pull paging (chunk 5): at most N ranked
    /// rows, skipping M first. Offset without limit pages budget cuts too.
    pub(crate) fn paging(&self) -> Result<(Option<usize>, usize), String> {
        let num = |f: &str| match self.one(f) {
            None => Ok(None),
            Some(v) => v
                .parse::<usize>()
                .map(Some)
                .map_err(|_| format!("rejected: --{f} needs a number — got {v:?}")),
        };
        let limit = num("limit")?;
        if limit == Some(0) {
            return Err("rejected: --limit 0 shows nothing — drop it or give 1 or more".into());
        }
        Ok((limit, num("offset")?.unwrap_or(0)))
    }

    /// Rebuild this `find`/`kickoff` call for the cut line: the same filters,
    /// so the agent reruns it with the new `--offset` the renderer appends.
    /// Kickoff takes no filter flags, only `--full` and `--limit`.
    pub(crate) fn page_base(
        &self,
        cmd: &str,
        positional: Option<&str>,
        limit: Option<usize>,
    ) -> String {
        let mut s = format!("fael {cmd}");
        if let Some(p) = positional.filter(|p| !p.is_empty()) {
            s.push_str(&format!(" {}", quoted(p)));
        }
        if cmd == "find" {
            let fs = self.files();
            if !fs.is_empty() {
                s.push_str(&format!(" --files {}", quoted(&fs.join(","))));
            }
            for f in ["key", "kind", "since", "by", "to"] {
                if let Some(v) = self.one(f) {
                    s.push_str(&format!(" --{f} {}", quoted(&v)));
                }
            }
            if self.has("all") {
                s.push_str(" --all");
            }
        }
        if self.has("full") {
            s.push_str(" --full");
        }
        if let Some(n) = limit {
            s.push_str(&format!(" --limit {n}"));
        }
        s
    }
}

/// Quote only when the shell would need it — `--kind issue` stays bare, a
/// glob (`--key auth:*`) or `$x` is single-quoted so the shell passes it as-is.
fn quoted(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || "-_./:,@+=".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
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
    let find = |name: &str| {
        cwd.ancestors()
            .find(|d| d.join(name).exists())
            .map(Path::to_path_buf)
    };
    let root = find(".git")
        .or_else(|| find(".fael"))
        .unwrap_or_else(|| cwd.clone());
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

/// `.fael/config.toml` — missing file = defaults, a broken file = error.
/// The user's home: `HOME` first (git and Git Bash on Windows honour it too,
/// and tests set it), else the OS answer (`USERPROFILE` on Windows).
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
}

fn config(path: &Path) -> Result<Config, String> {
    let Ok(s) = std::fs::read_to_string(path) else {
        return Ok(Config::default());
    };
    Config::from_toml(&s).map_err(|e| format!("{}: {e}", path.display()))
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
/// `pub(crate)` — the session-start hook matches `--to` against it.
pub(crate) fn writer(r: &Repo) -> String {
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

/// Writer, branch and sha from git — never asked of the agent.
fn stamp(r: &Repo) -> core::Stamp {
    core::Stamp {
        by: writer(r),
        branch: git(&r.root, &["symbolic-ref", "--short", "-q", "HEAD"]),
        sha: git(&r.root, &["rev-parse", "--short", "HEAD"]),
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
    let urgent = match (a.has("urgent"), a.one("urgent-before")) {
        (false, None) => core::Urgent::Unset,
        (true, None) => core::Urgent::End,
        (false, Some(t)) => core::Urgent::Before(t),
        (true, Some(_)) => {
            return Err("rejected: --urgent and --urgent-before pick one — the queue takes a single position".into());
        }
    };
    let (row, path, warns) = write::add_row(
        &r,
        kind,
        text,
        &a.files(),
        write::AddOpts {
            key: a.one("key"),
            to: a.one("to"),
            title: a.one("title"),
            urgent,
            supersedes: a.one("supersedes"),
            force: a.has("force"),
        },
    )?;
    warns.iter().for_each(|w| eprintln!("{w}"));
    written(a, &r, &row, &path);
    Ok(())
}

fn close(a: &Args, id: &str, why: &str) -> Result<(), String> {
    let r = repo()?;
    let (row, path, warns) = close_row(&r, id, why)?;
    warns.iter().for_each(|w| eprintln!("{w}"));
    written(a, &r, &row, &path);
    Ok(())
}

fn close_row(r: &Repo, id: &str, why: &str) -> Result<(Row, PathBuf, Vec<String>), String> {
    core::close_row(&r.fael, &read(r), &r.cfg, &stamp(r), id, why)
}

/// `fael bump` — same text/files, new `to`/`urgent` (see write::bump).
fn bump(a: &Args, id: &str) -> Result<(), String> {
    let r = repo()?;
    let (row, path, warns) = write::bump(&r, a, id)?;
    warns.iter().for_each(|w| eprintln!("{w}"));
    written(a, &r, &row, &path);
    Ok(())
}

/// Record that `old` moved to `new` — for what git can't see (anchors,
/// uncommitted rewrites, repos without git). Appends an alias row; the log
/// stays append-only, nothing is rewritten.
fn mv(a: &Args, old: &str, new: &str) -> Result<(), String> {
    let r = repo()?;
    let norm = core::normalize_files(&[old.to_string(), new.to_string()], &r.cwd, &r.root)?;
    let (from, to) = (&norm[0], &norm[1]);
    if from == to {
        return Err(format!(
            "rejected: {from:?} is already itself — `fael mv` needs two different paths"
        ));
    }
    let log = read(&r);
    if core::Aliases::from_log(&log).forward(from).contains(to) {
        return Err(format!(
            "rejected: {from} → {to} is already recorded — `fael find --files {to}` shows the rows"
        ));
    }
    let (row, _) = core::mv_row(&r.fael, &r.cfg, &stamp(&r), from, to)?;
    if a.has("json") {
        println!("{}", row.to_line());
    } else {
        println!("{} → {from} → {to}", row.id);
    }
    Ok(())
}
