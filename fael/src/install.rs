//! `fael install [--client claude|codex|opencode] [--dry-run] [--replace-fapony]`
//! Wires MCP + hooks + skill into every client found on this machine.
//! Idempotent: an entry already pointing at this binary stays, one pointing at
//! another fael binary is repointed, anything that is not fael's is never touched.
//! `--replace-fapony` also takes out fapony's Stop/session-start hooks and MCP —
//! left in, fapony's Stop blocks every turn that writes to `.fael/` instead of
//! `.fapony/`. Opt-in because fapony's hooks are user scope: they serve every
//! repo, including ones fael has not adopted yet.
//!
//! Thin entry only — the clients live in `install/`:
//! `hooks` (the shared hooks-JSON shape), `claude`, `codex`, `opencode`.

mod claude;
mod codex;
mod hooks;
mod opencode;

use std::path::{Path, PathBuf};

const SKILL: &str = include_str!("../skill/SKILL.md");
const CLIENTS: [&str; 3] = ["claude", "codex", "opencode"];

/// (event, matcher, fael hook event)
const CLAUDE_HOOKS: &[(&str, Option<&str>, &str)] = &[
    ("Stop", None, "stop"),
    ("SessionStart", None, "session-start"),
    ("PostToolUse", Some("Read"), "read"),
    // every file-writing tool, or stop sees no edits (only the git fallback)
    (
        "PostToolUse",
        Some("Edit|Write|MultiEdit|NotebookEdit"),
        "edit",
    ),
];
/// Codex reads through the shell — no read hook; file edits are apply_patch.
const CODEX_HOOKS: &[(&str, Option<&str>, &str)] = &[
    ("Stop", None, "stop"),
    ("SessionStart", None, "session-start"),
    ("PostToolUse", Some("apply_patch|Edit|Write"), "edit"),
];

pub(crate) struct Ctx {
    pub(crate) home: PathBuf,
    pub(crate) exe: String,
    pub(crate) dry: bool,
    pub(crate) replace: bool,
}

impl Ctx {
    pub(crate) fn say(&self, what: &str, path: &Path) {
        let verb = if self.dry { "would write" } else { "wrote" };
        println!("  {verb} {what} → {}", path.display());
    }

    pub(crate) fn write(&self, path: &Path, body: &str) -> Result<(), String> {
        if self.dry {
            return Ok(());
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::write(path, body).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The hook command. Quoted only when the path needs it.
    pub(crate) fn command(&self, sub: &str, client: &str) -> String {
        let exe = if self.exe.contains([' ', '\'', '"']) {
            format!("'{}'", self.exe.replace('\'', r"'\''"))
        } else {
            self.exe.clone()
        };
        match client {
            "" => format!("{exe} hook {sub}"),
            c => format!("{exe} hook {sub} --client {c}"),
        }
    }
}

pub fn cmd(client: Option<String>, dry: bool, replace: bool) -> Result<(), String> {
    let home = crate::home().ok_or("fael install: cannot find the home directory")?;
    // Configs never use current_exe(): under npx that is a disposable cache dir.
    // ponytail: resolved from PATH now, so PATH at hook time must find it too
    let exe = match hook_exe() {
        Some(e) => e,
        None if dry => "fael".into(),
        None => return Err("fael install: fael is not on PATH, so the hooks could not run it — install it first: brew install zecalis/tap/fael, npm i -g @zecalis/fael or cargo install fael".into()),
    };
    let c = Ctx {
        home,
        exe,
        dry,
        replace,
    };
    let found = |name: &str| match name {
        "claude" => c.home.join(".claude").is_dir() || on_path("claude"),
        "codex" => c.home.join(".codex").is_dir(),
        _ => c.home.join(".config/opencode").is_dir(),
    };
    let targets: Vec<&str> = match client.as_deref() {
        Some(n) if CLIENTS.contains(&n) => vec![n],
        Some(n) => {
            return Err(format!(
                "fael install: unknown client {n:?} — want claude|codex|opencode"
            ));
        }
        None => CLIENTS.into_iter().filter(|n| found(n)).collect(),
    };
    if targets.is_empty() {
        return Err("fael install: found no Claude Code (~/.claude), Codex (~/.codex) or OpenCode (~/.config/opencode)".into());
    }
    if dry {
        println!("dry run — nothing is written");
    }
    let mut skills = vec![];
    for t in targets {
        println!("{t}");
        match t {
            "claude" => {
                claude::claude_mcp(&c);
                hooks::hooks_json(
                    &c,
                    &c.home.join(".claude/settings.json"),
                    "claude",
                    CLAUDE_HOOKS,
                )?;
                skills.push(c.home.join(".claude/skills/fael/SKILL.md"));
            }
            "codex" => {
                codex::codex_mcp(&c)?;
                hooks::hooks_json(&c, &c.home.join(".codex/hooks.json"), "codex", CODEX_HOOKS)?;
                println!("  trust the new hooks in Codex with /hooks before they run");
                skills.push(c.home.join(".agents/skills/fael/SKILL.md"));
            }
            _ => {
                opencode::opencode(&c)?;
                // OpenCode reads ~/.claude/skills too
                skills.push(c.home.join(".claude/skills/fael/SKILL.md"));
            }
        }
    }
    skills.sort();
    skills.dedup();
    for s in skills {
        skill(&c, &s)?;
    }
    Ok(())
}

/// The command client configs call: bare `fael`, except when PATH's `fael` is
/// npm's cargo-dist wrapper (run-fael.js, ~70 ms of Node per spawn vs ~3 ms) —
/// then the native binary it wraps, which stays put across `npm i -g` upgrades.
fn hook_exe() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let hit = std::env::split_paths(&path).find_map(|d| {
        ["fael", "fael.exe", "fael.cmd"]
            .into_iter()
            .map(|n| d.join(n))
            .find(|p| p.is_file())
    })?;
    let real = |pkg: &Path| {
        ["fael", "fael.exe"]
            .into_iter()
            .map(|n| pkg.join("node_modules/.bin_real").join(n))
            .find(|p| p.is_file())
    };
    let wrapped = hit
        .canonicalize()
        .ok()
        .filter(|t| t.file_name().is_some_and(|n| n == "run-fael.js"))
        .and_then(|t| t.parent().and_then(real))
        // Windows npm: shims sit in <prefix>/, packages in <prefix>/node_modules/
        .or_else(|| {
            hit.parent()
                .and_then(|d| real(&d.join("node_modules/@zecalis/fael")))
        });
    Some(wrapped.map_or("fael".into(), |p| p.to_string_lossy().into_owned()))
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

fn skill(c: &Ctx, path: &Path) -> Result<(), String> {
    match std::fs::read_to_string(path) {
        Ok(cur) if cur == SKILL => println!("skill already at {}", path.display()),
        Ok(cur) if !cur.contains("generated by `fael install`") => {
            println!("! {} is not fael's — left alone", path.display());
        }
        _ => {
            c.write(path, SKILL)?;
            let verb = if c.dry { "would write" } else { "wrote" };
            println!("{verb} skill → {}", path.display());
        }
    }
    Ok(())
}
