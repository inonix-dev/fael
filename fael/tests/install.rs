//! `fael install` against a throwaway HOME with fapony already installed.

use std::path::Path;
use std::process::Command;

fn install(home: &Path, args: &[&str]) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_fael"))
        .arg("install")
        .args(args)
        .env("HOME", home)
        // fael on PATH (install refuses without it), no claude CLI: MCP is printed, not run
        .env(
            "PATH",
            std::env::join_paths([
                Path::new(env!("CARGO_BIN_EXE_fael")).parent().unwrap(),
                Path::new("/usr/bin"),
                Path::new("/bin"),
            ])
            .unwrap(),
        )
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

#[test]
fn install_all_three_idempotent_and_replaces_fapony_on_request() {
    let home = std::env::temp_dir().join(format!("fael-install-{}", fael_core::ulid()));
    let claude = home.join(".claude/settings.json");
    let codex_hooks = home.join(".codex/hooks.json");
    let codex_cfg = home.join(".codex/config.toml");
    let oc = home.join(".config/opencode/opencode.jsonc");
    std::fs::create_dir_all(home.join(".config/opencode/plugins")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let fapony = r#"{"theme": "dark", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "bun /x/fapony.ts hook-stop"}]}],
      "PreToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "bun /x/fapony.ts hook-read-hint"}]}]}}"#;
    std::fs::write(&claude, fapony).unwrap();
    std::fs::write(&codex_hooks, fapony).unwrap();
    std::fs::write(
        &codex_cfg,
        "model = \"x\"\n\n[mcp_servers.fapony]\ncommand = \"bun\"\n",
    )
    .unwrap();
    std::fs::write(
        &oc,
        "{\n  // mine\n  \"mcp\": {\n    \"fapony\": {\"type\": \"local\"}\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".config/opencode/plugins/fapony-session-start.ts"),
        "",
    )
    .unwrap();

    // dry run writes nothing
    let out = install(&home, &["--dry-run"]);
    assert!(
        out.contains("would write") && out.contains("--replace-fapony"),
        "{out}"
    );
    assert_eq!(read(&claude), fapony);
    assert!(!home.join(".claude/skills/fael/SKILL.md").exists());

    // real run: fael wired everywhere, fapony left in place and warned about
    let out = install(&home, &[]);
    assert!(out.contains("! fapony's Stop"), "{out}");
    let s = read(&claude);
    assert!(s.starts_with("{\n  \"theme\""), "key order kept: {s}");
    for sub in ["stop", "session-start", "read", "edit"] {
        assert!(
            s.contains(&format!("\"fael hook {sub} --client claude\"")),
            "{sub}: {s}"
        );
    }
    assert!(
        s.contains("Edit|Write|MultiEdit|NotebookEdit") && s.contains("hook-stop"),
        "{s}"
    );
    let h = read(&codex_hooks);
    assert!(
        h.contains("hook edit --client codex") && !h.contains("hook read --client codex"),
        "{h}"
    );
    let t = read(&codex_cfg);
    assert!(
        t.contains("[mcp_servers.fael]\ncommand = \"")
            && !t.contains(r"\\?\")
            && t.contains("[mcp_servers.fapony]"),
        "{t}"
    );
    let o = read(&oc);
    assert!(
        o.contains("// mine") && o.contains("\"fael\": {\"type\": \"local\""),
        "{o}"
    );
    let p = read(&home.join(".config/opencode/plugins/fael.js"));
    assert!(
        p.contains("const FAEL = \"") && !p.contains(r"\\?\") && !p.contains("__FAEL__"),
        "{p}"
    );
    assert!(read(&home.join(".claude/skills/fael/SKILL.md")).contains("fael add issue"));
    assert!(home.join(".agents/skills/fael/SKILL.md").is_file());

    // second run changes nothing
    let before = [
        read(&claude),
        read(&codex_hooks),
        read(&codex_cfg),
        read(&oc),
    ];
    let out = install(&home, &[]);
    assert!(!out.contains("wrote"), "{out}");
    assert_eq!(
        before,
        [
            read(&claude),
            read(&codex_hooks),
            read(&codex_cfg),
            read(&oc)
        ]
    );

    // --replace-fapony: blockers + MCP out, everything else of fapony stays
    install(&home, &["--replace-fapony"]);
    let s = read(&claude);
    assert!(
        !s.contains("hook-stop")
            && s.contains("hook-read-hint")
            && s.contains("hook stop --client claude"),
        "{s}"
    );
    assert!(!read(&codex_hooks).contains("hook-stop"));
    let t = read(&codex_cfg);
    assert!(
        !t.contains("fapony") && t.contains("[mcp_servers.fael]"),
        "{t}"
    );
    assert!(read(&oc).contains("\"fapony\": {\"enabled\": false,"));
    assert!(
        home.join(".config/opencode/plugins/fapony-session-start.ts.disabled")
            .is_file()
    );
}

/// Configs call bare `fael`, so install refuses when PATH cannot find it
/// (e.g. run through npx) instead of wiring hooks that silently never run.
#[test]
fn install_refuses_when_fael_not_on_path() {
    let home = std::env::temp_dir().join(format!("fael-install-{}", fael_core::ulid()));
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_fael"))
        .arg("install")
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("not on PATH"));
    assert!(!home.join(".claude/settings.json").exists());
}

/// Native Windows has no HOME (only USERPROFILE) — install must still find
/// the home dir. Dry run: nothing is written to the real home. A machine with
/// no client installed (CI) still fails later with "found no Claude Code",
/// so only the home lookup is asserted.
#[test]
fn install_without_home_env_falls_back_to_os_home() {
    let o = Command::new(env!("CARGO_BIN_EXE_fael"))
        .args(["install", "--dry-run"])
        .env_remove("HOME")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!err.to_lowercase().contains("home"), "{err}");
}
