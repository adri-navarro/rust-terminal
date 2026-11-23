use crate::completer::{Completer, CompletionKind, longest_common_prefix};
use crate::config::{Config, Rgb, Theme};
use crate::history::{HistoryStore, fuzzy_score};
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
