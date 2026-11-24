use crate::completer::{Completer, CompletionKind, longest_common_prefix};
use crate::config::{Config, Rgb, Theme};
use crate::history::{HistoryStore, fuzzy_score};
use crate::shell::clamp_ghost;
use crate::shell::parser::{LogicalOp, parse_command_line};
use crate::terminal::grid::{ColorSpec, FLAG_BOLD, FLAG_ITALIC, TerminalGrid};
use crate::usage::{TimeRange, format_cost, format_tokens, parse_iso8601, scan_usage_from_home};
use std::collections::HashMap;
use std::path::Path;

#[test]
fn test_parser_pipes_redirects_and_chains() {
    let aliases = HashMap::new();
    let parsed = parse_command_line(
        "echo 'hello world' | grep hello > out.txt && ls -la || echo failed",
        0,
        &aliases,
    )
    .expect("should parse");
    assert_eq!(parsed.items.len(), 3);
    assert_eq!(parsed.items[0].0, LogicalOp::Always);
    assert_eq!(parsed.items[0].1.commands.len(), 2);
    assert_eq!(parsed.items[0].1.commands[0].args, vec!["echo", "hello world"]);
    assert_eq!(parsed.items[0].1.commands[1].args, vec!["grep", "hello"]);
    assert_eq!(
        parsed.items[0].1.commands[1].stdout_file.as_deref(),
        Some("out.txt")
    );
    assert_eq!(parsed.items[1].0, LogicalOp::And);
    assert_eq!(parsed.items[2].0, LogicalOp::Or);
}

#[test]
fn test_parser_env_and_status_expansion() {
    let aliases = HashMap::new();
    unsafe {
        std::env::set_var("ITSADRI_TEST_VAR", "rust_rules");
    }
    let parsed = parse_command_line("echo \"$ITSADRI_TEST_VAR\" $? '$ITSADRI_TEST_VAR'", 42, &aliases)
        .expect("should parse");
    let args = &parsed.items[0].1.commands[0].args;
    assert_eq!(args, &vec!["echo", "rust_rules", "42", "$ITSADRI_TEST_VAR"]);
}

#[test]
fn test_fuzzy_matcher_ranks_substring_above_scattered() {
    let (s1, idx1) = fuzzy_score("cargo build --release", "build").expect("match");
    let (s2, _) = fuzzy_score("git branch -u origin/old", "build").expect("match");
    assert!(s1 > s2);
    assert_eq!(idx1, vec![6, 7, 8, 9, 10]);
}

#[test]
fn test_history_prefix_autosuggestion() {
    let tmp = std::env::temp_dir().join("itsadri_test_history.tsv");
    let _ = std::fs::remove_file(&tmp);
    let mut store = HistoryStore::with_path(tmp.clone());
    store.add("git status --short");
    store.add("git switch main");
    assert_eq!(store.suggest_prefix("git s"), Some("git switch main"));
    assert_eq!(store.suggest_prefix("git st"), Some("git status --short"));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_completer_builtins_and_lcp() {
    let mut completer = Completer::new();
    let aliases = HashMap::new();
    let (start, items) = completer.complete("hist", 4, Path::new("."), &aliases);
    assert_eq!(start, 0);
    assert!(items.iter().any(|i| i.label == "history" && i.kind == CompletionKind::Builtin));

    let (_, items2) = completer.complete("ex", 2, Path::new("."), &aliases);
    let lcp = longest_common_prefix(&items2);
    assert!(lcp.starts_with("ex"));
}

#[test]
fn test_terminal_grid_vte_sgr_and_osc777() {
    let mut grid = TerminalGrid::new(40, 10, 100);
    let mut parser = vte::Parser::new();

    // Styled text + OSC 7 CWD + OSC 777 git + OSC 777 completion popup
    let seq = b"\x1b[1;3;38;2;255;100;50mHi\x1b[0m\x1b]7;file://localhost/tmp\x07\x1b]777;git;main\x07\x1b]777;comp_open;1;DIR:src/|FILE:Cargo.toml\x07";
    parser.advance(&mut grid, seq);

    assert_eq!(grid.cells[0][0].ch, 'H');
    assert_eq!(grid.cells[0][1].ch, 'i');
    assert_eq!(grid.cells[0][0].flags & (FLAG_BOLD | FLAG_ITALIC), FLAG_BOLD | FLAG_ITALIC);
    assert_eq!(grid.cells[0][0].fg, ColorSpec::TrueColor(Rgb::new(255, 100, 50)));
    assert_eq!(grid.cwd, "/tmp");
    assert_eq!(grid.git_branch, "main");

    let popup = grid.completion_popup.as_ref().expect("popup should be open");
    assert_eq!(popup.selected, 1);
    assert_eq!(popup.items.len(), 2);
    assert_eq!(popup.items[0].kind, CompletionKind::Dir);
    assert_eq!(popup.items[0].label, "src/");
    assert_eq!(popup.items[1].kind, CompletionKind::File);
    assert_eq!(popup.items[1].label, "Cargo.toml");
}

#[test]
fn test_system_monitor_samples_cpu_ram_disks_and_processes() {
    let mut mon = crate::monitor::SystemMonitor::new();
    let snap = mon.sample(crate::monitor::ProcessSort::Memory);
    assert!(snap.memory.total_bytes > 0);
    assert!(snap.total_processes > 0);
    assert!(!snap.processes.is_empty());
    assert_eq!(crate::monitor::format_bytes(1024 * 1024 * 1024), "1.0 GiB");
}

#[test]
fn test_themes_and_default_config() {
    let cfg = Config::default();
    assert!(cfg.copy_on_select);
    let t = Theme::by_name(&cfg.theme);
    assert_eq!(t.name, "catppuccin-mocha");
    assert_eq!(Theme::next_name(t.name), "tokyo-night");
    let black = Theme::by_name("pure-black");
    assert_eq!(black.name, "pure-black");
    assert_eq!(black.bg, Rgb::new(0, 0, 0));
}


#[test]
fn test_usage_scanner_detects_all_four_agents() {
    let tmp = std::env::temp_dir().join("itsadri_test_usage_home");
    let _ = std::fs::remove_dir_all(&tmp);

    // 1. Claude Code log (with duplicate streaming message.id to test deduplication)
    let claude_dir = tmp.join(".claude/projects/demo");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("sess1.jsonl"),
        concat!(
            "{\"timestamp\":\"2026-11-23T19:00:00Z\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":200,\"cache_read_input_tokens\":500}}}\n",
            "{\"timestamp\":\"2026-11-23T19:00:01Z\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":450,\"cache_read_input_tokens\":500}}}\n"
        ),
    )
    .unwrap();

    // 2. Codex log
    let codex_dir = tmp.join(".codex/sessions/2026/11");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
        codex_dir.join("rollout-1.jsonl"),
        concat!(
            "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-11-23T19:10:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":2000,\"cached_input_tokens\":500,\"output_tokens\":300,\"reasoning_output_tokens\":100}}}}\n"
        ),
    )
    .unwrap();

    // 3. Gemini CLI session log
    let gemini_dir = tmp.join(".gemini/tmp/proj/chats");
    std::fs::create_dir_all(&gemini_dir).unwrap();
    std::fs::write(
        gemini_dir.join("session-001.json"),
        r#"{"sessionId":"s1","lastUpdated":"2026-11-23T19:20:00Z","messages":[{"type":"gemini","model":"gemini-2.5-pro","tokens":{"input":800,"output":250,"cached":100,"thoughts":50}}]}"#,
    )
    .unwrap();

    // 3b. Antigravity CLI transcript.jsonl (top-level input_tokens/cache_read_tokens/output_tokens)
    let agy_dir = tmp.join(".gemini/antigravity-cli/brain/conv-1/.system_generated/logs");
    std::fs::create_dir_all(&agy_dir).unwrap();
    std::fs::write(
        agy_dir.join("transcript.jsonl"),
        concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"created_at\":\"2026-11-23T20:12:20Z\",\"content\":\"hi\"}\n",
            "{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-11-23T20:12:21Z\",\"model\":\"gemini-4-neon-medium\",\"input_tokens\":3000,\"cache_read_tokens\":1000,\"output_tokens\":200}\n"
        ),
    )
    .unwrap();

    // 4. OpenCode message JSON
    let opencode_dir = tmp.join(".local/share/opencode/storage/message/sess_oc");
    std::fs::create_dir_all(&opencode_dir).unwrap();
    std::fs::write(
        opencode_dir.join("m1.json"),
        r#"{"role":"assistant","modelID":"deepseek-chat","sessionID":"sess_oc","tokens":{"input":600,"output":150,"reasoning":0,"cache":{"read":50,"write":0}},"cost":0.002,"time":{"completed":1795461600000}}"#,
    )
    .unwrap();

    let report = scan_usage_from_home(&tmp);
    let snap = report.snapshot(TimeRange::All);

    assert_eq!(report.files_scanned, 5);
    assert_eq!(snap.models.len(), 5);
    assert_eq!(snap.total.requests, 5);
    assert_eq!(snap.total.sessions, 5);

    // Claude deduplicated: 1000 + 450 + 500 = 1950 (not doubled)
    let claude = snap.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(claude.detected);
    assert_eq!(claude.stats.total_tokens(), 1950);
    assert_eq!(claude.stats.requests, 1);

    // Codex: (2000 - 500) in + 500 cached + 300 out + 100 reasoning = 2400
    let codex = snap.agents.iter().find(|a| a.id == "codex").unwrap();
    assert!(codex.detected);
    assert_eq!(codex.stats.total_tokens(), 2400);

    // Gemini (1200) + Antigravity (3000 + 1000 + 200 = 4200) = 5400
    let gemini = snap.agents.iter().find(|a| a.id == "gemini").unwrap();
    assert!(gemini.detected);
    assert_eq!(gemini.stats.total_tokens(), 5400);
    assert_eq!(gemini.stats.requests, 2);

    // OpenCode: 600 + 150 + 50 = 800
    let opencode = snap.agents.iter().find(|a| a.id == "opencode").unwrap();
    assert!(opencode.detected);
    assert_eq!(opencode.stats.total_tokens(), 800);

    // Formatting & ISO-8601 helpers
    assert_eq!(format_tokens(1_500_000), "1.50M");
    assert_eq!(format_tokens(12_340), "12.3K");
    assert_eq!(format_cost(1.25), "$1.25");
    assert!(parse_iso8601("2026-11-23T19:00:00Z").is_some());

    // Live OAuth cache test (Quickshell-compatible ~/.cache/claude-usage.json)
    // and Antigravity `agy -p "/usage"` cache test (~/.cache/agy-usage.txt)
    let cache_dir = tmp.join(".cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(
        cache_dir.join("claude-usage.json"),
        r#"{"five_hour":{"utilization":28.0,"resets_at":"2026-11-23T23:00:00Z"},"seven_day":{"utilization":14.0,"resets_at":"2026-11-28T00:00:00Z"}}"#,
    )
    .unwrap();
    std::fs::write(
        cache_dir.join("agy-usage.txt"),
        concat!(
            "Quota:\n",
            "Gemini Models          Weekly Limit Remaining     93%   2026-11-30 20:03 UTC\n",
            "Gemini Models          Five Hour Limit Remaining  73%   2026-11-24 01:03 UTC\n",
            "Claude and GPT models  Weekly Limit Remaining     100%  2026-11-30 21:36 UTC\n",
        ),
    )
    .unwrap();
    let report_live = scan_usage_from_home(&tmp);
    let snap_live = report_live.snapshot(TimeRange::All);
    let claude_live = snap_live.agents.iter().find(|a| a.id == "claude").unwrap();
    assert_eq!(claude_live.quota_5h.live_used_pct, Some(28.0));
    assert_eq!(claude_live.quota_5h.live_remaining_pct(), Some(72.0));
    assert_eq!(claude_live.quota_weekly.live_used_pct, Some(14.0));
    assert_eq!(claude_live.quota_weekly.live_remaining_pct(), Some(86.0));

    let gemini_live = snap_live.agents.iter().find(|a| a.id == "gemini").unwrap();
    assert_eq!(gemini_live.quota_5h.live_used_pct, Some(27.0));
    assert_eq!(gemini_live.quota_5h.live_remaining_pct(), Some(73.0));
    assert_eq!(gemini_live.quota_weekly.live_used_pct, Some(7.0));
    assert_eq!(gemini_live.quota_weekly.live_remaining_pct(), Some(93.0));
    assert!(gemini_live.quota_5h.reset_in_secs.is_some());
    assert!(gemini_live.quota_weekly.reset_in_secs.is_some());
    assert_eq!(crate::usage::format_duration_short(3660), "1h 1m");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_build_enriched_path_adds_user_bin_dirs_without_duplicates() {
    let tmp = std::env::temp_dir().join("itsadri_test_enriched_path");
    let _ = std::fs::remove_dir_all(&tmp);
    let local_bin = tmp.join(".local/bin");
    let agy_bin = tmp.join(".gemini/antigravity-cli/bin");
    std::fs::create_dir_all(&local_bin).unwrap();
    std::fs::create_dir_all(&agy_bin).unwrap();

    let enriched = crate::shell::executor::build_enriched_path("/usr/bin", Some(&tmp));
    let parts: Vec<std::path::PathBuf> = std::env::split_paths(&enriched).collect();
    assert_eq!(parts[0], local_bin);
    assert_eq!(parts[1], agy_bin);
    assert!(parts.contains(&std::path::PathBuf::from("/usr/bin")));
    // Non-existent dir should not be added
    assert!(!parts.contains(&tmp.join(".cargo/bin")));

    // Calling again should not duplicate entries
    let enriched2 = crate::shell::executor::build_enriched_path(&enriched, Some(&tmp));
    assert_eq!(enriched, enriched2);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_commands_manager_persist_edit_run_stop_and_compose_detection() {
    use crate::commands::{
        CommandEvent, CommandManager, CommandStatus, derive_stop_command,
        is_detached_compose_command,
    };
    use std::sync::{Arc, Mutex};

    // Compose detached detection & derived stop command
    assert!(is_detached_compose_command("docker-compose up -d"));
    assert!(is_detached_compose_command("docker compose up --detach --build"));
    assert!(is_detached_compose_command(
        "docker-compose -f docker-compose.dev.yml up -d"
    ));
    assert!(!is_detached_compose_command("docker-compose up"));
    assert!(!is_detached_compose_command("npm run dev"));

    assert_eq!(
        derive_stop_command("docker-compose up -d").as_deref(),
        Some("docker-compose down")
    );
    assert_eq!(
        derive_stop_command("docker compose -f compose.prod.yml -p app up -d").as_deref(),
        Some("docker compose -f compose.prod.yml -p app down")
    );
    assert_eq!(
        derive_stop_command("podman-compose up -d").as_deref(),
        Some("podman-compose down")
    );
    assert_eq!(derive_stop_command("npm run dev"), None);

    // Persistence + Add + Edit
    let tmp_file = std::env::temp_dir().join("itsadri_test_commands.toml");
    let _ = std::fs::remove_file(&tmp_file);

    let mut mgr = CommandManager::with_path(tmp_file.clone());
    let id = mgr.add("echo hello_runner".to_string(), "/tmp".to_string());
    assert_eq!(mgr.entries.len(), 1);
    assert!(mgr.update(id, "echo updated_runner".to_string(), "/tmp".to_string()));

    // Reload from disk
    let mut mgr2 = CommandManager::with_path(tmp_file.clone());
    assert_eq!(mgr2.entries.len(), 1);
    assert_eq!(mgr2.entries[0].command, "echo updated_runner");
    assert_eq!(mgr2.entries[0].directory, "/tmp");

    // Start command and collect events
    let events: Arc<Mutex<Vec<CommandEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let ev_sink = events.clone();
    let cb = Arc::new(move |ev: CommandEvent| {
        ev_sink.lock().unwrap().push(ev);
    });
    mgr2.start(id, cb).expect("should start echo");

    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(25));
        let drained: Vec<CommandEvent> = events.lock().unwrap().drain(..).collect();
        for ev in drained {
            mgr2.apply_event(ev);
        }
        if mgr2.entries[0].status == CommandStatus::Exited(0) {
            break;
        }
    }
    assert_eq!(mgr2.entries[0].status, CommandStatus::Exited(0));
    assert!(
        mgr2.entries[0]
            .logs
            .iter()
            .any(|l| l.contains("updated_runner"))
    );

    // Detached compose exit 0 transitions to Detached status
    let compose_id = mgr2.add("docker-compose up -d".to_string(), "/tmp".to_string());
    mgr2.apply_event(CommandEvent::Exited {
        id: compose_id,
        code: Some(0),
        was_stop_action: false,
    });
    assert_eq!(
        mgr2.find_by_id(compose_id).unwrap().status,
        CommandStatus::Detached
    );

    // Simulated stop action completing transitions Detached -> Stopped
    mgr2.apply_event(CommandEvent::Exited {
        id: compose_id,
        code: Some(0),
        was_stop_action: true,
    });
    assert_eq!(
        mgr2.find_by_id(compose_id).unwrap().status,
        CommandStatus::Stopped
    );

    let _ = std::fs::remove_file(&tmp_file);
}

#[test]
fn test_ctrl_shift_copy_paste_shortcut_matching_and_modifier_keys() {
    use crate::ui::{encode_key, matches_shortcut_key};
    use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

    // Uppercase, lowercase, and ASCII control-char representations of C and V
    assert!(matches_shortcut_key(
        &Key::Character("C".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        'c',
        KeyCode::KeyC
    ));
    assert!(matches_shortcut_key(
        &Key::Character("c".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        'c',
        KeyCode::KeyC
    ));
    assert!(matches_shortcut_key(
        &Key::Character("\u{3}".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        'c',
        KeyCode::KeyC
    ));
    assert!(matches_shortcut_key(
        &Key::Character("V".into()),
        PhysicalKey::Code(KeyCode::KeyV),
        'v',
        KeyCode::KeyV
    ));
    assert!(matches_shortcut_key(
        &Key::Character("\u{16}".into()),
        PhysicalKey::Code(KeyCode::KeyV),
        'v',
        KeyCode::KeyV
    ));
    // Non-Latin layout falls back to physical key
    assert!(matches_shortcut_key(
        &Key::Character("С".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        'c',
        KeyCode::KeyC
    ));
    // Different ASCII letter on Dvorak/Colemak does NOT falsely match physical KeyC
    assert!(!matches_shortcut_key(
        &Key::Character("J".into()),
        PhysicalKey::Code(KeyCode::KeyC),
        'c',
        KeyCode::KeyC
    ));

    // Modifier keys must encode to empty bytes so they never clear active text selection
    assert!(encode_key(&Key::Named(NamedKey::Control), None, true, false, false, false).is_empty());
    assert!(encode_key(&Key::Named(NamedKey::Shift), None, true, true, false, false).is_empty());
}

#[test]
fn test_settings_config_font_sizes_and_custom_shortcuts() {
    use crate::config::{Shortcut, ShortcutKey};
    use crate::ui::{matches_configured_shortcut, shortcut_from_winit};
    use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

    // Default config has individual font sizes and default shortcuts
    let def = Config::default();
    assert_eq!(def.font_size, 19.0);
    assert_eq!(def.commands_font_size, 18.0);
    assert_eq!(def.monitor_font_size, 18.0);
    assert_eq!(def.usage_font_size, 18.0);
    assert_eq!(def.commands_shortcut().format(), "Ctrl+P");
    assert_eq!(def.monitor_shortcut().format(), "Ctrl+M");
    assert_eq!(def.usage_shortcut().format(), "Ctrl+U");

    // Backward-compatible TOML deserialization (older config missing new keys)
    let legacy: Config = toml::from_str("theme = \"tokyo-night\"\nfont_size = 21.0\n").unwrap();
    assert_eq!(legacy.theme, "tokyo-night");
    assert_eq!(legacy.font_size, 21.0);
    assert_eq!(legacy.commands_font_size, 18.0);
    assert_eq!(legacy.monitor_font_size, 18.0);
    assert_eq!(legacy.usage_font_size, 18.0);
    assert_eq!(legacy.shortcut_commands, "Ctrl+P");

    // Round-trip TOML with customized per-tab font sizes & shortcuts
    let mut custom = Config::default();
    custom.font_size = 16.0;
    custom.commands_font_size = 22.0;
    custom.monitor_font_size = 14.0;
    custom.usage_font_size = 20.0;
    custom.shortcut_commands = "Ctrl+Shift+C".to_string();
    custom.shortcut_monitor = "Alt+M".to_string();
    custom.shortcut_usage = "F8".to_string();
    let encoded = toml::to_string_pretty(&custom).unwrap();
    let decoded: Config = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded.font_size, 16.0);
    assert_eq!(decoded.commands_font_size, 22.0);
    assert_eq!(decoded.monitor_font_size, 14.0);
    assert_eq!(decoded.usage_font_size, 20.0);
    assert_eq!(decoded.commands_shortcut().format(), "Ctrl+Shift+C");
    assert_eq!(decoded.monitor_shortcut().format(), "Alt+M");
    assert_eq!(decoded.usage_shortcut().format(), "F8");

    // Shortcut parsing & validation
    assert_eq!(
        Shortcut::parse("ctrl+shift+p"),
        Some(Shortcut {
            ctrl: true,
            shift: true,
            alt: false,
            key: ShortcutKey::Char('p'),
        })
    );
    assert_eq!(
        Shortcut::parse("F9"),
        Some(Shortcut {
            ctrl: false,
            shift: false,
            alt: false,
            key: ShortcutKey::Function(9),
        })
    );
    // Plain unmodified letters are rejected so normal typing isn't hijacked
    assert_eq!(Shortcut::parse("p"), None);
    assert_eq!(Shortcut::parse("Shift+P"), None);
    assert_eq!(Shortcut::parse(""), None);

    // Winit shortcut capture & matching
    let captured = shortcut_from_winit(
        &Key::Character("K".into()),
        PhysicalKey::Code(KeyCode::KeyK),
        true,
        true,
        false,
    )
    .expect("should capture Ctrl+Shift+K");
    assert_eq!(captured.format(), "Ctrl+Shift+K");
    assert!(matches_configured_shortcut(
        captured,
        &Key::Character("k".into()),
        PhysicalKey::Code(KeyCode::KeyK),
        true,
        true,
        false,
    ));
    assert!(!matches_configured_shortcut(
        captured,
        &Key::Character("k".into()),
        PhysicalKey::Code(KeyCode::KeyK),
        true,
        false,
        false,
    ));

    let f8 = shortcut_from_winit(
        &Key::Named(NamedKey::F8),
        PhysicalKey::Code(KeyCode::F8),
        false,
        false,
        false,
    )
    .expect("should capture F8");
    assert_eq!(f8.format(), "F8");
    assert!(matches_configured_shortcut(
        f8,
        &Key::Named(NamedKey::F8),
        PhysicalKey::Code(KeyCode::F8),
        false,
        false,
        false,
    ));
    // Unmodified letter cannot be captured
    assert_eq!(
        shortcut_from_winit(
            &Key::Character("a".into()),
            PhysicalKey::Code(KeyCode::KeyA),
            false,
            false,
            false,
        ),
        None
    );

    // Theme prev/next cycle
    assert_eq!(Theme::prev_name("catppuccin-mocha"), "pure-black");
    assert_eq!(Theme::prev_name("tokyo-night"), "catppuccin-mocha");
}

#[test]
fn test_clamp_ghost_ascii_truncates() {
    assert_eq!(clamp_ghost("abcdef", 3), "abc");
}

#[test]
fn test_clamp_ghost_zero_is_empty() {
    assert_eq!(clamp_ghost("abcdef", 0), "");
}

#[test]
fn test_clamp_ghost_enough_room_unchanged() {
    assert_eq!(clamp_ghost("abc", 3), "abc");
    assert_eq!(clamp_ghost("abc", 10), "abc");
}

#[test]
fn test_clamp_ghost_multibyte_char_boundary() {
    assert_eq!(clamp_ghost("ñé日本", 1), "ñ");
    assert_eq!(clamp_ghost("ñé日本", 3), "ñé日");
    assert_eq!(clamp_ghost("ñé日本", 4), "ñé日本");
}
