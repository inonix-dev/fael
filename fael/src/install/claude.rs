//! Claude client: MCP through the CLI, never `~/.claude.json` by hand.

use super::{Ctx, on_path};
use std::process::Command;

pub(crate) fn claude_mcp(c: &Ctx) {
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
    // the path after `Command:` in `claude mcp get` — a fael binary there is ours to repoint
    let points_at_fael = |out: &str| {
        out.lines()
            .find_map(|l| l.trim().strip_prefix("Command:"))
            .is_some_and(|v| v.contains("fael"))
    };
    let remove = "claude mcp remove fael -s user";
    match get("fael") {
        Some(out) if out.contains(&c.exe) => println!("  mcp fael already set"),
        Some(out) if !points_at_fael(&out) => println!(
            "  ! an MCP server named fael points elsewhere — left alone; `{remove}` and rerun"
        ),
        Some(_) if c.dry => println!("  would run: {remove} && {add}"),
        Some(_) => {
            let ok = [
                &["mcp", "remove", "fael", "-s", "user"][..],
                &["mcp", "add", "fael", "-s", "user", "--", &c.exe, "mcp"],
            ]
            .iter()
            .all(|a| {
                Command::new("claude")
                    .args(*a)
                    .status()
                    .is_ok_and(|s| s.success())
            });
            println!(
                "  {}",
                if ok {
                    format!("ran: {remove} && {add} (repointed)")
                } else {
                    format!("! failed: {remove} && {add}")
                }
            );
        }
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
