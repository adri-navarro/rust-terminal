//! Settings tab renderer.

use crate::config::{Config, Theme};
use crate::ui::canvas::{Canvas, Rect};
use crate::ui::chrome::tab_bar::{STATUS_BAR_H, TabBarHitZones, top_reserved};
use crate::ui::font::{FontEngine, FontStyle};
use crate::ui::renderer::{SettingsFontTarget, SettingsShortcutTarget, SettingsViewState};

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_settings_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    sv: &SettingsViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 22i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(400);
    let content_bottom = (height as i32 - bottom - 12).max(top + 200);

    // Header
    let hy = top + 14;
    let reset_label = "Reset Defaults";
    let reset_w = font.measure_text(reset_label, 15.5) + 24;
    let reset_rect = Rect {
        x: content_x + content_w - reset_w,
        y: hy + 4,
        w: reset_w,
        h: 34,
    };
    canvas.fill_rounded_rect(reset_rect.x, reset_rect.y, reset_rect.w, reset_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(reset_rect.x, reset_rect.y, reset_rect.w, reset_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        reset_label,
        FontStyle::Bold,
        15.5,
        reset_rect.x + 12,
        reset_rect.y + 22,
        reset_rect.x + reset_rect.w - 4,
        theme.fg_muted,
    );
    hit_zones.settings_reset_all_btn = Some(reset_rect);

    let header_max_x = (reset_rect.x - 12).max(content_x + 180);
    canvas.draw_text_clipped(
        font,
        "Settings",
        FontStyle::Bold,
        24.0,
        content_x,
        hy + 22,
        header_max_x,
        theme.fg,
    );
    canvas.draw_text_clipped(
        font,
        "Changes apply live and save automatically to ~/.config/itsadri-terminal/config.toml",
        FontStyle::Regular,
        15.5,
        content_x,
        hy + 44,
        header_max_x,
        theme.fg_muted,
    );

    let body_top = hy + 58;
    canvas.fill_rect(content_x, body_top, content_w, 1, theme.border);
    let scroll_px = (sv.scroll as i32) * 32;
    let mut cur_y = body_top + 14 - scroll_px;

    // --- CARD 1: FONT SIZES ---
    let font_rows = [
        (
            SettingsFontTarget::Terminal,
            "Terminal",
            "Main terminal grid & shell output",
            cfg.font_size,
            0usize,
        ),
        (
            SettingsFontTarget::Commands,
            "Commands",
            "Background commands list, form & logs",
            cfg.commands_font_size,
            1usize,
        ),
        (
            SettingsFontTarget::Docker,
            "Docker",
            "Containers, compose projects & output",
            cfg.docker_font_size,
            2usize,
        ),
        (
            SettingsFontTarget::Monitor,
            "Monitor",
            "CPU, RAM, disks & process table",
            cfg.monitor_font_size,
            3usize,
        ),
        (
            SettingsFontTarget::Usage,
            "Usage",
            "Agent & model token usage dashboard",
            cfg.usage_font_size,
            4usize,
        ),
    ];

    let font_row_h = 46i32;
    let font_card_h = 44 + (font_rows.len() as i32) * font_row_h + 10;
    hit_zones.settings_row_spans.clear();
    hit_zones.settings_viewport_h = content_bottom - body_top;
    for i in 0..font_rows.len() as i32 {
        let top_y = cur_y + 44 + i * font_row_h + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + font_row_h - 6));
    }
    if cur_y + font_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, font_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, font_card_h, 10, theme.border, 210);

        canvas.draw_text_clipped(
            font,
            "FONT SIZES",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 260,
            theme.fg_muted,
        );
        let fhint = "← / → adjust  ·  9–36 px";
        let fhw = font.measure_text(fhint, 14.5);
        if content_w > fhw + 260 {
            canvas.draw_text_clipped(
                font,
                fhint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - fhw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        for (i, &(target, label, desc, size, row_idx)) in font_rows.iter().enumerate() {
            let ry = cur_y + 44 + (i as i32) * font_row_h;
            let row_rect = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: font_row_h - 6,
            };
            let is_sel = sv.selected == row_idx;
            if is_sel {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent, 140);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 6, 3, row_rect.h - 12, 1, theme.accent, 255);
            } else if i % 2 == 1 {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6, theme.bg, 90);
            }

            // Right controls: [-]  [18 px]  [+]
            let btn_s = 30i32;
            let btn_y = row_rect.y + (row_rect.h - btn_s) / 2;
            let plus_rect = Rect {
                x: row_rect.x + row_rect.w - btn_s - 10,
                y: btn_y,
                w: btn_s,
                h: btn_s,
            };
            let val_w = 72i32;
            let val_x = plus_rect.x - val_w - 6;
            let minus_rect = Rect {
                x: val_x - btn_s - 6,
                y: btn_y,
                w: btn_s,
                h: btn_s,
            };

            // Minus button
            canvas.fill_rounded_rect(minus_rect.x, minus_rect.y, minus_rect.w, minus_rect.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(minus_rect.x, minus_rect.y, minus_rect.w, minus_rect.h, 6, theme.border, 220);
            let mw = font.measure_text("-", 18.0);
            canvas.draw_text_clipped(
                font,
                "-",
                FontStyle::Bold,
                18.0,
                minus_rect.x + (minus_rect.w - mw) / 2,
                minus_rect.y + 21,
                minus_rect.x + minus_rect.w,
                theme.fg,
            );

            // Current size pill
            canvas.fill_rounded_rect(val_x, btn_y, val_w, btn_s, 6, theme.bg, 255);
            canvas.stroke_rounded_rect(val_x, btn_y, val_w, btn_s, 6, theme.border, 180);
            let sz_str = format!("{} px", size.round() as i32);
            let szw = font.measure_text(&sz_str, 15.5);
            canvas.draw_text_clipped(
                font,
                &sz_str,
                FontStyle::Bold,
                15.5,
                val_x + (val_w - szw) / 2,
                btn_y + 20,
                val_x + val_w - 2,
                theme.accent,
            );

            // Plus button
            canvas.fill_rounded_rect(plus_rect.x, plus_rect.y, plus_rect.w, plus_rect.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(plus_rect.x, plus_rect.y, plus_rect.w, plus_rect.h, 6, theme.border, 220);
            let pw = font.measure_text("+", 18.0);
            canvas.draw_text_clipped(
                font,
                "+",
                FontStyle::Bold,
                18.0,
                plus_rect.x + (plus_rect.w - pw) / 2,
                plus_rect.y + 21,
                plus_rect.x + plus_rect.w,
                theme.fg,
            );

            // Left label + description
            let ty = row_rect.y + 25;
            let label_w = font.measure_text(label, 17.0);
            canvas.draw_text_clipped(
                font,
                label,
                FontStyle::Bold,
                17.0,
                row_rect.x + 14,
                ty,
                minus_rect.x - 14,
                if is_sel { theme.fg } else { theme.fg.dim(theme.surface) },
            );
            let desc_x = row_rect.x + 14 + label_w.max(100) + 16;
            if desc_x + 60 < minus_rect.x - 10 {
                canvas.draw_text_clipped(
                    font,
                    desc,
                    FontStyle::Regular,
                    15.0,
                    desc_x,
                    ty,
                    minus_rect.x - 14,
                    theme.fg_muted,
                );
            }

            if row_rect.y >= body_top && row_rect.y + row_rect.h <= content_bottom + 20 {
                hit_zones
                    .settings_font_btns
                    .push((target, row_rect, minus_rect, plus_rect));
            }
        }
    }

    cur_y += font_card_h + 14;

    // --- CARD 2: FONT FAMILY ---
    let font_fam_card_h = 104i32;
    let font_fam_sel = sv.selected == 5;
    {
        let top_y = cur_y + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + font_fam_card_h));
    }
    if cur_y + font_fam_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, font_fam_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(
            content_x,
            cur_y,
            content_w,
            font_fam_card_h,
            10,
            if font_fam_sel { theme.accent } else { theme.border },
            if font_fam_sel { 240 } else { 210 },
        );

        canvas.draw_text_clipped(
            font,
            "FONT FAMILY",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 220,
            if font_fam_sel { theme.accent } else { theme.fg_muted },
        );
        let ffhint = "Click a font or press ← / →";
        let ffhw = font.measure_text(ffhint, 14.5);
        if content_w > ffhw + 220 {
            canvas.draw_text_clipped(
                font,
                ffhint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - ffhw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        let pill_gap = 10i32;
        let inner_w = content_w - 28;
        let pill_h = 46i32;
        let pill_y = cur_y + 46;

        if sv.font_presets.is_empty() {
            let fname = font
                .active_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("System Monospace")
                .to_string();
            canvas.draw_text_clipped(
                font,
                &fname,
                FontStyle::Regular,
                15.0,
                content_x + 14,
                pill_y + 28,
                content_x + content_w - 14,
                theme.fg_muted,
            );
        } else {
            let count = sv.font_presets.len() as i32;
            let pill_w = ((inner_w - pill_gap * (count - 1)) / count).max(110);
            for (i, preset) in sv.font_presets.iter().enumerate() {
                let px = content_x + 14 + (i as i32) * (pill_w + pill_gap);
                if px + 40 > content_x + content_w - 14 {
                    break;
                }
                let rect = Rect {
                    x: px,
                    y: pill_y,
                    w: pill_w.min(content_x + content_w - 14 - px),
                    h: pill_h,
                };
                let active = preset.regular == font.active_path;

                canvas.fill_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    8,
                    if active { theme.accent_soft } else { theme.bg },
                    255,
                );
                canvas.stroke_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    8,
                    if active { theme.accent } else { theme.border },
                    if active { 255 } else { 180 },
                );

                // Status dot + "Aa" badge
                let dot_col = if active { theme.accent } else { theme.fg_muted };
                canvas.fill_rounded_rect(rect.x + 10, rect.y + 11, 7, 7, 3, dot_col, 255);
                canvas.draw_text_clipped(
                    font,
                    "Aa",
                    FontStyle::Bold,
                    12.5,
                    rect.x + 22,
                    rect.y + 19,
                    rect.x + rect.w - 8,
                    if active { theme.accent } else { theme.fg_muted },
                );

                canvas.draw_text_clipped(
                    font,
                    &preset.label,
                    if active { FontStyle::Bold } else { FontStyle::Regular },
                    14.5,
                    rect.x + 10,
                    rect.y + 38,
                    rect.x + rect.w - 8,
                    if active { theme.accent } else { theme.fg },
                );

                if rect.y >= body_top && rect.y + rect.h <= content_bottom + 20 {
                    hit_zones
                        .settings_font_family_btns
                        .push((preset.regular.to_string_lossy().into_owned(), rect));
                }
            }
        }
    }

    cur_y += font_fam_card_h + 14;

    // --- CARD 3: THEME ---
    let theme_card_h = 104i32;
    let theme_sel = sv.selected == 6;
    {
        let top_y = cur_y + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + theme_card_h));
    }
    if cur_y + theme_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, theme_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(
            content_x,
            cur_y,
            content_w,
            theme_card_h,
            10,
            if theme_sel { theme.accent } else { theme.border },
            if theme_sel { 240 } else { 210 },
        );

        canvas.draw_text_clipped(
            font,
            "THEME",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 200,
            if theme_sel { theme.accent } else { theme.fg_muted },
        );
        let thint = "Click a theme or press ← / →  (F2 cycles anytime)";
        let thw = font.measure_text(thint, 14.5);
        if content_w > thw + 200 {
            canvas.draw_text_clipped(
                font,
                thint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - thw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        let pill_gap = 10i32;
        let inner_w = content_w - 28;
        let count = Theme::ALL_NAMES.len() as i32;
        let pill_w = ((inner_w - pill_gap * (count - 1)) / count).max(90);
        let pill_h = 46i32;
        let pill_y = cur_y + 46;

        for (i, &tname) in Theme::ALL_NAMES.iter().enumerate() {
            let px = content_x + 14 + (i as i32) * (pill_w + pill_gap);
            let rect = Rect {
                x: px,
                y: pill_y,
                w: pill_w,
                h: pill_h,
            };
            let preview = Theme::by_name(tname);
            let active = theme.name.eq_ignore_ascii_case(tname);

            canvas.fill_rounded_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                8,
                if active { theme.accent_soft } else { theme.bg },
                255,
            );
            canvas.stroke_rounded_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                8,
                if active { theme.accent } else { theme.border },
                if active { 255 } else { 180 },
            );

            // Mini color swatch dots (bg, accent, success, danger)
            let swatches = [preview.bg, preview.accent, preview.success, preview.danger];
            for (si, &col) in swatches.iter().enumerate() {
                let sx = rect.x + 10 + (si as i32) * 14;
                let sy = rect.y + 10;
                canvas.fill_rounded_rect(sx, sy, 10, 10, 5, col, 255);
                canvas.stroke_rounded_rect(sx, sy, 10, 10, 5, theme.border, 150);
            }

            canvas.draw_text_clipped(
                font,
                tname,
                if active { FontStyle::Bold } else { FontStyle::Regular },
                14.5,
                rect.x + 10,
                rect.y + 37,
                rect.x + rect.w - 8,
                if active { theme.accent } else { theme.fg },
            );

            if rect.y >= body_top && rect.y + rect.h <= content_bottom + 20 {
                hit_zones.settings_theme_btns.push((tname, rect));
            }
        }
    }

    cur_y += theme_card_h + 14;

    // --- CARD 4: SHORTCUTS ---
    let shortcut_rows = [
        (
            SettingsShortcutTarget::Commands,
            "Commands Tab",
            cfg.commands_shortcut().display(),
            7usize,
        ),
        (
            SettingsShortcutTarget::Docker,
            "Docker Tab",
            cfg.docker_shortcut().display(),
            8usize,
        ),
        (
            SettingsShortcutTarget::Monitor,
            "Monitor Tab",
            cfg.monitor_shortcut().display(),
            9usize,
        ),
        (
            SettingsShortcutTarget::Usage,
            "Usage Tab",
            cfg.usage_shortcut().display(),
            10usize,
        ),
        (
            SettingsShortcutTarget::History,
            "History Search",
            cfg.history_shortcut().display(),
            11usize,
        ),
    ];

    let sc_row_h = 48i32;
    let extra_err = if sv.error.is_some() { 26 } else { 0 };
    let sc_card_h = 44 + (shortcut_rows.len() as i32) * sc_row_h + 12 + extra_err;
    for i in 0..shortcut_rows.len() as i32 {
        let top_y = cur_y + 44 + i * sc_row_h + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + sc_row_h - 6));
    }
    hit_zones.settings_content_h = cur_y + sc_card_h + 14 + scroll_px - body_top;
    if cur_y + sc_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, sc_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, sc_card_h, 10, theme.border, 210);

        canvas.draw_text_clipped(
            font,
            "SHORTCUTS",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 260,
            theme.fg_muted,
        );
        let shint = if cfg!(target_os = "macos") {
            "Click Change or press Enter, then press new shortcut (Cmd/Alt/F1-F12)"
        } else {
            "Click Change or press Enter, then press new shortcut (Ctrl/Alt/F1-F12)"
        };
        let shw = font.measure_text(shint, 14.5);
        if content_w > shw + 260 {
            canvas.draw_text_clipped(
                font,
                shint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - shw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        for (i, (target, label, key_str, row_idx)) in shortcut_rows.iter().enumerate() {
            let ry = cur_y + 44 + (i as i32) * sc_row_h;
            let row_rect = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: sc_row_h - 6,
            };
            let is_sel = sv.selected == *row_idx;
            let is_rec = sv.recording_shortcut == Some(*target);

            if is_rec {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.warning, 30);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.warning, 230);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 6, 3, row_rect.h - 12, 1, theme.warning, 255);
            } else if is_sel {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent, 140);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 6, 3, row_rect.h - 12, 1, theme.accent, 255);
            } else if i % 2 == 1 {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6, theme.bg, 90);
            }

            // Right buttons: [Change / Cancel]  [Reset]
            let btn_h = 30i32;
            let btn_y = row_rect.y + (row_rect.h - btn_h) / 2;
            let rst_w = 66i32;
            let reset_btn = Rect {
                x: row_rect.x + row_rect.w - rst_w - 10,
                y: btn_y,
                w: rst_w,
                h: btn_h,
            };
            let chg_label = if is_rec { "Cancel" } else { "Change" };
            let chg_w = 82i32;
            let change_btn = Rect {
                x: reset_btn.x - chg_w - 8,
                y: btn_y,
                w: chg_w,
                h: btn_h,
            };

            // Shortcut keycap badge to the left of Change button
            let badge_text = if is_rec {
                "Press new keys... (Esc cancel)"
            } else {
                key_str.as_str()
            };
            let badge_w = (font.measure_text(badge_text, 15.5) + 24).max(88);
            let badge_x = change_btn.x - badge_w - 12;
            canvas.fill_rounded_rect(
                badge_x,
                btn_y,
                badge_w,
                btn_h,
                6,
                if is_rec { theme.warning } else { theme.bg },
                if is_rec { 38 } else { 255 },
            );
            canvas.stroke_rounded_rect(
                badge_x,
                btn_y,
                badge_w,
                btn_h,
                6,
                if is_rec { theme.warning } else { theme.accent },
                if is_rec { 240 } else { 180 },
            );
            let bw = font.measure_text(badge_text, 15.5);
            canvas.draw_text_clipped(
                font,
                badge_text,
                FontStyle::Bold,
                15.5,
                badge_x + (badge_w - bw) / 2,
                btn_y + 20,
                badge_x + badge_w - 4,
                if is_rec { theme.warning } else { theme.accent },
            );

            // Change button
            canvas.fill_rounded_rect(
                change_btn.x,
                change_btn.y,
                change_btn.w,
                change_btn.h,
                6,
                if is_rec { theme.warning } else { theme.accent_soft },
                if is_rec { 40 } else { 255 },
            );
            canvas.stroke_rounded_rect(
                change_btn.x,
                change_btn.y,
                change_btn.w,
                change_btn.h,
                6,
                if is_rec { theme.warning } else { theme.accent },
                210,
            );
            let cw = font.measure_text(chg_label, 15.0);
            canvas.draw_text_clipped(
                font,
                chg_label,
                FontStyle::Bold,
                15.0,
                change_btn.x + (change_btn.w - cw) / 2,
                btn_y + 20,
                change_btn.x + change_btn.w - 2,
                if is_rec { theme.warning } else { theme.accent },
            );

            // Reset button
            canvas.fill_rounded_rect(reset_btn.x, reset_btn.y, reset_btn.w, reset_btn.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(reset_btn.x, reset_btn.y, reset_btn.w, reset_btn.h, 6, theme.border, 200);
            let rw = font.measure_text("Reset", 15.0);
            canvas.draw_text_clipped(
                font,
                "Reset",
                FontStyle::Regular,
                15.0,
                reset_btn.x + (reset_btn.w - rw) / 2,
                btn_y + 20,
                reset_btn.x + reset_btn.w - 2,
                theme.fg_muted,
            );

            // Left label
            canvas.draw_text_clipped(
                font,
                label,
                FontStyle::Bold,
                17.0,
                row_rect.x + 14,
                row_rect.y + 26,
                badge_x - 12,
                if is_sel || is_rec { theme.fg } else { theme.fg.dim(theme.surface) },
            );

            if row_rect.y >= body_top && row_rect.y + row_rect.h <= content_bottom + 20 {
                hit_zones
                    .settings_shortcut_btns
                    .push((*target, row_rect, change_btn, reset_btn));
            }
        }

        if let Some(err) = sv.error {
            canvas.draw_text_clipped(
                font,
                err,
                FontStyle::Bold,
                15.0,
                content_x + 14,
                cur_y + sc_card_h - 10,
                content_x + content_w - 14,
                theme.danger,
            );
        }
    }
}
