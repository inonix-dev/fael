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
    // a JSON string is a valid TOML basic string
    let exe = serde_json::to_string(&c.exe).map_err(|e| e.to_string())?;
    if s.contains("[mcp_servers.fael]") {
        match toml_value(&s, "mcp_servers.fael", "command") {
            Some(r) if s[r.clone()] == exe => println!("  mcp fael already set"),
            Some(r) if s[r.clone()].contains("fael") => {
                s.replace_range(r, &exe);
                what.push("mcp_servers.fael (repointed)");
            }
            _ => println!(
                "  ! mcp_servers.fael in {} is not a fael binary — left alone",
                path.display()
            ),
        }
    } else {
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

/// Byte range of `key`'s value in `[table]` (not its sub-tables), so one line
/// changes and comments elsewhere survive.
// ponytail: a trailing `# comment` on that line counts as part of the value
fn toml_value(s: &str, table: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let (mut inside, mut at) = (false, 0);
    for line in s.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let t = line.trim_start();
        if t.starts_with('[') {
            let h = t.trim_start_matches('[').split(']').next().unwrap_or("");
            inside = h.trim() == table;
        } else if let Some((k, v)) = t.split_once('=').filter(|(k, _)| inside && k.trim() == key) {
            let v0 = start + (line.len() - t.len()) + k.len() + 1;
            let lead = v.len() - v.trim_start().len();
            return Some(v0 + lead..v0 + lead + v.trim().len());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{drop_toml_table, toml_value};

    #[test]
    fn toml_value_in_table_only() {
        let t = "command = \"x\"\n[mcp_servers.fael]\nargs = [\"mcp\"]\n  command = \"/old/fael\"\n[mcp_servers.fael.env]\ncommand = \"y\"\n";
        let r = toml_value(t, "mcp_servers.fael", "command").unwrap();
        assert_eq!(&t[r], "\"/old/fael\"");
        assert!(toml_value(t, "mcp_servers.other", "command").is_none());
    }

    #[test]
    fn toml_table_drop() {
        let t = "a = 1\n[mcp_servers.fapony]\ncommand = \"bun\"\n[mcp_servers.fapony.env]\nX = \"1\"\n[mcp_servers.other]\nc = 2\n";
        assert_eq!(
            drop_toml_table(t, "mcp_servers.fapony"),
            "a = 1\n[mcp_servers.other]\nc = 2\n"
        );
    }
}
