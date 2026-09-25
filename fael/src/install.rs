//! `fael install [--client claude|codex|opencode] [--dry-run] [--replace-fapony]`
//! Wires MCP + hooks + skill into every client found on this machine.
//! Idempotent: an entry already pointing at this binary stays, one pointing at
//! another fael binary is repointed, anything that is not fael's is never touched.
//! `--replace-fapony` also takes out fapony's Stop/session-start hooks and MCP —
//! left in, fapony's Stop blocks every turn that writes to `.fael/` instead of
//! `.fapony/`. Opt-in because fapony's hooks are user scope: they serve every
//! repo, including ones fael has not adopted yet.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const SKILL: &str = include_str!("../skill/SKILL.md");
const PLUGIN: &str = include_str!("../skill/opencode.js");
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

struct Ctx {
    home: PathBuf,
    exe: String,
    dry: bool,
    replace: bool,
}

impl Ctx {
    fn say(&self, what: &str, path: &Path) {
        let verb = if self.dry { "would write" } else { "wrote" };
        println!("  {verb} {what} → {}", path.display());
    }

    fn write(&self, path: &Path, body: &str) -> Result<(), String> {
        if self.dry {
            return Ok(());
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::write(path, body).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The hook command. Quoted only when the path needs it.
    fn command(&self, sub: &str, client: &str) -> String {
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
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("fael install: where am I? {e}"))?
        .to_string_lossy()
        .into_owned();
    // Windows canonicalize() gives `\\?\C:\...`, which cmd.exe can't run —
    // drop the prefix for drive paths (`\\?\UNC\` stays as-is)
    let exe = match exe.strip_prefix(r"\\?\") {
        Some(p) if p.as_bytes().get(1) == Some(&b':') => p.to_string(),
        _ => exe,
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
                claude_mcp(&c);
                hooks_json(
                    &c,
                    &c.home.join(".claude/settings.json"),
                    "claude",
                    CLAUDE_HOOKS,
                )?;
                skills.push(c.home.join(".claude/skills/fael/SKILL.md"));
            }
            "codex" => {
                codex_mcp(&c)?;
                hooks_json(&c, &c.home.join(".codex/hooks.json"), "codex", CODEX_HOOKS)?;
                println!("  trust the new hooks in Codex with /hooks before they run");
                skills.push(c.home.join(".agents/skills/fael/SKILL.md"));
            }
            _ => {
                opencode(&c)?;
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

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

// --- claude / codex hooks: the same {hooks: {Event: [{matcher?, hooks: [{type, command}]}]}} ---

fn is_fapony_blocker(cmd: &str) -> bool {
    cmd.contains("fapony") && (cmd.contains("hook-stop") || cmd.contains("hook-session-start"))
}

fn hooks_json(
    c: &Ctx,
    path: &Path,
    client: &str,
    want: &[(&str, Option<&str>, &str)],
) -> Result<(), String> {
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v @ Value::Object(_)) => v,
            _ => {
                println!(
                    "  ! {} is not a JSON object — left alone, hooks not installed",
                    path.display()
                );
                return Ok(());
            }
        },
        Err(_) => json!({}),
    };
    let Some(hooks) = root
        .as_object_mut()
        .map(|o| o.entry("hooks").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
    else {
        println!(
            "  ! {} has a non-object \"hooks\" — left alone",
            path.display()
        );
        return Ok(());
    };
    let mut changed = vec![];
    for (event, matcher, sub) in want {
        let cmd = c.command(sub, client);
        let suffix = format!(" hook {sub} --client {client}");
        let list = hooks.entry(*event).or_insert_with(|| json!([]));
        let Some(groups) = list.as_array_mut() else {
            continue;
        };
        // ours = a fael command for this event + client, wherever it sits
        let mine = groups
            .iter_mut()
            .filter_map(|g| g.get_mut("hooks").and_then(Value::as_array_mut))
            .flatten()
            .filter_map(|h| h.get_mut("command"))
            .find(|v| {
                v.as_str()
                    .is_some_and(|s| s.contains("fael") && s.ends_with(&suffix))
            });
        match mine {
            Some(v) if v == &json!(cmd) => {}
            Some(v) => {
                *v = json!(cmd);
                changed.push(format!("{event} (repointed)"));
            }
            None => {
                let mut g = Map::new();
                if let Some(m) = matcher {
                    g.insert("matcher".into(), json!(m));
                }
                g.insert("hooks".into(), json!([{"type": "command", "command": cmd}]));
                groups.push(Value::Object(g));
                changed.push(match matcher {
                    Some(m) => format!("{event}({m})"),
                    None => event.to_string(),
                });
            }
        }
    }
    // fapony's Stop looks for rows in .fapony/ — next to fael it blocks every turn
    let mut fapony = 0;
    for groups in hooks.values_mut().filter_map(Value::as_array_mut) {
        for g in groups.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                if c.replace {
                    hs.retain(|h| !h["command"].as_str().is_some_and(is_fapony_blocker));
                    fapony += before - hs.len();
                } else {
                    fapony += hs
                        .iter()
                        .filter(|h| h["command"].as_str().is_some_and(is_fapony_blocker))
                        .count();
                }
            }
        }
        if c.replace {
            groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
        }
    }
    if fapony > 0 && c.replace {
        changed.push(format!("removed {fapony} fapony hook(s)"));
    } else if fapony > 0 {
        println!(
            "  ! fapony's Stop/session-start hooks are still in {} — in a repo with .fael/ both block; \
             rerun with --replace-fapony once that repo's log is imported",
            path.display()
        );
    }
    if changed.is_empty() {
        println!("  hooks already set in {}", path.display());
        return Ok(());
    }
    let body = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())? + "\n";
    c.write(path, &body)?;
    c.say(&format!("hooks {}", changed.join(", ")), path);
    Ok(())
}

// --- claude MCP: through the CLI, never ~/.claude.json by hand ---

fn claude_mcp(c: &Ctx) {
    let add = format!("claude mcp add fael -s user -- {} mcp", c.exe);
    if !on_path("claude") {
        println!("  ! claude CLI not on PATH — add the MCP server yourself: {add}");
        return;
    }
    let get = |name: &str| {
        Command::new("claude")
            .args(["mcp", "get", name])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout).into_owned()
                    + &String::from_utf8_lossy(&o.stderr)
            })
    };
    match get("fael") {
        Some(out) if out.contains(&c.exe) => println!("  mcp fael already set"),
        Some(_) => println!(
            "  ! an MCP server named fael points elsewhere — left alone; `claude mcp remove fael -s user` and rerun"
        ),
        None if c.dry => println!("  would run: {add}"),
        None => {
            let ok = Command::new("claude")
                .args(["mcp", "add", "fael", "-s", "user", "--", &c.exe, "mcp"])
                .status()
                .is_ok_and(|s| s.success());
            println!(
                "  {}",
                if ok {
                    format!("ran: {add}")
                } else {
                    format!("! failed: {add}")
                }
            );
        }
    }
    if get("fapony").is_some() {
        if !c.replace {
            println!("  ! MCP fapony is still set — --replace-fapony removes it");
        } else if c.dry {
            println!("  would run: claude mcp remove fapony -s user");
        } else {
            let ok = Command::new("claude")
                .args(["mcp", "remove", "fapony", "-s", "user"])
                .status()
                .is_ok_and(|s| s.success());
            println!(
                "  {}",
                if ok {
                    "ran: claude mcp remove fapony -s user"
                } else {
                    "! failed: claude mcp remove fapony -s user"
                }
            );
        }
    }
}

// --- codex MCP: config.toml, edited as text so comments and order survive ---

fn codex_mcp(c: &Ctx) -> Result<(), String> {
    let path = c.home.join(".codex/config.toml");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut s = old.clone();
    let mut what = vec![];
    if c.replace && s.contains("[mcp_servers.fapony]") {
        s = drop_toml_table(&s, "mcp_servers.fapony");
        what.push("removed mcp_servers.fapony");
    } else if s.contains("[mcp_servers.fapony]") {
        println!(
            "  ! mcp_servers.fapony is still in {} — --replace-fapony removes it",
            path.display()
        );
    }
    if s.contains("[mcp_servers.fael]") {
        println!("  mcp fael already set");
    } else {
        // a JSON string is a valid TOML basic string
        let exe = serde_json::to_string(&c.exe).map_err(|e| e.to_string())?;
        s = format!(
            "{}\n\n[mcp_servers.fael]\ncommand = {exe}\nargs = [\"mcp\"]\n",
            s.trim_end()
        )
        .trim_start()
        .to_string();
        what.push("mcp_servers.fael");
    }
    if s != old {
        c.write(&path, &s)?;
        c.say(&what.join(", "), &path);
    }
    Ok(())
}

/// `[name]` and its `[name.*]` sub-tables, up to the next other header.
fn drop_toml_table(s: &str, name: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in s.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with('[') {
            let h = t
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim();
            skip = h == name || h.starts_with(&format!("{name}."));
        }
        if !skip {
            out.push_str(line);
        }
    }
    out
}

// --- opencode: MCP in opencode.json(c) + one in-process plugin ---

fn opencode(c: &Ctx) -> Result<(), String> {
    let dir = c.home.join(".config/opencode");
    let path = ["opencode.jsonc", "opencode.json"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
        .unwrap_or_else(|| dir.join("opencode.json"));
    let old = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}\n".into());
    let mut s = old.clone();
    let mut what = vec![];
    // ponytail: JSONC edited as text so comments survive. The probes are
    // substring checks — a key named "fael"/"mcp" inside a comment fools them.
    if s.contains("\"fael\":") {
        println!("  mcp fael already set");
    } else {
        let exe = serde_json::to_string(&c.exe).map_err(|e| e.to_string())?;
        let entry = format!("\"fael\": {{\"type\": \"local\", \"command\": [{exe}, \"mcp\"]}}");
        s = match s.find("\"mcp\"") {
            Some(i) => insert_first(&s, i, &entry),
            None => insert_first(&s, 0, &format!("\"mcp\": {{{entry}}}")),
        }
        .ok_or(format!(
            "{}: no JSON object to add mcp.fael to",
            path.display()
        ))?;
        what.push("mcp.fael");
    }
    const OFF: &str = "\"fapony\": {\"enabled\": false,";
    if let Some(i) = s.find("\"fapony\": {").filter(|_| !s.contains(OFF)) {
        if c.replace {
            s.replace_range(i..i + "\"fapony\": {".len(), OFF);
            what.push("disabled mcp.fapony");
        } else {
            println!(
                "  ! mcp.fapony is still on in {} — --replace-fapony disables it",
                path.display()
            );
        }
    }
    if s != old {
        c.write(&path, &s)?;
        c.say(&what.join(", "), &path);
    }
    // fapony's session-start plugin is the only one that competes (OpenCode has
    // no Stop): disable by rename, never delete
    let fp = dir.join("plugins/fapony-session-start.ts");
    if fp.is_file() {
        if c.replace {
            if !c.dry {
                std::fs::rename(&fp, fp.with_extension("ts.disabled"))
                    .map_err(|e| e.to_string())?;
            }
            println!(
                "  {} {} → .disabled",
                if c.dry { "would rename" } else { "renamed" },
                fp.display()
            );
        } else {
            println!(
                "  ! {} still runs — --replace-fapony disables it",
                fp.display()
            );
        }
    }
    let plugin = dir.join("plugins/fael.js");
    let body = PLUGIN.replace(
        "__FAEL__",
        &serde_json::to_string(&c.exe).map_err(|e| e.to_string())?,
    );
    match std::fs::read_to_string(&plugin) {
        Ok(cur) if cur == body => println!("  plugin already set"),
        Ok(cur) if !cur.starts_with("// fael — generated by `fael install`") => {
            println!("  ! {} is not fael's — left alone", plugin.display());
        }
        _ => {
            c.write(&plugin, &body)?;
            c.say("plugin (stop/session-start/read/edit)", &plugin);
        }
    }
    Ok(())
}

/// Put `entry` right after the first `{` at or after `from`, with a comma
/// only when the object is not empty.
fn insert_first(s: &str, from: usize, entry: &str) -> Option<String> {
    let brace = from + s[from..].find('{')? + 1;
    let empty = s[brace..].trim_start().starts_with('}');
    let comma = if empty { "" } else { "," };
    Some(format!("{}\n  {entry}{comma}{}", &s[..brace], &s[brace..]))
}

// --- skill ---

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_table_drop_and_jsonc_insert() {
        let t = "a = 1\n[mcp_servers.fapony]\ncommand = \"bun\"\n[mcp_servers.fapony.env]\nX = \"1\"\n[mcp_servers.other]\nc = 2\n";
        assert_eq!(
            drop_toml_table(t, "mcp_servers.fapony"),
            "a = 1\n[mcp_servers.other]\nc = 2\n"
        );
        assert_eq!(insert_first("{}", 0, "\"k\": 1").unwrap(), "{\n  \"k\": 1}");
        assert_eq!(
            insert_first("{ // c\n \"mcp\": { \"x\": {} } }", 8, "\"k\": 1").unwrap(),
            "{ // c\n \"mcp\": {\n  \"k\": 1, \"x\": {} } }"
        );
        assert!(insert_first("[]", 0, "x").is_none());
    }
}
