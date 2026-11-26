//! Full-screen Docker Compose tab: user-managed list of compose directories
//! with Up / Down / Edit / Remove controls, the engine's running containers with
//! Stop controls, and streamed per-directory output.

use crate::commands::expand_directory;
use crate::config::{Config, Rgb, Theme};
use crate::docker::{ComposeDir, ComposeStatus, RunningContainer};
use crate::shell::shorten_path;
use crate::ui::font::{FontEngine, FontStyle};
use std::collections::{HashSet, VecDeque};
use crate::ui::renderer::{
    Canvas, DockerContainerRowHitZone, DockerRowHitZone, Rect, STATUS_BAR_H, TabBarHitZones, top_reserved,
};

pub struct DockerFormView<'a> {
    pub is_edit: bool,
    pub dir_input: &'a str,
    pub dir_cursor: usize,
    /// Compose file detected in the typed directory (None = not found / not a dir)
    pub detected_file: Option<&'a str>,
    pub dir_exists: bool,
    pub error: Option<&'a str>,
}

/// Which card keyboard input drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DockerFocus {
    #[default]
    Dirs,
    Containers,
}

pub struct DockerViewState<'a> {
    pub entries: &'a [ComposeDir],
    pub selected: usize,
    pub scroll: usize,
    pub log_scroll: usize,
    pub form: Option<DockerFormView<'a>>,
    pub cursor_blink_on: bool,
    /// None = engine never scanned yet.
    pub containers: &'a Option<Result<Vec<RunningContainer>, String>>,
    pub stopping: &'a HashSet<String>,
    /// Compose project names with a rebuild in flight.
    pub rebuilding: &'a HashSet<String>,
    /// Output of rebuilds of projects that are not registered directories.
    pub container_logs: &'a VecDeque<String>,
    /// True while such an unregistered rebuild is running.
    pub container_job_running: bool,
    pub container_selected: usize,
    pub container_scroll: usize,
    pub focus: DockerFocus,
}

#[allow(clippy::too_many_arguments)]
fn button(
    canvas: &mut Canvas<'_>,
    font: &mut FontEngine,
    label: &str,
    size: f32,
    rect: Rect,
    base: i32,
    fill: Rgb,
    stroke: Rgb,
    text: Rgb,
) {
    canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, fill, 255);
    canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, stroke, 210);
    canvas.draw_text_clipped(
        font,
        label,
        FontStyle::Bold,
        size,
        rect.x + 11,
        base,
        rect.x + rect.w - 4,
        text,
    );
}

fn status_color(status: ComposeStatus, theme: &Theme) -> Rgb {
    match status {
        ComposeStatus::Up => theme.success,
        ComposeStatus::Starting | ComposeStatus::Stopping => theme.warning,
        ComposeStatus::Failed => theme.danger,
        ComposeStatus::Idle | ComposeStatus::Down => theme.fg_muted,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_docker_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    dv: &DockerViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.docker_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 18i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(360);
    let content_bottom = (height as i32 - bottom - 12).max(top + 240);

    // ---------- Header ----------
    let hy = top + 14;
    let btn_h = sc(36).max(24);
    let btn_base = hy + 4 + sc(24).max(16);
    let mut rx = content_x + content_w;

    let mut header_btn = |canvas: &mut Canvas<'_>,
                          font: &mut FontEngine,
                          label: &str,
                          fill: Rgb,
                          stroke: Rgb,
                          text: Rgb|
     -> Rect {
        let w = font.measure_text(label, fs(17.5)) + 22;
        rx -= w;
        let r = Rect { x: rx, y: hy + 4, w, h: btn_h };
        button(canvas, font, label, fs(17.5), r, btn_base, fill, stroke, text);
        rx -= 8;
        r
    };

    let up_count = dv.entries.iter().filter(|e| e.status.is_up()).count();
    let busy_count: usize = dv.entries.iter().map(|e| e.jobs_running).sum();

    if up_count > 0 {
        hit_zones.docker_down_all_btn = Some(header_btn(
            canvas,
            font,
            "■ Down All",
            theme.surface,
            theme.danger,
            theme.danger,
        ));
    }
    hit_zones.docker_add_btn = Some(header_btn(
        canvas,
        font,
        "+ Add Directory (a)",
        theme.accent_soft,
        theme.accent,
        theme.accent,
    ));

    let header_max_x = (rx - 12).max(content_x + 160);
    canvas.draw_text_clipped(
        font,
        "Docker Compose",
        FontStyle::Bold,
        fs(25.0),
        content_x,
        hy + sc(24),
        header_max_x,
        theme.fg,
    );

    let dir_word = if dv.entries.len() == 1 { "directory" } else { "directories" };
    let mut sub = format!("{} {dir_word}  ·  {up_count} up", dv.entries.len());
    if busy_count > 0 {
        sub.push_str(&format!("  ·  {busy_count} job(s) running"));
    }
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

    let mut cur_y = hy + sc(62).max(btn_h + 14);

    // ---------- Add / Edit Compose Directory form ----------
    if let Some(ref form) = dv.form {
        let form_h = sc(172).max(124) + if form.error.is_some() { sc(24) } else { 0 };
        canvas.fill_rounded_rect(content_x, cur_y, content_w, form_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, form_h, 10, theme.accent, 220);

        let title = if form.is_edit {
            "EDIT COMPOSE DIRECTORY"
        } else {
            "ADD COMPOSE DIRECTORY"
        };
        canvas.draw_text_clipped(
            font,
            title,
            FontStyle::Bold,
            fs(18.0),
            content_x + 14,
            cur_y + sc(28),
            content_x + 360,
            theme.accent,
        );
        let paste = if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" };
        let fhint = format!("Tab complete dir  ·  {paste} paste  ·  Enter save  ·  Esc cancel");
        let fhw = font.measure_text(&fhint, fs(16.0));
        if content_w > fhw + 340 {
            canvas.draw_text_clipped(
                font,
                &fhint,
                FontStyle::Regular,
                fs(16.0),
                content_x + content_w - fhw - 14,
                cur_y + sc(28),
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }

        let inner_x = content_x + 14;
        let inner_w = content_w - 28;
        let field_y = cur_y + sc(62);
        let field_h = sc(42).max(28);
        canvas.draw_text_clipped(
            font,
            "Directory containing compose.yaml / docker-compose.yml",
            FontStyle::Bold,
            fs(17.0),
            inner_x,
            field_y - 8,
            inner_x + inner_w,
            theme.accent,
        );
        let field = Rect { x: inner_x, y: field_y, w: inner_w, h: field_h };
        canvas.fill_rounded_rect(field.x, field.y, field.w, field.h, 6, theme.bg, 255);
        canvas.stroke_rounded_rect(field.x, field.y, field.w, field.h, 6, theme.accent, 255);
        let text_base = field.y + sc(28).max(18);
        // Keep the cursor visible inside long paths
        let max_text_w = (field.w - 24).max(1);
        let (shown, cursor_px) = crate::ui::text_input::visible_slice_for_cursor(
            form.dir_input,
            form.dir_cursor,
            font,
            fs(18.5),
            max_text_w,
        );
        canvas.draw_text_clipped(
            font,
            shown,
            FontStyle::Regular,
            fs(18.5),
            field.x + 10,
            text_base,
            field.x + field.w - 10,
            theme.fg,
        );
        if dv.cursor_blink_on {
            canvas.fill_rect(
                (field.x + 10 + cursor_px).min(field.x + field.w - 6),
                field.y + 7,
                2,
                (field_h - 14).max(12),
                theme.accent,
            );
        }
        hit_zones.docker_form_field = Some(field);

        // Detection line + buttons
        let row_y = field_y + field_h + sc(12);
        let row_base = row_y + sc(24).max(16);
        let (det, det_col) = if !form.dir_exists {
            ("✖ Directory does not exist".to_string(), theme.danger)
        } else if let Some(f) = form.detected_file {
            (format!("✔ {f} found"), theme.success)
        } else {
            (
                "✖ No compose.yaml / docker-compose.yml in this directory".to_string(),
                theme.danger,
            )
        };

        let mut bx = content_x + content_w - 14;
        let cancel_w = font.measure_text("Cancel", fs(17.5)) + 22;
        bx -= cancel_w;
        let cancel_r = Rect { x: bx, y: row_y, w: cancel_w, h: btn_h };
        button(
            canvas,
            font,
            "Cancel",
            fs(17.5),
            cancel_r,
            row_base,
            theme.surface_alt,
            theme.border,
            theme.fg_muted,
        );
        hit_zones.docker_form_cancel_btn = Some(cancel_r);

        let primary = if form.is_edit { "Save" } else { "+ Add" };
        let pw = font.measure_text(primary, fs(17.5)) + 22;
        bx -= pw + 8;
        let primary_r = Rect { x: bx, y: row_y, w: pw, h: btn_h };
        button(
            canvas,
            font,
            primary,
            fs(17.5),
            primary_r,
            row_base,
            theme.accent_soft,
            theme.success,
            theme.success,
        );
        hit_zones.docker_form_submit_btn = Some(primary_r);

        canvas.draw_text_clipped(
            font,
            &det,
            FontStyle::Regular,
            fs(17.0),
            inner_x,
            row_base,
            bx - 12,
            det_col,
        );
        if let Some(err) = form.error {
            canvas.draw_text_clipped(
                font,
                err,
                FontStyle::Bold,
                fs(17.0),
                inner_x,
                row_base + sc(26),
                content_x + content_w - 14,
                theme.danger,
            );
        }

        cur_y += form_h + 12;
    }

    // ---------- Body: compose directories (top), running containers (middle), output (bottom) ----------
    let avail = (content_bottom - cur_y).max(160);
    let gap = 12;
    let list_hdr_h = sc(42).max(28);
    let row_h = sc(68).max(46);
    // Shown when directories exist or containers are listed, so toggling focus
    // never resizes the cards.
    let has_containers = matches!(dv.containers, Some(Ok(l)) if !l.is_empty());
    let mut log_h = if dv.entries.is_empty() && !has_containers {
        0
    } else {
        (avail * 40 / 100).clamp(sc(130).min(avail / 2), sc(260).max(150))
    };
    let one_row_h = list_hdr_h + row_h + 16;
    let cont_min = list_hdr_h + 2 * row_h + 16;
    let dirs_needed = list_hdr_h + (dv.entries.len().max(1) as i32) * row_h + 16;
    // Drop OUTPUT first when the three cards cannot fit
    if log_h > 0 && avail < one_row_h + gap + one_row_h + gap + log_h {
        log_h = 0;
    }
    let rest = if log_h > 0 { avail - log_h - gap } else { avail };
    let dirs_cap = (rest * 45 / 100).max(one_row_h);
    let mut list_h = dirs_needed
        .min(dirs_cap)
        .min(rest - gap - cont_min)
        .max(one_row_h);
    let mut cont_h = (rest - list_h - gap).max(one_row_h);
    if list_h + gap + cont_h > rest {
        // Still too tall: shrink both proportionally, keeping the title row visible
        let total = rest - gap;
        let sum = (list_h + cont_h).max(1);
        list_h = (total * list_h / sum).max(list_hdr_h);
        cont_h = (total - list_h).max(list_hdr_h);
    }
    let dirs_focused = dv.focus == DockerFocus::Dirs;

    let list_rect = Rect { x: content_x, y: cur_y, w: content_w, h: list_h };
    hit_zones.docker_dirs_area = Some(list_rect);
    canvas.fill_rounded_rect(list_rect.x, list_rect.y, list_rect.w, list_rect.h, 10, theme.surface, 255);
    let dirs_border = if dirs_focused { theme.accent } else { theme.border };
    canvas.stroke_rounded_rect(list_rect.x, list_rect.y, list_rect.w, list_rect.h, 10, dirs_border, 220);

    let title = format!("COMPOSE DIRECTORIES ({})", dv.entries.len());
    canvas.draw_text_clipped(
        font,
        &title,
        FontStyle::Bold,
        fs(18.0),
        content_x + 14,
        cur_y + sc(28),
        content_x + 320,
        theme.fg_muted,
    );
    let hint = "Tab switch pane  ·  a add  ·  u up  ·  b rebuild  ·  d down  ·  R restart  ·  l logs  ·  e edit  ·  x remove  ·  Esc close";
    let hw = font.measure_text(hint, fs(15.5));
    if content_w > hw + 320 {
        canvas.draw_text_clipped(
            font,
            hint,
            FontStyle::Regular,
            fs(15.5),
            content_x + content_w - hw - 14,
            cur_y + sc(28),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }

    canvas.fill_rect(content_x + 10, cur_y + list_hdr_h, content_w - 20, 1, theme.border);

    let list_y = cur_y + list_hdr_h + 8;
    // Rows that actually fit (0 when the card was shrunk below one row)
    let max_rows = ((list_h - list_hdr_h - 16) / row_h).max(0) as usize;
    hit_zones.docker_page_rows = max_rows.max(1);

    if dv.entries.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No compose directories yet. Click '+ Add Directory' or press 'a' to add one.",
            FontStyle::Italic,
            fs(18.5),
            content_x + 18,
            list_y + sc(28),
            content_x + content_w - 18,
            theme.fg_muted,
        );
    } else {
        let start = dv.scroll.min(dv.entries.len().saturating_sub(1));
        for (vi, entry) in dv.entries.iter().skip(start).take(max_rows).enumerate() {
            let idx = start + vi;
            let ry = list_y + (vi as i32) * row_h;
            let sel = idx == dv.selected;
            let row = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: row_h - 6,
            };

            if sel && dirs_focused {
                canvas.fill_rounded_rect(row.x, row.y, row.w, row.h, 8, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row.x, row.y, row.w, row.h, 8, theme.accent, 160);
                canvas.fill_rounded_rect(row.x, row.y + 6, 3, (row.h - 12).max(4), 1, theme.accent, 255);
            } else if sel {
                canvas.fill_rounded_rect(row.x, row.y, row.w, row.h, 8, theme.accent_soft, 100);
            } else if vi % 2 == 1 {
                canvas.fill_rounded_rect(row.x, row.y, row.w, row.h, 6, theme.bg, 90);
            }

            // Inline action buttons on the right
            let rbtn_h = sc(30).max(20);
            let rbtn_y = row.y + (row.h - rbtn_h) / 2;
            let rbtn_base = rbtn_y + (rbtn_h * 7) / 10;
            let mut bx = row.x + row.w - 10;

            let del_lbl = "Del";
            let del_w = font.measure_text(del_lbl, fs(15.5)) + 18;
            bx -= del_w;
            let delete_btn = Rect { x: bx, y: rbtn_y, w: del_w, h: rbtn_h };
            button(
                canvas,
                font,
                del_lbl,
                fs(15.5),
                delete_btn,
                rbtn_base,
                theme.surface_alt,
                theme.border,
                theme.fg_muted,
            );

            let edit_lbl = "Edit";
            let edit_w = font.measure_text(edit_lbl, fs(15.5)) + 18;
            bx -= edit_w + 6;
            let edit_btn = Rect { x: bx, y: rbtn_y, w: edit_w, h: rbtn_h };
            button(
                canvas,
                font,
                edit_lbl,
                fs(15.5),
                edit_btn,
                rbtn_base,
                theme.surface_alt,
                theme.border,
                theme.fg,
            );

            let down_lbl = "■ Down";
            let down_w = font.measure_text(down_lbl, fs(15.5)) + 20;
            bx -= down_w + 6;
            let down_btn = Rect { x: bx, y: rbtn_y, w: down_w, h: rbtn_h };
            button(
                canvas,
                font,
                down_lbl,
                fs(15.5),
                down_btn,
                rbtn_base,
                theme.surface,
                theme.danger,
                theme.danger,
            );

            let rebuild_lbl = "⟳ Rebuild";
            let rebuild_w = font.measure_text(rebuild_lbl, fs(15.5)) + 20;
            bx -= rebuild_w + 6;
            let rebuild_btn = Rect { x: bx, y: rbtn_y, w: rebuild_w, h: rbtn_h };
            button(
                canvas,
                font,
                rebuild_lbl,
                fs(15.5),
                rebuild_btn,
                rbtn_base,
                theme.surface,
                theme.info,
                theme.info,
            );

            let up_lbl = "▶ Up";
            let up_w = font.measure_text(up_lbl, fs(15.5)) + 20;
            bx -= up_w + 6;
            let up_btn = Rect { x: bx, y: rbtn_y, w: up_w, h: rbtn_h };
            button(
                canvas,
                font,
                up_lbl,
                fs(15.5),
                up_btn,
                rbtn_base,
                theme.accent_soft,
                theme.success,
                theme.success,
            );

            // Status pill on the left
            let st_col = status_color(entry.status, theme);
            let st_lbl = entry.status.label();
            let pill_w = font.measure_text(st_lbl, fs(14.5)) + 18;
            let pill_h = sc(24).max(16);
            let pill_x = row.x + 12;
            let pill_y = row.y + (row.h - pill_h) / 2;
            canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, 5, theme.bg, 220);
            canvas.stroke_rounded_rect(pill_x, pill_y, pill_w, pill_h, 5, st_col, 200);
            canvas.draw_text_clipped(
                font,
                st_lbl,
                FontStyle::Bold,
                fs(14.5),
                pill_x + 9,
                pill_y + (pill_h * 7) / 10,
                pill_x + pill_w - 4,
                st_col,
            );

            // Folder name + compose file on top line; shortened path on bottom line
            let text_x = pill_x + pill_w + 14;
            let text_max = (bx - 12).max(text_x + 40);
            let name = entry.display_name();
            let top_y = row.y + sc(25).max(16);
            let name_end = canvas.draw_text_clipped(
                font,
                &name,
                FontStyle::Bold,
                fs(18.5),
                text_x,
                top_y,
                text_max,
                theme.fg,
            );

            let (file_txt, file_col) = match entry.compose_file() {
                Some(f) => (format!("  ·  {f}"), theme.fg_muted),
                None => ("  ·  compose file missing".to_string(), theme.warning),
            };
            if name_end + 20 < text_max {
                canvas.draw_text_clipped(
                    font,
                    &file_txt,
                    FontStyle::Regular,
                    fs(15.5),
                    name_end + 2,
                    top_y,
                    text_max,
                    file_col,
                );
            }

            let expanded = expand_directory(&entry.directory);
            let short_dir = shorten_path(&expanded);
            let bot_y = row.y + sc(48).max(33);
            canvas.draw_text_clipped(
                font,
                &short_dir,
                FontStyle::Regular,
                fs(15.5),
                text_x,
                bot_y,
                text_max,
                theme.fg_muted,
            );

            hit_zones.docker_rows.push(DockerRowHitZone {
                index: idx,
                id: entry.id,
                row_rect: row,
                up_btn,
                rebuild_btn,
                down_btn,
                edit_btn,
                delete_btn,
            });
        }
    }

    // ---------- Running containers card ----------
    let cont_rect = Rect { x: content_x, y: cur_y + list_h + gap, w: content_w, h: cont_h };
    hit_zones.docker_containers_area = Some(cont_rect);
    canvas.fill_rounded_rect(cont_rect.x, cont_rect.y, cont_rect.w, cont_rect.h, 10, theme.surface, 255);
    let cont_border = if dirs_focused { theme.border } else { theme.accent };
    canvas.stroke_rounded_rect(cont_rect.x, cont_rect.y, cont_rect.w, cont_rect.h, 10, cont_border, 220);

    let loaded = match dv.containers {
        Some(Ok(list)) => Some(list.as_slice()),
        _ => None,
    };
    let ctitle = match loaded {
        Some(list) => format!("RUNNING CONTAINERS ({})", list.len()),
        None => "RUNNING CONTAINERS".to_string(),
    };
    canvas.draw_text_clipped(
        font,
        &ctitle,
        FontStyle::Bold,
        fs(18.0),
        content_x + 14,
        cont_rect.y + sc(28),
        content_x + 360,
        theme.fg_muted,
    );
    let chint = "Tab switch pane  ·  b rebuild  ·  s stop  ·  r refresh";
    let chw = font.measure_text(chint, fs(15.5));
    if content_w > chw + 400 {
        canvas.draw_text_clipped(
            font,
            chint,
            FontStyle::Regular,
            fs(15.5),
            content_x + content_w - chw - 14,
            cont_rect.y + sc(28),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }
    canvas.fill_rect(cont_rect.x + 10, cont_rect.y + list_hdr_h, cont_rect.w - 20, 1, theme.border);

    let cont_y = cont_rect.y + list_hdr_h + 8;
    let cont_rows = ((cont_h - list_hdr_h - 16) / row_h).max(0) as usize;
    hit_zones.docker_container_page_rows = cont_rows.max(1);
    let msg_x = content_x + 18;
    let msg_max = content_x + content_w - 18;
    match dv.containers {
        None => {
            canvas.draw_text_clipped(
                font,
                "Loading containers…",
                FontStyle::Italic,
                fs(18.5),
                msg_x,
                cont_y + sc(28),
                msg_max,
                theme.fg_muted,
            );
        }
        Some(Err(e)) => {
            canvas.draw_text_clipped(
                font,
                &format!("Docker unavailable: {e}"),
                FontStyle::Regular,
                fs(18.5),
                msg_x,
                cont_y + sc(28),
                msg_max,
                theme.warning,
            );
        }
        Some(Ok(list)) if list.is_empty() => {
            canvas.draw_text_clipped(
                font,
                "No running containers.",
                FontStyle::Italic,
                fs(18.5),
                msg_x,
                cont_y + sc(28),
                msg_max,
                theme.fg_muted,
            );
        }
        Some(Ok(list)) => {
            let start = dv.container_scroll.min(list.len().saturating_sub(1));
            for (vi, c) in list.iter().skip(start).take(cont_rows).enumerate() {
                let idx = start + vi;
                let ry = cont_y + (vi as i32) * row_h;
                let sel = idx == dv.container_selected;
                let row = Rect { x: content_x + 10, y: ry, w: content_w - 20, h: row_h - 6 };
                if sel && !dirs_focused {
                    canvas.fill_rounded_rect(row.x, row.y, row.w, row.h, 8, theme.accent_soft, 255);
                    canvas.stroke_rounded_rect(row.x, row.y, row.w, row.h, 8, theme.accent, 160);
                    canvas.fill_rounded_rect(row.x, row.y + 6, 3, (row.h - 12).max(4), 1, theme.accent, 255);
                } else if sel {
                    canvas.fill_rounded_rect(row.x, row.y, row.w, row.h, 8, theme.accent_soft, 100);
                } else if vi % 2 == 1 {
                    canvas.fill_rounded_rect(row.x, row.y, row.w, row.h, 6, theme.bg, 90);
                }

                let stopping = dv.stopping.contains(&c.id);
                let rebuilding = dv.rebuilding.contains(&c.project) && c.can_rebuild();
                let busy = stopping || rebuilding;
                let rbtn_h = sc(30).max(20);
                let rbtn_y = row.y + (row.h - rbtn_h) / 2;
                let rbtn_base = rbtn_y + (rbtn_h * 7) / 10;
                let stop_lbl = "■ Stop";
                let stop_w = font.measure_text(stop_lbl, fs(15.5)) + 20;
                let bx = row.x + row.w - 10 - stop_w;
                let stop_btn = Rect { x: bx, y: rbtn_y, w: stop_w, h: rbtn_h };
                let (stroke, text) = if busy {
                    (theme.fg_muted, theme.fg_muted)
                } else {
                    (theme.danger, theme.danger)
                };
                button(
                    canvas,
                    font,
                    stop_lbl,
                    fs(15.5),
                    stop_btn,
                    rbtn_base,
                    theme.surface,
                    stroke,
                    text,
                );

                let rebuild_btn = if c.can_rebuild() {
                    let rb_lbl = "⟳ Rebuild";
                    let rb_w = font.measure_text(rb_lbl, fs(15.5)) + 20;
                    let r = Rect { x: bx - 6 - rb_w, y: rbtn_y, w: rb_w, h: rbtn_h };
                    let col = if busy { theme.fg_muted } else { theme.info };
                    button(canvas, font, rb_lbl, fs(15.5), r, rbtn_base, theme.surface, col, col);
                    Some(r)
                } else {
                    None
                };
                let btns_x = rebuild_btn.map_or(bx, |r| r.x);

                let (st_lbl, st_col) = if stopping {
                    ("STOPPING", theme.warning)
                } else if rebuilding {
                    ("REBUILDING", theme.warning)
                } else {
                    ("RUNNING", theme.success)
                };
                let pill_w = font.measure_text("REBUILDING", fs(14.5)) + 18;
                let pill_h = sc(24).max(16);
                let pill_x = row.x + 12;
                let pill_y = row.y + (row.h - pill_h) / 2;
                canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, 5, theme.bg, 220);
                canvas.stroke_rounded_rect(pill_x, pill_y, pill_w, pill_h, 5, st_col, 200);
                canvas.draw_text_clipped(
                    font,
                    st_lbl,
                    FontStyle::Bold,
                    fs(14.5),
                    pill_x + 9,
                    pill_y + (pill_h * 7) / 10,
                    pill_x + pill_w - 4,
                    st_col,
                );

                let text_x = pill_x + pill_w + 14;
                let text_max = (btns_x - 12).max(text_x + 40);
                let top_y = row.y + sc(25).max(16);
                let name_end = canvas.draw_text_clipped(
                    font,
                    &c.name,
                    FontStyle::Bold,
                    fs(18.5),
                    text_x,
                    top_y,
                    text_max,
                    theme.fg,
                );
                if name_end + 20 < text_max && !c.image.is_empty() {
                    canvas.draw_text_clipped(
                        font,
                        &format!("  ·  {}", c.image),
                        FontStyle::Regular,
                        fs(15.5),
                        name_end + 2,
                        top_y,
                        text_max,
                        theme.fg_muted,
                    );
                }
                let mut parts: Vec<String> = Vec::new();
                if !c.status.is_empty() {
                    parts.push(c.status.clone());
                }
                if !c.ports.is_empty() {
                    parts.push(c.ports.clone());
                }
                if !c.project.is_empty() {
                    parts.push(format!("project {}", c.project));
                }
                canvas.draw_text_clipped(
                    font,
                    &parts.join("  ·  "),
                    FontStyle::Regular,
                    fs(15.5),
                    text_x,
                    row.y + sc(48).max(33),
                    text_max,
                    theme.fg_muted,
                );

                hit_zones.docker_container_rows.push(DockerContainerRowHitZone {
                    index: idx,
                    row_rect: row,
                    rebuild_btn,
                    stop_btn,
                });
            }
        }
    }

    // ---------- Output Card (for selected directory) ----------
    if log_h > 0 {
        let by = cont_rect.y + cont_h + gap;
        let out_rect = Rect { x: content_x, y: by, w: content_w, h: log_h };
        hit_zones.docker_log_area = Some(out_rect);
        canvas.fill_rounded_rect(out_rect.x, out_rect.y, out_rect.w, out_rect.h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(out_rect.x, out_rect.y, out_rect.w, out_rect.h, 10, theme.border, 220);

        let sel_entry = dv.entries.get(dv.selected);
        let (ot, is_running) = if dirs_focused {
            match sel_entry {
                Some(e) if e.jobs_running > 0 => {
                    (format!("OUTPUT — {}  ·  running…", e.display_name()), true)
                }
                Some(e) => (format!("OUTPUT — {}", e.display_name()), false),
                None => ("OUTPUT".to_string(), false),
            }
        } else if dv.container_job_running {
            ("OUTPUT — containers  ·  running…".to_string(), true)
        } else {
            ("OUTPUT — containers".to_string(), false)
        };
        canvas.draw_text_clipped(
            font,
            &ot,
            FontStyle::Bold,
            fs(17.5),
            out_rect.x + 14,
            out_rect.y + sc(26).max(18),
            out_rect.x + out_rect.w - 10,
            if is_running { theme.warning } else { theme.fg_muted },
        );
        let ohint = if dirs_focused { "PgUp/PgDn scroll  ·  c clear" } else { "PgUp/PgDn scroll" };
        let ohw = font.measure_text(ohint, fs(15.5));
        if content_w > ohw + 260 {
            canvas.draw_text_clipped(
                font,
                ohint,
                FontStyle::Regular,
                fs(15.5),
                out_rect.x + out_rect.w - ohw - 14,
                out_rect.y + sc(26).max(18),
                out_rect.x + out_rect.w - 10,
                theme.fg_muted,
            );
        }
        let ohdr = sc(38).max(26);
        canvas.fill_rect(out_rect.x + 10, out_rect.y + ohdr, out_rect.w - 20, 1, theme.border);
        let inner_y = out_rect.y + ohdr + 6;
        let line_h = sc(24).max(15);
        let visible = ((out_rect.h - ohdr - 14) / line_h).max(1) as usize;

        let logs = if dirs_focused {
            sel_entry.map(|e| &e.logs)
        } else {
            Some(dv.container_logs)
        };
        if logs.is_none_or(|l| l.is_empty()) {
            let empty_hint = if dirs_focused {
                "Press 'u' (Up), 'b' (Rebuild), 'd' (Down), 'R' (Restart), or 'l' (Logs) to run compose here."
            } else {
                "Press 'b' to rebuild the selected container's compose project."
            };
            canvas.draw_text_clipped(
                font,
                empty_hint,
                FontStyle::Italic,
                fs(17.0),
                out_rect.x + 14,
                inner_y + sc(20).max(14),
                out_rect.x + out_rect.w - 14,
                theme.fg_muted,
            );
        } else if let Some(logs) = logs {
            let total = logs.len();
            let max_off = total.saturating_sub(visible);
            let off = dv.log_scroll.min(max_off);
            let end = total - off;
            let start = end.saturating_sub(visible);
            for (i, line) in logs.iter().skip(start).take(end - start).enumerate() {
                let ly = inner_y + (i as i32) * line_h + sc(18).max(13);
                let col = if line.starts_with('✖') {
                    theme.danger
                } else if line.starts_with('▶') {
                    theme.accent
                } else if line.starts_with('✔') {
                    theme.success
                } else {
                    theme.fg
                };
                canvas.draw_text_clipped(
                    font,
                    line,
                    FontStyle::Regular,
                    fs(16.5),
                    out_rect.x + 14,
                    ly,
                    out_rect.x + out_rect.w - 14,
                    col,
                );
            }
        }
    }
}
