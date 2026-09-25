//! Per-client wiring: MCP entries, hook tables, the in-process plugin.
//!
//! Thin entry only — the clients live in `install/`:
//! `hooks` (the shared hooks-JSON shape), `claude`, `codex`, `opencode`.

use super::Ctx;
use serde_json::{Map, Value, json};
use std::path::Path;

// --- claude / codex hooks: the same {hooks: {Event: [{matcher?, hooks: [{type, command}]}]}} ---

pub(crate) fn is_fapony_blocker(cmd: &str) -> bool {
    cmd.contains("fapony") && (cmd.contains("hook-stop") || cmd.contains("hook-session-start"))
}

pub(crate) fn hooks_json(
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
