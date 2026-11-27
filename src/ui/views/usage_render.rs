//! AI Agent & Model Usage tab renderer.

use crate::config::{Config, Rgb, Theme};
use crate::history::format_relative_time;
use crate::ui::canvas::{Canvas, Rect};
use crate::ui::chrome::tab_bar::{STATUS_BAR_H, TabBarHitZones, top_reserved};
use crate::ui::font::{FontEngine, FontStyle};
use crate::ui::renderer::UsageViewState;
use crate::usage::{TimeRange, WindowQuota, format_cost, format_duration_short, format_tokens};

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_usage_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    uv: &UsageViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.usage_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 22i32;
    let x0 = pad;
    let y0 = top + 16;
    let content_w = (width as i32 - pad * 2).max(240);
    let max_y = (height as i32) - bottom - 14;

    // Right-aligned controls: [5h] [24h] [7d] [30d] [All]  [Refresh]
    let btn_y = y0 + 4;
    let btn_h = sc(36).max(24);
    let btn_base = btn_y + sc(24).max(16);
    let mut rx = x0 + content_w;

    let refresh_label = if uv.scanning { "Scanning..." } else { "Refresh" };
    let ref_w = font.measure_text(refresh_label, fs(17.5)) + 22;
    rx -= ref_w;
    let ref_rect = Rect {
        x: rx,
        y: btn_y,
        w: ref_w,
        h: btn_h,
    };
    canvas.fill_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        refresh_label,
        FontStyle::Bold,
        fs(17.5),
        ref_rect.x + 11,
        btn_base,
        ref_rect.x + ref_rect.w - 4,
        if uv.scanning { theme.fg_muted } else { theme.accent },
    );
    hit_zones.usage_refresh_btn = Some(ref_rect);

    rx -= 10;
    for &range in TimeRange::ALL.iter().rev() {
        let lbl = range.label();
        let rw = font.measure_text(lbl, fs(17.5)) + 22;
        rx -= rw;
        let rect = Rect {
            x: rx,
            y: btn_y,
            w: rw,
            h: btn_h,
        };
        let active = range == uv.range;
        if active {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.accent_soft, 255);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.accent, 210);
        } else {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.surface, 255);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.border, 140);
        }
        canvas.draw_text_clipped(
            font,
            lbl,
            if active { FontStyle::Bold } else { FontStyle::Regular },
            fs(17.5),
            rect.x + 11,
            btn_base,
            rect.x + rect.w - 4,
            if active { theme.accent } else { theme.fg_muted },
        );
        hit_zones.usage_range_btns.push((range, rect));
        rx -= 6;
    }

    // --- Header Row ---
    let header_max_x = (rx - 12).max(x0 + 180);
    canvas.draw_text_clipped(
        font,
        "Agent & Model Usage",
        FontStyle::Bold,
        fs(25.0),
        x0,
        y0 + sc(24),
        header_max_x,
        theme.fg,
    );

    let subtitle = if uv.scanning && uv.report.is_none() {
        "Scanning local AI CLI logs (~/.claude, ~/.codex, ~/.gemini, opencode)...".to_string()
    } else if let Some(rep) = uv.report {
        format!(
            "Live · 5s auto-refresh  ·  {} files ({}ms)  ·  updated {}",
            rep.files_scanned,
            rep.scan_duration_ms,
            format_relative_time(rep.generated_at),
        )
    } else {
        "Auto-detecting local AI CLI logs...".to_string()
    };
    canvas.draw_text_clipped(
        font,
        &subtitle,
        FontStyle::Regular,
        fs(17.5),
        x0,
        y0 + sc(47),
        header_max_x,
        theme.fg_muted,
    );

    let mut cur_y = y0 + sc(62).max(btn_h + 14);
    canvas.fill_rect(x0, cur_y, content_w, 1, theme.border);
    cur_y += 14;

    let Some(report) = uv.report else {
        // Initial scanning state
        let card_h = sc(116).max(80);
        canvas.fill_rounded_rect(x0, cur_y, content_w, card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(x0, cur_y, content_w, card_h, 10, theme.border, 200);
        canvas.draw_text_clipped(
            font,
            "Scanning local AI CLI logs...",
            FontStyle::Bold,
            fs(20.5),
            x0 + 20,
            cur_y + sc(46),
            x0 + content_w - 20,
            theme.accent,
        );
        canvas.draw_text_clipped(
            font,
            "Checking ~/.claude/projects, ~/.codex/sessions, ~/.gemini/tmp, ~/.gemini/antigravity-cli, and ~/.local/share/opencode",
            FontStyle::Regular,
            fs(18.0),
            x0 + 20,
            cur_y + sc(78),
            x0 + content_w - 20,
            theme.fg_muted,
        );
        return;
    };

    let snap = report.snapshot(uv.range);

    // --- KPI Summary Cards (4 columns) ---
    let kpi_gap = 12i32;
    let kpi_h = sc(106).max(74);
    let kpi_w = (content_w - kpi_gap * 3) / 4;
    let tot = &snap.total;
    let kpis = [
        (
            "TOTAL TOKENS",
            format_tokens(tot.total_tokens()),
            format!("{} in · {} out", format_tokens(tot.input_tokens), format_tokens(tot.output_tokens)),
            theme.accent,
        ),
        (
            "REQUESTS",
            format_tokens(tot.requests),
            format!("{} sessions · {} models", tot.sessions, snap.models.len()),
            theme.info,
        ),
        (
            "CACHE READ",
            format_tokens(tot.cache_read_tokens),
            format!("{:.0}% cache hit rate", tot.cache_hit_pct()),
            theme.success,
        ),
        (
            "EST. COST",
            format_cost(tot.estimated_cost_usd),
            format!("range: {}", uv.range.label()),
            theme.warning,
        ),
    ];

    for (i, (label, value, sub, color)) in kpis.iter().enumerate() {
        let kx = x0 + (i as i32) * (kpi_w + kpi_gap);
        canvas.fill_rounded_rect(kx, cur_y, kpi_w, kpi_h, 9, theme.surface, 255);
        canvas.stroke_rounded_rect(kx, cur_y, kpi_w, kpi_h, 9, theme.border, 190);
        // Subtle left accent stripe
        canvas.fill_rounded_rect(kx + 1, cur_y + 12, 3, (kpi_h - 24).max(8), 1, *color, 255);

        canvas.draw_text_clipped(
            font,
            label,
            FontStyle::Bold,
            fs(16.5),
            kx + 14,
            cur_y + sc(26),
            kx + kpi_w - 10,
            theme.fg_muted,
        );
        canvas.draw_text_clipped(
            font,
            value,
            FontStyle::Bold,
            fs(28.0),
            kx + 14,
            cur_y + sc(62),
            kx + kpi_w - 10,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            sub,
            FontStyle::Regular,
            fs(17.0),
            kx + 14,
            cur_y + sc(90),
            kx + kpi_w - 10,
            theme.fg_muted,
        );
    }

    cur_y += kpi_h + 16;

    // --- Agents Section (Per-Agent 5h & Weekly Quotas) ---
    canvas.draw_text_clipped(
        font,
        "AGENTS  (5H & WEEKLY USAGE PER AGENT)",
        FontStyle::Bold,
        fs(16.5),
        x0,
        cur_y + sc(16),
        x0 + content_w,
        theme.fg_muted,
    );
    cur_y += sc(26).max(18);

    let agent_gap = 12i32;
    let cols = if content_w >= 860 {
        snap.agents.len().clamp(1, 4) as i32
    } else {
        2
    };
    let agent_w = (content_w - agent_gap * (cols - 1)) / cols;
    let agent_h = sc(176).max(120);
    let rows_count = ((snap.agents.len() as i32) + cols - 1) / cols;

    for (i, agent) in snap.agents.iter().enumerate() {
        let col_i = (i as i32) % cols;
        let row_i = (i as i32) / cols;
        let ax = x0 + col_i * (agent_w + agent_gap);
        let ay = cur_y + row_i * (agent_h + agent_gap);
        canvas.fill_rounded_rect(ax, ay, agent_w, agent_h, 9, theme.surface, 255);
        canvas.stroke_rounded_rect(ax, ay, agent_w, agent_h, 9, theme.border, 185);

        let accent = agent_color(agent.id, theme);
        let active_in_range = agent.stats.total_tokens() > 0 || agent.stats.requests > 0;
        let dot = if active_in_range {
            accent
        } else if agent.detected {
            theme.warning
        } else {
            theme.fg_muted
        };
        canvas.fill_rounded_rect(ax + 12, ay + sc(16), 7, 7, 3, dot, 255);
        let active_ago = agent.last_active.map(format_relative_time);
        let name_max_x = if let Some(ref ago) = active_ago {
            let aw = font.measure_text(ago, fs(16.0));
            let ago_x = ax + agent_w - aw - 10;
            if ago_x > ax + 110 {
                canvas.draw_text_clipped(
                    font,
                    ago,
                    FontStyle::Regular,
                    fs(16.0),
                    ago_x,
                    ay + sc(26),
                    ax + agent_w - 8,
                    theme.fg_muted,
                );
                ago_x - 6
            } else {
                ax + agent_w - 10
            }
        } else {
            ax + agent_w - 10
        };
        canvas.draw_text_clipped(
            font,
            agent.name,
            FontStyle::Bold,
            fs(18.5),
            ax + 24,
            ay + sc(27),
            name_max_x,
            if active_in_range || agent.detected { theme.fg } else { theme.fg_muted },
        );

        let stat_line = if active_in_range {
            format!(
                "{} tok · {} req",
                format_tokens(agent.stats.total_tokens()),
                format_tokens(agent.stats.requests)
            )
        } else if agent.detected {
            "No activity in range".to_string()
        } else {
            "Not detected".to_string()
        };
        canvas.draw_text_clipped(
            font,
            &stat_line,
            FontStyle::Regular,
            fs(17.0),
            ax + 12,
            ay + sc(51),
            ax + agent_w - 10,
            if active_in_range { theme.fg } else { theme.fg_muted },
        );

        let model_sub = agent.top_model.as_deref().unwrap_or("—");
        canvas.draw_text_clipped(
            font,
            model_sub,
            FontStyle::Italic,
            fs(16.5),
            ax + 12,
            ay + sc(73),
            ax + agent_w - 10,
            theme.fg_muted,
        );

        // 5-hour quota row inside this agent card
        render_agent_quota_row(
            canvas,
            ax + 12,
            ay + sc(88),
            agent_w - 24,
            "5h",
            &agent.quota_5h,
            accent,
            theme,
            font,
            s,
        );

        // Weekly (7d) quota row inside this agent card
        render_agent_quota_row(
            canvas,
            ax + 12,
            ay + sc(128),
            agent_w - 24,
            "7d",
            &agent.quota_weekly,
            accent,
            theme,
            font,
            s,
        );
    }

    cur_y += rows_count * agent_h + (rows_count - 1).max(0) * agent_gap + 16;

    // --- Models Usage Table ---
    let models_hdr = format!("MODELS ({})", snap.models.len());
    canvas.draw_text_clipped(
        font,
        &models_hdr,
        FontStyle::Bold,
        fs(17.0),
        x0,
        cur_y + sc(16),
        x0 + 280,
        theme.fg_muted,
    );
    let usage_hint = "1-5 range  ·  R refresh  ·  ↑↓ scroll  ·  Esc close";
    let uhw = font.measure_text(usage_hint, fs(16.5));
    if content_w > uhw + 280 {
        canvas.draw_text_clipped(
            font,
            usage_hint,
            FontStyle::Regular,
            fs(16.5),
            x0 + content_w - uhw,
            cur_y + sc(16),
            x0 + content_w,
            theme.fg_muted,
        );
    }
    cur_y += sc(26).max(18);

    let table_h = (max_y - cur_y).max(80);
    canvas.fill_rounded_rect(x0, cur_y, content_w, table_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(x0, cur_y, content_w, table_h, 10, theme.border, 190);

    if snap.models.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No agent or model usage found for this time range.",
            FontStyle::Bold,
            fs(19.5),
            x0 + 20,
            cur_y + sc(42),
            x0 + content_w - 20,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            "Auto-scanned: ~/.claude/projects  ·  ~/.codex/sessions  ·  ~/.gemini/tmp  ·  ~/.gemini/antigravity-cli  ·  ~/.local/share/opencode",
            FontStyle::Regular,
            fs(17.5),
            x0 + 20,
            cur_y + sc(74),
            x0 + content_w - 20,
            theme.fg_muted,
        );
        return;
    }

    // Column layout
    let col_model = x0 + 16;
    let col_agent = x0 + (content_w * 30 / 100).max(170);
    let col_req = x0 + (content_w * 46 / 100);
    let col_in = x0 + (content_w * 55 / 100);
    let col_out = x0 + (content_w * 65 / 100);
    let col_cache = x0 + (content_w * 74 / 100);
    let col_total = x0 + (content_w * 84 / 100);
    let col_cost = x0 + (content_w * 92 / 100);

    let hdr_h = sc(40).max(26);
    let hdr_base = cur_y + sc(27).max(18);
    for (cx, title) in [
        (col_model, "MODEL"),
        (col_agent, "AGENT"),
        (col_req, "REQ"),
        (col_in, "INPUT"),
        (col_out, "OUTPUT"),
        (col_cache, "CACHE"),
        (col_total, "TOTAL"),
        (col_cost, "COST"),
    ] {
        canvas.draw_text_clipped(
            font,
            title,
            FontStyle::Bold,
            fs(16.5),
            cx,
            hdr_base,
            x0 + content_w - 10,
            theme.fg_muted,
        );
    }
    canvas.fill_rect(x0 + 10, cur_y + hdr_h, content_w - 20, 1, theme.border);

    let row_h = sc(48).max(30);
    let list_y = cur_y + hdr_h + 6;
    let visible_rows = ((table_h - hdr_h - 12) / row_h).max(1) as usize;
    let max_scroll = snap.models.len().saturating_sub(visible_rows);
    let start = uv.scroll.min(max_scroll);
    let total_all_tokens = snap.total.total_tokens().max(1);

    for (vi, m) in snap.models.iter().skip(start).take(visible_rows).enumerate() {
        let ry = list_y + (vi as i32) * row_h;
        if vi % 2 == 1 {
            canvas.fill_rounded_rect(x0 + 8, ry, content_w - 16, row_h - 2, 6, theme.surface_alt, 110);
        }
        let base = ry + sc(28).max(18);
        let accent = agent_color(m.agent_id, theme);

        // Model name
        canvas.draw_text_clipped(
            font,
            &m.model,
            FontStyle::Bold,
            fs(18.5),
            col_model,
            base,
            col_agent - 12,
            theme.fg,
        );

        // Proportional share underline under model name
        let share = (m.stats.total_tokens() as f64 / total_all_tokens as f64).clamp(0.0, 1.0);
        let bar_max = (col_agent - col_model - 20).max(40);
        let bar_w = ((bar_max as f64) * share).round() as i32;
        if bar_w > 0 {
            canvas.fill_rounded_rect(col_model, ry + row_h - 9, bar_w.max(3), 3, 1, accent, 200);
        }

        // Agent badge pill
        let badge_w = (font.measure_text(m.agent_name, fs(16.5)) + 14).min(col_req - col_agent - 10);
        if badge_w > 24 {
            let bh = sc(28).max(18);
            let by = ry + (row_h - bh) / 2;
            canvas.fill_rounded_rect(col_agent, by, badge_w, bh, 4, accent, 38);
            canvas.draw_text_clipped(
                font,
                m.agent_name,
                FontStyle::Bold,
                fs(16.5),
                col_agent + 7,
                base,
                col_agent + badge_w - 4,
                accent,
            );
        }

        // Numeric columns
        let req_s = format_tokens(m.stats.requests);
        let in_s = format_tokens(m.stats.input_tokens);
        let out_s = format_tokens(m.stats.output_tokens + m.stats.reasoning_tokens);
        let cache_s = format_tokens(m.stats.cache_total());
        let tot_s = format_tokens(m.stats.total_tokens());
        let cost_s = format_cost(m.stats.estimated_cost_usd);

        canvas.draw_text_clipped(font, &req_s, FontStyle::Regular, fs(18.0), col_req, base, col_in - 6, theme.fg_muted);
        canvas.draw_text_clipped(font, &in_s, FontStyle::Regular, fs(18.0), col_in, base, col_out - 6, theme.fg);
        canvas.draw_text_clipped(font, &out_s, FontStyle::Regular, fs(18.0), col_out, base, col_cache - 6, theme.fg);
        canvas.draw_text_clipped(font, &cache_s, FontStyle::Regular, fs(18.0), col_cache, base, col_total - 6, theme.fg_muted);
        canvas.draw_text_clipped(font, &tot_s, FontStyle::Bold, fs(18.0), col_total, base, col_cost - 6, accent);
        canvas.draw_text_clipped(font, &cost_s, FontStyle::Regular, fs(18.0), col_cost, base, x0 + content_w - 10, theme.warning);
    }

    // Subtle scroll indicator on right edge if overflowing
    if snap.models.len() > visible_rows {
        let track_h = (table_h - hdr_h - 16).max(20) as f32;
        let thumb_h = ((visible_rows as f32 / snap.models.len() as f32) * track_h).clamp(20.0, track_h);
        let frac = if max_scroll == 0 {
            0.0
        } else {
            start as f32 / max_scroll as f32
        };
        let thumb_y = (list_y as f32) + frac * (track_h - thumb_h);
        canvas.fill_rounded_rect(x0 + content_w - 6, thumb_y as i32, 3, thumb_h as i32, 1, theme.accent, 180);
    }
}

fn agent_color(agent_id: &str, theme: &Theme) -> Rgb {
    match agent_id {
        "claude" => theme.accent,
        "codex" => theme.success,
        "gemini" => theme.info,
        "opencode" => theme.warning,
        _ => theme.fg_muted,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_agent_quota_row(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    w: i32,
    label: &str,
    quota: &WindowQuota,
    accent: Rgb,
    theme: &Theme,
    font: &mut FontEngine,
    s: f32,
) {
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;
    let bar_y = y + sc(23).max(15);
    let bar_w = w.max(20);
    canvas.fill_rounded_rect(x, bar_y, bar_w, 6, 3, theme.surface_alt, 255);

    if let Some(used_pct) = quota.live_used_pct {
        // Exact server-side quota percentage (Anthropic OAuth API / Codex rate_limits)
        let rem_pct = quota.live_remaining_pct().unwrap_or(0.0);
        let bar_color = if used_pct >= 85.0 {
            theme.danger
        } else if used_pct >= 60.0 {
            theme.warning
        } else {
            theme.success
        };

        let left_str = format!("{label} {:.0}%", used_pct.round());
        let right_str = if let Some(secs) = quota.reset_in_secs {
            format!("{:.0}% left · {}", rem_pct.round(), format_duration_short(secs))
        } else {
            format!("{:.0}% left", rem_pct.round())
        };
        let right_w = font.measure_text(&right_str, fs(16.0));
        let right_x = (x + w - right_w).max(x + sc(92));

        canvas.draw_text_clipped(
            font,
            &left_str,
            FontStyle::Bold,
            fs(16.5),
            x,
            y + sc(17),
            right_x - 4,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            &right_str,
            FontStyle::Regular,
            fs(16.0),
            right_x,
            y + sc(17),
            x + w,
            bar_color,
        );

        let fill_frac = (used_pct / 100.0).clamp(0.0, 1.0);
        let fill_w = ((bar_w as f64) * fill_frac).round() as i32;
        if fill_w > 0 {
            canvas.fill_rounded_rect(x, bar_y, fill_w.max(3), 6, 3, bar_color, 255);
        }
    } else {
        // Local log telemetry only (no server-side % quota API available for this agent)
        let left_str = format!("{label} {} tok", format_tokens(quota.used_tokens));
        let right_str = if quota.requests > 0 {
            format!("{} req", format_tokens(quota.requests))
        } else {
            "idle".to_string()
        };
        let right_w = font.measure_text(&right_str, fs(16.0));
        let right_x = (x + w - right_w).max(x + sc(92));

        canvas.draw_text_clipped(
            font,
            &left_str,
            FontStyle::Bold,
            fs(16.5),
            x,
            y + sc(17),
            right_x - 4,
            if quota.used_tokens > 0 { theme.fg } else { theme.fg_muted },
        );
        canvas.draw_text_clipped(
            font,
            &right_str,
            FontStyle::Regular,
            fs(16.0),
            right_x,
            y + sc(17),
            x + w,
            if quota.used_tokens > 0 { accent } else { theme.fg_muted },
        );

        if quota.used_tokens > 0 {
            canvas.fill_rounded_rect(x, bar_y, (bar_w / 3).max(6), 6, 3, accent, 160);
        }
    }
}
