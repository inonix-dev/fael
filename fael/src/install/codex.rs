//! Codex client: MCP in `config.toml`, edited as text so comments and order
//! survive.

use super::Ctx;

pub(crate) fn codex_mcp(c: &Ctx) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::drop_toml_table;

    #[test]
    fn toml_table_drop() {
        let t = "a = 1\n[mcp_servers.fapony]\ncommand = \"bun\"\n[mcp_servers.fapony.env]\nX = \"1\"\n[mcp_servers.other]\nc = 2\n";
        assert_eq!(
            drop_toml_table(t, "mcp_servers.fapony"),
            "a = 1\n[mcp_servers.other]\nc = 2\n"
        );
    }
}
