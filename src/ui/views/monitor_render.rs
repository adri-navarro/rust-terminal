//! btop-style System & Process Monitor tab renderer.

use crate::config::{Config, Rgb, Theme};
use crate::monitor::{ProcessSort, format_bytes, format_uptime};
use crate::ui::canvas::{Canvas, Rect};
use crate::ui::chrome::tab_bar::{STATUS_BAR_H, TabBarHitZones, top_reserved};
use crate::ui::font::{FontEngine, FontStyle};
use crate::ui::renderer::MonitorViewState;

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_monitor_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    mv: &MonitorViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.monitor_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 18i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(360);
    let content_bottom = (height as i32 - bottom - 12).max(top + 200);

    // Header bar
    let hy = top + 14;
    let btn_h = sc(36).max(24);
    let btn_base = hy + 4 + sc(24).max(16);

    // Right-side controls: Sort pills (CPU, MEM, PID, NAME) + Refresh
    let mut rx = content_x + content_w;
    let ref_label = "Refresh (Alt+R)";
    let ref_w = font.measure_text(ref_label, fs(17.5)) + 22;
    rx -= ref_w;
    let ref_rect = Rect {
        x: rx,
        y: hy + 4,
        w: ref_w,
        h: btn_h,
    };
    canvas.fill_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        ref_label,
        FontStyle::Bold,
        fs(17.5),
        ref_rect.x + 11,
        btn_base,
        ref_rect.x + ref_rect.w - 4,
        theme.fg,
    );
    hit_zones.monitor_refresh_btn = Some(ref_rect);

    rx -= 10;
    for &sort_mode in &[
        ProcessSort::Name,
        ProcessSort::Pid,
        ProcessSort::Memory,
        ProcessSort::Cpu,
    ] {
        let lbl = sort_mode.label();
        let sw = font.measure_text(lbl, fs(17.5)) + 20;
        rx -= sw + 4;
        let sr = Rect {
            x: rx,
            y: hy + 4,
            w: sw,
            h: btn_h,
        };
        let active = mv.sort == sort_mode;
        canvas.fill_rounded_rect(
            sr.x,
            sr.y,
            sr.w,
            sr.h,
            6,
            if active { theme.accent_soft } else { theme.surface },
            255,
        );
        canvas.stroke_rounded_rect(
            sr.x,
            sr.y,
            sr.w,
            sr.h,
            6,
            if active { theme.accent } else { theme.border },
            220,
        );
        canvas.draw_text_clipped(
            font,
            lbl,
            if active { FontStyle::Bold } else { FontStyle::Regular },
            fs(17.5),
            sr.x + 10,
            btn_base,
            sr.x + sr.w - 4,
            if active { theme.accent } else { theme.fg_muted },
        );
        hit_zones.monitor_sort_btns.push((sort_mode, sr));
    }

    let header_max_x = (rx - 12).max(content_x + 180);
    canvas.draw_text_clipped(
        font,
        "System Monitor",
        FontStyle::Bold,
        fs(25.0),
        content_x,
        hy + sc(24),
        header_max_x,
        theme.fg,
    );

    if let Some(snap) = mv.snapshot {
        let sub = format!(
            "up {}  ·  load {:.2} {:.2} {:.2}  ·  {} procs ({} running)",
            format_uptime(snap.uptime_secs),
            snap.load_avg.0,
            snap.load_avg.1,
            snap.load_avg.2,
            snap.total_processes,
            snap.running_processes
        );
        canvas.draw_text_clipped(
            font,
            &sub,
            FontStyle::Regular,
            fs(17.5),
            content_x,
            hy + sc(47),
            header_max_x,
            theme.fg_muted,
        );
    }

    let Some(snap) = mv.snapshot else {
        canvas.draw_text_clipped(
            font,
            "Sampling system metrics...",
            FontStyle::Italic,
            fs(20.0),
            content_x,
            hy + sc(90),
            content_x + 380,
            theme.fg_muted,
        );
        return;
    };

    // Top 3-card row: CPU | Memory (RAM & Swap) | Disks
    let cards_y = hy + sc(62).max(btn_h + 14);
    let card_gap = 12i32;
    let card_h = sc(254).clamp(170, (content_bottom - cards_y - 140).max(170));
    let card_w = (content_w - card_gap * 2) / 3;

    let cpu_x = content_x;
    let mem_x = cpu_x + card_w + card_gap;
    let disk_x = mem_x + card_w + card_gap;
    let disk_w = (content_x + content_w) - disk_x;

    // --- 1. CPU CARD ---
    canvas.fill_rounded_rect(cpu_x, cards_y, card_w, card_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(cpu_x, cards_y, card_w, card_h, 10, theme.border, 220);

    let cpu_color = load_color(snap.cpu_total_pct, theme);
    canvas.draw_text_clipped(
        font,
        "CPU",
        FontStyle::Bold,
        fs(18.0),
        cpu_x + 14,
        cards_y + sc(28),
        cpu_x + 130,
        theme.fg_muted,
    );
    let cpu_pct_str = format!("{:.1}%", snap.cpu_total_pct);
    let cpu_pw = font.measure_text(&cpu_pct_str, fs(28.0));
    canvas.draw_text_clipped(
        font,
        &cpu_pct_str,
        FontStyle::Bold,
        fs(28.0),
        cpu_x + card_w - cpu_pw - 14,
        cards_y + sc(32),
        cpu_x + card_w - 8,
        cpu_color,
    );

    // Live btop-style sparkline graph
    let graph_x = cpu_x + 14;
    let graph_y = cards_y + sc(46);
    let graph_w = card_w - 28;
    let graph_h = sc(52).max(32);
    canvas.fill_rounded_rect(graph_x, graph_y, graph_w, graph_h, 6, theme.bg, 255);
    if !snap.cpu_history.is_empty() && graph_w > 20 {
        let max_bars = 48usize;
        let bar_slot = (graph_w - 8) as f32 / max_bars as f32;
        let bar_w = (bar_slot - 1.0).max(2.0) as i32;
        let start_slot = max_bars.saturating_sub(snap.cpu_history.len());
        for (i, &pct) in snap.cpu_history.iter().enumerate() {
            let bx = graph_x + 4 + ((start_slot + i) as f32 * bar_slot) as i32;
            let bh = ((pct / 100.0) * (graph_h - 8) as f32).round().max(2.0) as i32;
            let by = graph_y + graph_h - 4 - bh;
            let col = load_color(pct, theme);
            canvas.fill_rect(bx, by, bar_w, bh, col);
        }
    }

    // Per-core mini bars (2 columns, up to 6 rows = 12 cores)
    let cores_y = graph_y + graph_h + 10;
    let max_cores = snap.cpu_cores_pct.len().min(12);
    let half_w = (card_w - 36) / 2;
    let core_row_step = sc(22).max(14);
    for (ci, &cpct) in snap.cpu_cores_pct.iter().take(max_cores).enumerate() {
        let col_idx = (ci % 2) as i32;
        let row_idx = (ci / 2) as i32;
        let cx = cpu_x + 14 + col_idx * (half_w + 8);
        let cy = cores_y + row_idx * core_row_step;
        if cy + sc(18) > cards_y + card_h - 6 {
            break;
        }
        let lbl = format!("C{ci}");
        let lbl_w = sc(36).max(24);
        canvas.draw_text_clipped(font, &lbl, FontStyle::Regular, fs(15.5), cx, cy + sc(15), cx + lbl_w, theme.fg_muted);
        let bar_x = cx + lbl_w + 2;
        let pct_col_w = sc(48).max(32);
        let bar_w = (half_w - lbl_w - pct_col_w - 6).max(16);
        canvas.fill_rounded_rect(bar_x, cy + sc(6), bar_w, 7, 3, theme.bg, 255);
        let fill_w = ((cpct / 100.0) * bar_w as f32).round() as i32;
        if fill_w > 0 {
            canvas.fill_rounded_rect(bar_x, cy + sc(6), fill_w.min(bar_w), 7, 3, load_color(cpct, theme), 255);
        }
        let pct_s = format!("{:.0}%", cpct);
        canvas.draw_text_clipped(
            font,
            &pct_s,
            FontStyle::Regular,
            fs(15.5),
            bar_x + bar_w + 4,
            cy + sc(15),
            cx + half_w,
            theme.fg_muted,
        );
    }

    // --- 2. MEMORY (RAM & SWAP) CARD ---
    canvas.fill_rounded_rect(mem_x, cards_y, card_w, card_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(mem_x, cards_y, card_w, card_h, 10, theme.border, 220);

    let mem = &snap.memory;
    let mem_color = load_color(mem.used_pct, theme);
    canvas.draw_text_clipped(
        font,
        "MEMORY (RAM)",
        FontStyle::Bold,
        fs(18.0),
        mem_x + 14,
        cards_y + sc(28),
        mem_x + 210,
        theme.fg_muted,
    );
    let mem_pct_str = format!("{:.1}%", mem.used_pct);
    let mem_pw = font.measure_text(&mem_pct_str, fs(28.0));
    canvas.draw_text_clipped(
        font,
        &mem_pct_str,
        FontStyle::Bold,
        fs(28.0),
        mem_x + card_w - mem_pw - 14,
        cards_y + sc(32),
        mem_x + card_w - 8,
        mem_color,
    );

    // Segmented RAM bar (Used + Cached + Free)
    let mbar_x = mem_x + 14;
    let mbar_y = cards_y + sc(46);
    let mbar_w = card_w - 28;
    canvas.fill_rounded_rect(mbar_x, mbar_y, mbar_w, 12, 6, theme.bg, 255);
    if mem.total_bytes > 0 {
        let used_w = ((mem.used_bytes as f64 / mem.total_bytes as f64) * mbar_w as f64).round() as i32;
        let cached_w = ((mem.cached_bytes as f64 / mem.total_bytes as f64) * mbar_w as f64).round() as i32;
        let total_fill = (used_w + cached_w).min(mbar_w);
        if total_fill > 0 {
            canvas.fill_rounded_rect(mbar_x, mbar_y, total_fill, 12, 6, theme.border, 220);
        }
        if used_w > 0 {
            canvas.fill_rounded_rect(mbar_x, mbar_y, used_w.min(mbar_w), 12, 6, mem_color, 255);
        }
    }

    // Explicit RAM breakdown rows: Used, Free, Available, Cached, Total, Swap
    let mem_rows = [
        ("Used", format_bytes(mem.used_bytes), mem_color),
        ("Available", format_bytes(mem.available_bytes), theme.success),
        ("Free", format_bytes(mem.free_bytes), theme.info),
        ("Cached", format_bytes(mem.cached_bytes), theme.fg_muted),
        ("Total", format_bytes(mem.total_bytes), theme.fg),
        (
            "Swap",
            if mem.swap_total_bytes > 0 {
                format!(
                    "{} / {}",
                    format_bytes(mem.swap_used_bytes),
                    format_bytes(mem.swap_total_bytes)
                )
            } else {
                "off".to_string()
            },
            theme.accent,
        ),
    ];
    let mem_step = sc(28).max(18);
    for (ri, (label, val, col)) in mem_rows.iter().enumerate() {
        let ry = cards_y + sc(84) + (ri as i32) * mem_step;
        if ry > cards_y + card_h - 6 {
            break;
        }
        canvas.draw_text_clipped(
            font,
            label,
            FontStyle::Regular,
            fs(17.5),
            mem_x + 14,
            ry,
            mem_x + card_w / 2,
            theme.fg_muted,
        );
        let vw = font.measure_text(val, fs(17.5));
        canvas.draw_text_clipped(
            font,
            val,
            FontStyle::Bold,
            fs(17.5),
            mem_x + card_w - vw - 14,
            ry,
            mem_x + card_w - 8,
            *col,
        );
    }

    // --- 3. DISKS CARD ---
    canvas.fill_rounded_rect(disk_x, cards_y, disk_w, card_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(disk_x, cards_y, disk_w, card_h, 10, theme.border, 220);

    canvas.draw_text_clipped(
        font,
        "DISKS",
        FontStyle::Bold,
        fs(18.0),
        disk_x + 14,
        cards_y + sc(28),
        disk_x + 140,
        theme.fg_muted,
    );
    let dcount = format!("{} mounted", snap.disks.len());
    let dcw = font.measure_text(&dcount, fs(17.0));
    canvas.draw_text_clipped(
        font,
        &dcount,
        FontStyle::Regular,
        fs(17.0),
        disk_x + disk_w - dcw - 14,
        cards_y + sc(28),
        disk_x + disk_w - 8,
        theme.fg_muted,
    );

    if snap.disks.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No mounted block disks found",
            FontStyle::Italic,
            fs(18.0),
            disk_x + 14,
            cards_y + sc(66),
            disk_x + disk_w - 14,
            theme.fg_muted,
        );
    } else {
        let disk_step = sc(66).max(46);
        for (di, d) in snap.disks.iter().take(3).enumerate() {
            let dy = cards_y + sc(48) + (di as i32) * disk_step;
            if dy + sc(46) > cards_y + card_h - 4 {
                break;
            }
            let dcol = load_color(d.used_pct, theme);
            let title = format!("{} ({})", d.mount, d.fs_type);
            let pct_s = format!("{:.0}%", d.used_pct);
            let pw = font.measure_text(&pct_s, fs(17.5));
            canvas.draw_text_clipped(
                font,
                &title,
                FontStyle::Bold,
                fs(17.5),
                disk_x + 14,
                dy + sc(17),
                disk_x + disk_w - pw - 22,
                theme.fg,
            );
            canvas.draw_text_clipped(
                font,
                &pct_s,
                FontStyle::Bold,
                fs(17.5),
                disk_x + disk_w - pw - 14,
                dy + sc(17),
                disk_x + disk_w - 8,
                dcol,
            );

            // Progress bar
            let bx = disk_x + 14;
            let bw = disk_w - 28;
            canvas.fill_rounded_rect(bx, dy + sc(25), bw, 8, 4, theme.bg, 255);
            let fw = ((d.used_pct / 100.0) * bw as f32).round() as i32;
            if fw > 0 {
                canvas.fill_rounded_rect(bx, dy + sc(25), fw.min(bw), 8, 4, dcol, 255);
            }

            let detail = format!(
                "{} used  ·  {} free  ·  {} total",
                format_bytes(d.used_bytes),
                format_bytes(d.free_bytes),
                format_bytes(d.total_bytes)
            );
            canvas.draw_text_clipped(
                font,
                &detail,
                FontStyle::Regular,
                fs(16.5),
                disk_x + 14,
                dy + sc(53),
                disk_x + disk_w - 12,
                theme.fg_muted,
            );
        }
    }

    // --- 4. PROCESSES TABLE CARD ---
    let proc_y = cards_y + card_h + 12;
    let proc_h = (content_bottom - proc_y).max(120);
    canvas.fill_rounded_rect(content_x, proc_y, content_w, proc_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(content_x, proc_y, content_w, proc_h, 10, theme.border, 220);

    // Table title + filter pill + shortcut hints
    let filtered = !mv.filter.trim().is_empty();
    let ptitle = if filtered {
        format!("PROCESSES ({} of {})", mv.visible.len(), snap.processes.len())
    } else {
        format!("PROCESSES ({})", snap.processes.len())
    };
    let title_w = font.measure_text(&ptitle, fs(18.0));
    canvas.draw_text_clipped(
        font,
        &ptitle,
        FontStyle::Bold,
        fs(18.0),
        content_x + 14,
        proc_y + sc(28),
        content_x + 14 + title_w + 4,
        theme.fg_muted,
    );
    let pill_h = sc(28).max(20);
    let pill_x = content_x + 14 + title_w + 16;
    let pill_y = proc_y + sc(28) - pill_h + sc(7);
    let pill_w = sc(260).min(content_x + content_w - 14 - pill_x).max(0);
    if pill_w >= 80 {
        let has_filter = !mv.filter.is_empty();
        canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, 6, theme.surface_alt, 255);
        canvas.stroke_rounded_rect(
            pill_x,
            pill_y,
            pill_w,
            pill_h,
            6,
            if has_filter { theme.accent } else { theme.border },
            if has_filter { 255 } else { 200 },
        );
        let text_x = pill_x + 10;
        let text_base = pill_y + pill_h - sc(8).max(5);
        let text_max = pill_x + pill_w - 8;
        if has_filter {
            // Keep the cursor visible when the query is wider than the pill.
            let avail = (text_max - text_x - 6).max(1);
            let (shown, cursor_px) = crate::ui::text_input::visible_slice_for_cursor(
                mv.filter,
                mv.filter_cursor,
                font,
                fs(17.0),
                avail,
            );
            canvas.draw_text_clipped(font, shown, FontStyle::Regular, fs(17.0), text_x, text_base, text_max, theme.fg);
            let caret_x = (text_x + cursor_px).min(text_max - 2);
            canvas.fill_rect(caret_x, pill_y + 5, 2, (pill_h - 10).max(4), theme.accent);
        } else {
            canvas.draw_text_clipped(
                font,
                "type to filter",
                FontStyle::Regular,
                fs(17.0),
                text_x,
                text_base,
                text_max,
                theme.fg_muted,
            );
        }
    }
    let pill_right = if pill_w >= 80 { pill_x + pill_w } else { content_x + 14 + title_w };
    let hint = "↑↓ select  ·  type to filter  ·  Alt+c/m/p/n sort  ·  Del terminate  ·  Alt+K kill  ·  Esc clear/close";
    let hw = font.measure_text(hint, fs(16.5));
    let hint_x = content_x + content_w - hw - 14;
    if hint_x > pill_right + 20 {
        canvas.draw_text_clipped(
            font,
            hint,
            FontStyle::Regular,
            fs(16.5),
            hint_x,
            proc_y + sc(28),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }

    // Column layout
    let col_pid = content_x + 16;
    let col_name = content_x + sc(112).clamp(70, 180);
    let col_mem_pct = content_x + content_w - sc(96).clamp(64, 150);
    let col_mem = col_mem_pct - sc(132).clamp(84, 210);
    let col_cpu = col_mem - sc(110).clamp(72, 175);
    let col_thr = col_cpu - sc(72).clamp(48, 115);
    let col_state = col_thr - sc(52).clamp(36, 85);

    let hdr_y = proc_y + sc(56);
    canvas.draw_text_clipped(
        font,
        "PID",
        FontStyle::Bold,
        fs(17.0),
        col_pid,
        hdr_y,
        col_name - 6,
        if mv.sort == ProcessSort::Pid { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "COMMAND",
        FontStyle::Bold,
        fs(17.0),
        col_name,
        hdr_y,
        col_state - 8,
        if mv.sort == ProcessSort::Name { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(font, "S", FontStyle::Bold, fs(17.0), col_state, hdr_y, col_thr - 4, theme.fg_muted);
    canvas.draw_text_clipped(font, "THR", FontStyle::Bold, fs(17.0), col_thr, hdr_y, col_cpu - 4, theme.fg_muted);
    canvas.draw_text_clipped(
        font,
        "CPU %",
        FontStyle::Bold,
        fs(17.0),
        col_cpu,
        hdr_y,
        col_mem - 6,
        if mv.sort == ProcessSort::Cpu { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "MEMORY",
        FontStyle::Bold,
        fs(17.0),
        col_mem,
        hdr_y,
        col_mem_pct - 6,
        if mv.sort == ProcessSort::Memory { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "MEM %",
        FontStyle::Bold,
        fs(17.0),
        col_mem_pct,
        hdr_y,
        content_x + content_w - 10,
        if mv.sort == ProcessSort::Memory { theme.accent } else { theme.fg_muted },
    );
    canvas.fill_rect(content_x + 10, hdr_y + 10, content_w - 20, 1, theme.border);

    let list_y = hdr_y + 16;
    let row_h = sc(34).max(20);
    let max_rows = ((proc_y + proc_h - list_y - 8) / row_h).max(1) as usize;
    let start = mv.scroll.min(mv.visible.len().saturating_sub(1));

    if filtered && mv.visible.is_empty() {
        let msg = format!("No processes match \"{}\"", mv.filter.trim());
        let mw = font.measure_text(&msg, fs(18.0));
        canvas.draw_text_clipped(
            font,
            &msg,
            FontStyle::Regular,
            fs(18.0),
            content_x + ((content_w - mw) / 2).max(14),
            list_y + sc(40),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }

    for (vi, p) in mv
        .visible
        .iter()
        .skip(start)
        .take(max_rows)
        .filter_map(|&i| snap.processes.get(i))
        .enumerate()
    {
        let real_idx = start + vi;
        let ry = list_y + (vi as i32) * row_h;
        let is_sel = real_idx == mv.selected;
        let row_rect = Rect {
            x: content_x + 8,
            y: ry,
            w: content_w - 16,
            h: row_h - 2,
        };

        if is_sel {
            canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6, theme.accent_soft, 255);
            canvas.fill_rounded_rect(row_rect.x, row_rect.y + 4, 3, (row_rect.h - 8).max(4), 1, theme.accent, 255);
        } else if vi % 2 == 1 {
            canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 4, theme.bg, 90);
        }

        let ty = ry + sc(23).max(14);
        let pid_s = format!("{}", p.pid);
        let state_s = p.state.to_string();
        let thr_s = format!("{}", p.threads);
        let cpu_s = format!("{:.1}%", p.cpu_pct);
        let mem_s = format_bytes(p.mem_bytes);
        let memp_s = format!("{:.1}%", p.mem_pct);

        let name_col = if is_sel { theme.fg } else { theme.fg.dim(theme.surface) };
        let state_col = match p.state {
            'R' => theme.success,
            'Z' | 'T' => theme.danger,
            _ => theme.fg_muted,
        };
        let cpu_col = if p.cpu_pct >= 50.0 {
            theme.danger
        } else if p.cpu_pct >= 15.0 {
            theme.warning
        } else if p.cpu_pct >= 1.0 {
            theme.success
        } else {
            theme.fg_muted
        };
        let mem_col = if p.mem_pct >= 20.0 {
            theme.danger
        } else if p.mem_pct >= 8.0 {
            theme.warning
        } else {
            theme.info
        };

        canvas.draw_text_clipped(font, &pid_s, FontStyle::Regular, fs(18.0), col_pid, ty, col_name - 6, theme.fg_muted);
        canvas.draw_text_clipped(
            font,
            &p.name,
            if is_sel { FontStyle::Bold } else { FontStyle::Regular },
            fs(18.0),
            col_name,
            ty,
            col_state - 8,
            name_col,
        );
        canvas.draw_text_clipped(font, &state_s, FontStyle::Bold, fs(17.5), col_state, ty, col_thr - 4, state_col);
        canvas.draw_text_clipped(font, &thr_s, FontStyle::Regular, fs(17.5), col_thr, ty, col_cpu - 4, theme.fg_muted);
        canvas.draw_text_clipped(
            font,
            &cpu_s,
            if p.cpu_pct >= 10.0 { FontStyle::Bold } else { FontStyle::Regular },
            fs(18.0),
            col_cpu,
            ty,
            col_mem - 6,
            cpu_col,
        );
        canvas.draw_text_clipped(font, &mem_s, FontStyle::Regular, fs(18.0), col_mem, ty, col_mem_pct - 6, mem_col);
        canvas.draw_text_clipped(
            font,
            &memp_s,
            FontStyle::Regular,
            fs(18.0),
            col_mem_pct,
            ty,
            content_x + content_w - 10,
            mem_col,
        );

        hit_zones.monitor_proc_rows.push((real_idx, row_rect));
    }
}

fn load_color(pct: f32, theme: &Theme) -> Rgb {
    if pct >= 85.0 {
        theme.danger
    } else if pct >= 60.0 {
        theme.warning
    } else {
        theme.success
    }
}
