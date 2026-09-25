//! `claude` + `codex` adapters: the same stdin fields and reply JSON as the
//! neutral protocol, only parsed from each client's shape and rendered back
//! into it. Codex hands stop its last message and edits as apply_patch.

use super::protocol::Event;
use super::{push::push, session::session_start, stop::stop};
use serde::Deserialize;
use std::process::ExitCode;

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
pub(crate) fn run(event: &str, stdin: &str, client: &str) -> ExitCode {
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
