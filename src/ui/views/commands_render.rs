//! Managed Commands & Aliases tab renderer.

use crate::commands::{CommandStatus, format_elapsed};
use crate::config::{Config, Theme};
use crate::shell::shorten_path;
use std::path::Path;
use crate::ui::canvas::{Canvas, Rect};
use crate::ui::chrome::tab_bar::{CommandRowHitZone, STATUS_BAR_H, TabBarHitZones, top_reserved};
use crate::ui::font::{FontEngine, FontStyle};
use crate::ui::renderer::{CommandFormField, CommandsViewState};

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_commands_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    cv: &CommandsViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.commands_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar {
        (height.saturating_sub(STATUS_BAR_H)) as i32
    } else {
        height as i32
    };
    let pad = 20i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(400);
    let content_bottom = bottom - 12;

    // Header bar
    let hy = top + 14;
    let running_count = cv.entries.iter().filter(|e| e.status.is_active()).count();
    let hdr_btn_h = sc(38).max(26);
    let hdr_btn_base = hy + 4 + sc(25).max(17);

    // Header right buttons: "+ Add Command (a)" and optional "Stop All (S)"
    let mut rx = content_x + content_w;

    let add_label = "+ Add Command (a)";
    let add_w = font.measure_text(add_label, fs(17.5)) + 24;
    rx -= add_w;
    let add_rect = Rect {
        x: rx,
        y: hy + 4,
        w: add_w,
        h: hdr_btn_h,
    };
    canvas.fill_rounded_rect(add_rect.x, add_rect.y, add_rect.w, add_rect.h, 6, theme.accent_soft, 255);
    canvas.stroke_rounded_rect(add_rect.x, add_rect.y, add_rect.w, add_rect.h, 6, theme.accent, 220);
    canvas.draw_text_clipped(
        font,
        add_label,
        FontStyle::Bold,
        fs(17.5),
        add_rect.x + 12,
        hdr_btn_base,
        add_rect.x + add_rect.w - 4,
        theme.accent,
    );
    hit_zones.cmd_add_btn = Some(add_rect);

    if running_count > 0 {
        let stop_all_label = "Stop All (S)";
        let stop_all_w = font.measure_text(stop_all_label, fs(17.5)) + 22;
        rx -= stop_all_w + 8;
        let stop_all_rect = Rect {
            x: rx,
            y: hy + 4,
            w: stop_all_w,
            h: hdr_btn_h,
        };
        canvas.fill_rounded_rect(
            stop_all_rect.x,
            stop_all_rect.y,
            stop_all_rect.w,
            stop_all_rect.h,
            6,
            theme.surface_alt,
            255,
        );
        canvas.stroke_rounded_rect(
            stop_all_rect.x,
            stop_all_rect.y,
            stop_all_rect.w,
            stop_all_rect.h,
            6,
            theme.danger,
            200,
        );
        canvas.draw_text_clipped(
            font,
            stop_all_label,
            FontStyle::Bold,
            fs(17.5),
            stop_all_rect.x + 11,
            hdr_btn_base,
            stop_all_rect.x + stop_all_rect.w - 4,
            theme.danger,
        );
        hit_zones.cmd_stop_all_btn = Some(stop_all_rect);
    }

    let header_max_x = (rx - 12).max(content_x + 180);
    canvas.draw_text_clipped(
        font,
        "Commands",
        FontStyle::Bold,
        fs(25.0),
        content_x,
        hy + sc(24),
        header_max_x,
        theme.fg,
    );

    let sub = format!(
        "{} total  ·  {} active  ·  saved in commands.toml",
        cv.entries.len(),
        running_count
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

    let mut cur_y = hy + sc(62).max(hdr_btn_h + 14);

    // Inline Add / Edit Form card
    if let Some(ref form) = cv.form {
        let form_h = if form.error.is_some() { sc(206).max(150) } else { sc(178).max(130) };
        canvas.fill_rounded_rect(content_x, cur_y, content_w, form_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, form_h, 10, theme.accent, 220);

        let form_title = if form.is_edit {
            "EDIT COMMAND"
        } else {
            "ADD & LAUNCH COMMAND"
        };
        canvas.draw_text_clipped(
            font,
            form_title,
            FontStyle::Bold,
            fs(18.0),
            content_x + 14,
            cur_y + sc(28),
            content_x + 360,
            theme.accent,
        );
        let fhint = if cfg!(target_os = "macos") {
            "Tab switch field  ·  Cmd+V paste  ·  Enter submit  ·  Esc cancel"
        } else {
            "Tab switch field  ·  Ctrl+V paste  ·  Enter submit  ·  Esc cancel"
        };
        let fhw = font.measure_text(fhint, fs(16.5));
        if content_w > fhw + 340 {
            canvas.draw_text_clipped(
                font,
                fhint,
                FontStyle::Regular,
                fs(16.5),
                content_x + content_w - fhw - 14,
                cur_y + sc(28),
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }

        // Three side-by-side input fields: Name (22%) | Command (43%) | Directory (35%)
        let inner_x = content_x + 14;
        let inner_w = content_w - 28;
        let gap = 12i32;
        let avail_w = inner_w - 2 * gap;
        let name_fw = avail_w * 22 / 100;
        let cmd_fw = avail_w * 43 / 100;
        let dir_fw = avail_w - name_fw - cmd_fw;
        let field_y = cur_y + sc(60);
        let field_h = sc(42).max(28);

        let draw_form_field = |canvas: &mut Canvas<'_>,
                                   font: &mut FontEngine,
                                   label: &str,
                                   placeholder: &str,
                                   text: &str,
                                   cursor: usize,
                                   rect: Rect,
                                   focused: bool| {
            canvas.draw_text_clipped(
                font,
                label,
                FontStyle::Bold,
                fs(17.0),
                rect.x,
                rect.y - 8,
                rect.x + rect.w,
                if focused { theme.accent } else { theme.fg_muted },
            );
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.bg, 255);
            canvas.stroke_rounded_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                6,
                if focused { theme.accent } else { theme.border },
                255,
            );
            let text_base = rect.y + sc(28).max(18);
            if text.is_empty() && !focused {
                canvas.draw_text_clipped(
                    font,
                    placeholder,
                    FontStyle::Italic,
                    fs(18.5),
                    rect.x + 10,
                    text_base,
                    rect.x + rect.w - 10,
                    theme.fg_muted,
                );
            } else {
                let max_text_w = (rect.w - 24).max(1);
                let (shown, cursor_px) = crate::ui::text_input::visible_slice_for_cursor(
                    text,
                    cursor,
                    font,
                    fs(18.5),
                    max_text_w,
                );
                canvas.draw_text_clipped(
                    font,
                    shown,
                    FontStyle::Regular,
                    fs(18.5),
                    rect.x + 10,
                    text_base,
                    rect.x + rect.w - 10,
                    theme.fg,
                );
                if focused && cv.cursor_blink_on {
                    canvas.fill_rect(
                        (rect.x + 10 + cursor_px).min(rect.x + rect.w - 6),
                        rect.y + 7,
                        2,
                        (field_h - 14).max(12),
                        theme.accent,
                    );
                }
            }
        };

        // Name input
        let name_field_rect = Rect {
            x: inner_x,
            y: field_y,
            w: name_fw,
            h: field_h,
        };
        draw_form_field(
            canvas,
            font,
            "Name",
            "optional",
            form.name_input,
            form.name_cursor,
            name_field_rect,
            form.active_field == CommandFormField::Name,
        );
        hit_zones.cmd_form_name_field = Some(name_field_rect);

        // Command input
        let cmd_x = inner_x + name_fw + gap;
        let cmd_field_rect = Rect {
            x: cmd_x,
            y: field_y,
            w: cmd_fw,
            h: field_h,
        };
        draw_form_field(
            canvas,
            font,
            "Command",
            "e.g. docker-compose up -d",
            form.command_input,
            form.command_cursor,
            cmd_field_rect,
            form.active_field == CommandFormField::Command,
        );
        hit_zones.cmd_form_cmd_field = Some(cmd_field_rect);

        // Directory input
        let dir_x = cmd_x + cmd_fw + gap;
        let dir_field_rect = Rect {
            x: dir_x,
            y: field_y,
            w: dir_fw,
            h: field_h,
        };
        draw_form_field(
            canvas,
            font,
            "Working Directory",
            "e.g. ~/projects/my-app",
            form.directory_input,
            form.directory_cursor,
            dir_field_rect,
            form.active_field == CommandFormField::Directory,
        );
        hit_zones.cmd_form_dir_field = Some(dir_field_rect);

        // Error message if any
        let btn_y = if let Some(err) = form.error {
            canvas.draw_text_clipped(
                font,
                err,
                FontStyle::Bold,
                fs(17.0),
                inner_x,
                field_y + field_h + sc(22),
                inner_x + inner_w,
                theme.danger,
            );
            field_y + field_h + sc(36)
        } else {
            field_y + field_h + sc(18)
        };

        // Action buttons: Primary ("Add & Run" / "Save"), Secondary ("Save Only" / "Save & Run"), Cancel
        let fbtn_h = sc(38).max(26);
        let fbtn_base = btn_y + sc(25).max(17);
        let primary_label = if form.is_edit { "Save" } else { "Add & Run" };
        let pw = font.measure_text(primary_label, fs(17.5)) + 26;
        let primary_rect = Rect {
            x: inner_x,
            y: btn_y,
            w: pw,
            h: fbtn_h,
        };
        canvas.fill_rounded_rect(primary_rect.x, primary_rect.y, primary_rect.w, primary_rect.h, 6, theme.success, 40);
        canvas.stroke_rounded_rect(primary_rect.x, primary_rect.y, primary_rect.w, primary_rect.h, 6, theme.success, 230);
        canvas.draw_text_clipped(
            font,
            primary_label,
            FontStyle::Bold,
            fs(17.5),
            primary_rect.x + 13,
            fbtn_base,
            primary_rect.x + primary_rect.w - 4,
            theme.success,
        );
        hit_zones.cmd_form_primary_btn = Some(primary_rect);

        let secondary_label = if form.is_edit { "Save & Run" } else { "Save Only" };
        let sw = font.measure_text(secondary_label, fs(17.5)) + 24;
        let secondary_rect = Rect {
            x: primary_rect.x + primary_rect.w + 8,
            y: btn_y,
            w: sw,
            h: fbtn_h,
        };
        canvas.fill_rounded_rect(secondary_rect.x, secondary_rect.y, secondary_rect.w, secondary_rect.h, 6, theme.surface_alt, 255);
        canvas.stroke_rounded_rect(secondary_rect.x, secondary_rect.y, secondary_rect.w, secondary_rect.h, 6, theme.border, 220);
        canvas.draw_text_clipped(
            font,
            secondary_label,
            FontStyle::Bold,
            fs(17.5),
            secondary_rect.x + 12,
            fbtn_base,
            secondary_rect.x + secondary_rect.w - 4,
            theme.fg,
        );
        hit_zones.cmd_form_secondary_btn = Some(secondary_rect);

        let cancel_label = "Cancel";
        let cw = font.measure_text(cancel_label, fs(17.5)) + 22;
        let cancel_rect = Rect {
            x: secondary_rect.x + secondary_rect.w + 8,
            y: btn_y,
            w: cw,
            h: fbtn_h,
        };
        canvas.fill_rounded_rect(cancel_rect.x, cancel_rect.y, cancel_rect.w, cancel_rect.h, 6, theme.surface_alt, 180);
        canvas.draw_text_clipped(
            font,
            cancel_label,
            FontStyle::Regular,
            fs(17.5),
            cancel_rect.x + 11,
            fbtn_base,
            cancel_rect.x + cancel_rect.w - 4,
            theme.fg_muted,
        );
        hit_zones.cmd_form_cancel_btn = Some(cancel_rect);

        cur_y += form_h + 12;
    }

    // Split remaining space between Commands List (top ~56%) and Selected Command Output (bottom ~44%)
    let remaining_h = (content_bottom - cur_y).max(180);
    let log_h = if cv.entries.is_empty() {
        0
    } else {
        (remaining_h * 40 / 100).clamp(110, 240)
    };
    let list_h = if log_h > 0 {
        remaining_h - log_h - 12
    } else {
        remaining_h
    };

    // --- COMMANDS LIST CARD ---
    canvas.fill_rounded_rect(content_x, cur_y, content_w, list_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(content_x, cur_y, content_w, list_h, 10, theme.border, 220);

    canvas.draw_text_clipped(
        font,
        "LAUNCHED COMMANDS",
        FontStyle::Bold,
        fs(18.0),
        content_x + 14,
        cur_y + sc(28),
        content_x + 320,
        theme.fg_muted,
    );
    let hint = "a add  ·  s start/stop  ·  e edit  ·  x delete  ·  ↑↓ select  ·  Esc close";
    let hw = font.measure_text(hint, fs(16.5));
    if content_w > hw + 320 {
        canvas.draw_text_clipped(
            font,
            hint,
            FontStyle::Regular,
            fs(16.5),
            content_x + content_w - hw - 14,
            cur_y + sc(28),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }
    let list_hdr_h = sc(42).max(28);
    canvas.fill_rect(content_x + 10, cur_y + list_hdr_h, content_w - 20, 1, theme.border);

    if cv.entries.is_empty() {
        let empty_msg = "No commands yet. Click '+ Add Command' or press 'a' to launch one (e.g. docker-compose up -d).";
        canvas.draw_text_clipped(
            font,
            empty_msg,
            FontStyle::Italic,
            fs(19.0),
            content_x + 18,
            cur_y + list_hdr_h + sc(38),
            content_x + content_w - 18,
            theme.fg_muted,
        );
    } else {
        let list_y = cur_y + list_hdr_h + 8;
        let row_h = sc(72).max(46);
        let max_rows = ((list_h - list_hdr_h - 16) / row_h).max(1) as usize;
        let start = cv.scroll.min(cv.entries.len().saturating_sub(1));

        for (vi, entry) in cv.entries.iter().skip(start).take(max_rows).enumerate() {
            let real_idx = start + vi;
            let ry = list_y + (vi as i32) * row_h;
            let is_sel = real_idx == cv.selected;
            let row_rect = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: row_h - 6,
            };

            if is_sel {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, theme.accent, 140);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 8, 3, (row_rect.h - 16).max(6), 1, theme.accent, 255);
            } else {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, theme.bg, 140);
            }

            // Status color & pill
            let (st_col, st_bg_alpha) = match entry.status {
                CommandStatus::Running => (theme.success, 36),
                CommandStatus::Detached => (theme.info, 36),
                CommandStatus::Stopping => (theme.warning, 36),
                CommandStatus::Stopped | CommandStatus::Idle => (theme.fg_muted, 24),
                CommandStatus::Exited(0) => (theme.info, 28),
                CommandStatus::Exited(_) | CommandStatus::Failed => (theme.danger, 36),
            };
            let st_label = entry.status.label();
            let badge_w = sc(116).clamp(80, 190);
            let badge_h = sc(32).max(22);
            let badge_x = row_rect.x + 12;
            let badge_y = row_rect.y + (row_rect.h - badge_h) / 2;
            canvas.fill_rounded_rect(badge_x, badge_y, badge_w, badge_h, 6, st_col, st_bg_alpha);
            canvas.fill_rounded_rect(badge_x + 8, badge_y + badge_h / 2 - 3, 6, 6, 3, st_col, 255);
            canvas.draw_text_clipped(
                font,
                &st_label,
                FontStyle::Bold,
                fs(16.5),
                badge_x + 20,
                badge_y + sc(22).max(15),
                badge_x + badge_w - 4,
                st_col,
            );

            // Right-side action buttons: [Stop/Start] [Edit] [×]
            let btn_h = sc(36).max(24);
            let btn_y = row_rect.y + (row_rect.h - btn_h) / 2;
            let del_w = sc(38).max(28);
            let del_x = row_rect.x + row_rect.w - del_w - 10;
            let delete_btn = Rect {
                x: del_x,
                y: btn_y,
                w: del_w,
                h: btn_h,
            };

            let edit_w = (font.measure_text("Edit", fs(17.0)) + 24).max(48);
            let edit_x = del_x - edit_w - 6;
            let edit_btn = Rect {
                x: edit_x,
                y: btn_y,
                w: edit_w,
                h: btn_h,
            };

            let can_stop = entry.status.can_stop();
            let toggle_label = if can_stop {
                "Stop"
            } else if entry.status == CommandStatus::Stopping {
                "..."
            } else {
                "Start"
            };
            let toggle_w = (font.measure_text("Start", fs(17.0)) + 28).max(56);
            let toggle_x = edit_x - toggle_w - 6;
            let toggle_btn = Rect {
                x: toggle_x,
                y: btn_y,
                w: toggle_w,
                h: btn_h,
            };

            // Draw Toggle (Start / Stop) button
            let toggle_col = if can_stop {
                theme.danger
            } else if entry.status == CommandStatus::Stopping {
                theme.warning
            } else {
                theme.success
            };
            let act_base = btn_y + sc(24).max(16);
            canvas.fill_rounded_rect(toggle_btn.x, toggle_btn.y, toggle_btn.w, toggle_btn.h, 6, toggle_col, 32);
            canvas.stroke_rounded_rect(toggle_btn.x, toggle_btn.y, toggle_btn.w, toggle_btn.h, 6, toggle_col, 200);
            let tlw = font.measure_text(toggle_label, fs(17.0));
            canvas.draw_text_clipped(
                font,
                toggle_label,
                FontStyle::Bold,
                fs(17.0),
                toggle_btn.x + (toggle_btn.w - tlw) / 2,
                act_base,
                toggle_btn.x + toggle_btn.w - 2,
                toggle_col,
            );

            // Draw Edit button
            canvas.fill_rounded_rect(edit_btn.x, edit_btn.y, edit_btn.w, edit_btn.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(edit_btn.x, edit_btn.y, edit_btn.w, edit_btn.h, 6, theme.border, 210);
            let elw = font.measure_text("Edit", fs(17.0));
            canvas.draw_text_clipped(
                font,
                "Edit",
                FontStyle::Bold,
                fs(17.0),
                edit_btn.x + (edit_btn.w - elw) / 2,
                act_base,
                edit_btn.x + edit_btn.w - 2,
                theme.fg,
            );

            // Draw Delete (×) button
            canvas.fill_rounded_rect(delete_btn.x, delete_btn.y, delete_btn.w, delete_btn.h, 6, theme.surface_alt, 200);
            let dlw = font.measure_text("×", fs(20.0));
            canvas.draw_text_clipped(
                font,
                "×",
                FontStyle::Bold,
                fs(20.0),
                delete_btn.x + (delete_btn.w - dlw) / 2,
                act_base + 1,
                delete_btn.x + delete_btn.w - 2,
                theme.fg_muted,
            );

            // Middle text: Command (top line) & Directory + meta (bottom line)
            let text_x = badge_x + badge_w + 12;
            let text_max_x = toggle_x - 12;
            canvas.draw_text_clipped(
                font,
                entry.display_name(),
                FontStyle::Bold,
                fs(19.0),
                text_x,
                row_rect.y + sc(26).max(17),
                text_max_x,
                theme.fg,
            );

            let short_dir = shorten_path(Path::new(&entry.directory));
            let mut meta = short_dir;
            if let Some(pid) = entry.pid {
                meta.push_str(&format!("  ·  pid {pid}"));
            }
            if let Some(up) = entry.uptime_secs() {
                meta.push_str(&format!("  ·  up {}", format_elapsed(up)));
            }
            canvas.draw_text_clipped(
                font,
                &meta,
                FontStyle::Regular,
                fs(17.0),
                text_x,
                row_rect.y + sc(52).max(34),
                text_max_x,
                theme.fg_muted,
            );

            hit_zones.cmd_rows.push(CommandRowHitZone {
                index: real_idx,
                id: entry.id,
                row_rect,
                toggle_btn,
                edit_btn,
                delete_btn,
            });
        }
    }

    // --- SELECTED COMMAND OUTPUT CARD ---
    if log_h > 0 {
        let log_y = cur_y + list_h + 12;
        let log_rect = Rect {
            x: content_x,
            y: log_y,
            w: content_w,
            h: log_h,
        };
        hit_zones.cmd_log_area = Some(log_rect);

        canvas.fill_rounded_rect(log_rect.x, log_rect.y, log_rect.w, log_rect.h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(log_rect.x, log_rect.y, log_rect.w, log_rect.h, 10, theme.border, 220);

        if let Some(sel) = cv.entries.get(cv.selected) {
            let title = format!("OUTPUT  ·  {}", sel.display_name());
            canvas.draw_text_clipped(
                font,
                &title,
                FontStyle::Bold,
                fs(17.5),
                log_rect.x + 14,
                log_rect.y + sc(26).max(18),
                log_rect.x + log_rect.w - 220,
                theme.fg_muted,
            );
            let dir_short = shorten_path(Path::new(&sel.directory));
            let dw = font.measure_text(&dir_short, fs(17.0));
            canvas.draw_text_clipped(
                font,
                &dir_short,
                FontStyle::Regular,
                fs(17.0),
                (log_rect.x + log_rect.w - dw - 14).max(log_rect.x + 220),
                log_rect.y + sc(26).max(18),
                log_rect.x + log_rect.w - 10,
                theme.info,
            );
            let log_hdr_h = sc(38).max(26);
            canvas.fill_rect(log_rect.x + 10, log_rect.y + log_hdr_h, log_rect.w - 20, 1, theme.border);

            let inner_y = log_rect.y + log_hdr_h + 6;
            let line_h = sc(25).max(15);
            let visible_lines = ((log_rect.h - log_hdr_h - 14) / line_h).max(1) as usize;
            if sel.logs.is_empty() {
                canvas.draw_text_clipped(
                    font,
                    "No output yet. Press 's' or click Start to run this command.",
                    FontStyle::Italic,
                    fs(17.5),
                    log_rect.x + 14,
                    inner_y + sc(20).max(14),
                    log_rect.x + log_rect.w - 14,
                    theme.fg_muted,
                );
            } else {
                let total = sel.logs.len();
                let max_offset = total.saturating_sub(visible_lines);
                let scroll_from_bottom = cv.log_scroll.min(max_offset);
                let end = total.saturating_sub(scroll_from_bottom);
                let start = end.saturating_sub(visible_lines);
                for (i, line) in sel.logs.iter().skip(start).take(end - start).enumerate() {
                    let ly = inner_y + (i as i32) * line_h + sc(19).max(13);
                    let col = if line.starts_with('✖') {
                        theme.danger
                    } else if line.starts_with('▶') {
                        theme.success
                    } else if line.starts_with('■') {
                        theme.warning
                    } else if line.starts_with('●') {
                        theme.info
                    } else {
                        theme.fg
                    };
                    canvas.draw_text_clipped(
                        font,
                        line,
                        FontStyle::Regular,
                        fs(17.5),
                        log_rect.x + 14,
                        ly,
                        log_rect.x + log_rect.w - 14,
                        col,
                    );
                }
            }
        }
    }
}
