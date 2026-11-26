//! Procedural block-element and box-drawing glyphs that fill the whole cell.

/// Arm weights: 0 none, 1 light, 2 heavy, 3 double.
type Arms = [u8; 4]; // up, down, left, right

#[rustfmt::skip]
const ARMS: [Arms; 128] = [
    [0,0,1,1],[0,0,2,2],[1,1,0,0],[2,2,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0],
    [0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,1,0,1],[0,1,0,2],[0,2,0,1],[0,2,0,2],
    [0,1,1,0],[0,1,2,0],[0,2,1,0],[0,2,2,0],[1,0,0,1],[1,0,0,2],[2,0,0,1],[2,0,0,2],
    [1,0,1,0],[1,0,2,0],[2,0,1,0],[2,0,2,0],[1,1,0,1],[1,1,0,2],[2,1,0,1],[1,2,0,1],
    [2,2,0,1],[2,1,0,2],[1,2,0,2],[2,2,0,2],[1,1,1,0],[1,1,2,0],[2,1,1,0],[1,2,1,0],
    [2,2,1,0],[2,1,2,0],[1,2,2,0],[2,2,2,0],[0,1,1,1],[0,1,2,1],[0,1,1,2],[0,1,2,2],
    [0,2,1,1],[0,2,2,1],[0,2,1,2],[0,2,2,2],[1,0,1,1],[1,0,2,1],[1,0,1,2],[1,0,2,2],
    [2,0,1,1],[2,0,2,1],[2,0,1,2],[2,0,2,2],[1,1,1,1],[1,1,2,1],[1,1,1,2],[1,1,2,2],
    [2,1,1,1],[1,2,1,1],[2,2,1,1],[2,1,2,1],[2,1,1,2],[1,2,2,1],[1,2,1,2],[2,1,2,2],
    [1,2,2,2],[2,2,2,1],[2,2,1,2],[2,2,2,2],[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,0,0],
    [0,0,3,3],[3,3,0,0],[0,1,0,3],[0,3,0,1],[0,3,0,3],[0,1,3,0],[0,3,1,0],[0,3,3,0],
    [1,0,0,3],[3,0,0,1],[3,0,0,3],[1,0,3,0],[3,0,1,0],[3,0,3,0],[1,1,0,3],[3,3,0,1],
    [3,3,0,3],[1,1,3,0],[3,3,1,0],[3,3,3,0],[0,1,3,3],[0,3,1,1],[0,3,3,3],[1,0,3,3],
    [3,0,1,1],[3,0,3,3],[1,1,3,3],[3,3,1,1],[3,3,3,3],[0,1,0,1],[0,1,1,0],[1,0,1,0],
    [1,0,0,1],[0,0,0,0],[0,0,0,0],[0,0,0,0],[0,0,1,0],[1,0,0,0],[0,0,0,1],[0,1,0,0],
    [0,0,2,0],[2,0,0,0],[0,0,0,2],[0,2,0,0],[0,0,1,2],[1,2,0,0],[0,0,2,1],[2,1,0,0],
];

/// Returns a `w * h` coverage bitmap (row-major, 0..=255) that fills the whole
/// cell, or `None` when `ch` is not drawn procedurally.
pub fn builtin_glyph(ch: char, w: usize, h: usize) -> Option<Vec<u8>> {
    if w == 0 || h == 0 {
        return None;
    }
    let c = ch as u32;
    match c {
        0x2580..=0x259F => Some(block_glyph(c, w, h)),
        0x2500..=0x2570 | 0x2574..=0x257F => Some(box_glyph(c, w, h)),
        _ => None,
    }
}

/// Splits `n` pixels at `k/8`, rounded. Complementary splits use `8 - k` so
/// that pieces tile exactly.
fn split(n: usize, k: usize) -> usize {
    (n * k + 4) / 8
}

#[allow(clippy::too_many_arguments)]
fn rect(buf: &mut [u8], w: usize, h: usize, x0: i32, y0: i32, x1: i32, y1: i32, v: u8) {
    let (x0, x1) = (
        x0.clamp(0, w as i32) as usize,
        x1.clamp(0, w as i32) as usize,
    );
    let (y0, y1) = (
        y0.clamp(0, h as i32) as usize,
        y1.clamp(0, h as i32) as usize,
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &mut buf[y * w + x];
            *p = (*p).max(v);
        }
    }
}

fn block_glyph(c: u32, w: usize, h: usize) -> Vec<u8> {
    let mut b = vec![0u8; w * h];
    let (wi, hi) = (w as i32, h as i32);
    let eighth_h = |k: usize| split(h, k).max(1) as i32;
    let eighth_w = |k: usize| split(w, k).max(1) as i32;
    match c {
        // lower k/8 (▁..▇), full block
        0x2581..=0x2588 => {
            let k = (c - 0x2580) as usize;
            let top = if k == 8 {
                0
            } else {
                // Same split point as the complementary upper piece.
                split(h, 8 - k).min(h - 1) as i32
            };
            rect(&mut b, w, h, 0, top, wi, hi, 255);
        }
        // upper half, upper eighth
        0x2580 => rect(&mut b, w, h, 0, 0, wi, split(h, 4) as i32, 255),
        0x2594 => rect(&mut b, w, h, 0, 0, wi, eighth_h(1), 255),
        // left k/8 (▉..▏)
        0x2589..=0x258F => {
            let k = (0x2590 - c) as usize;
            let x1 = if k == 4 {
                split(w, 4) as i32
            } else {
                eighth_w(k).min(wi)
            };
            rect(&mut b, w, h, 0, 0, x1, hi, 255);
        }
        0x2590 => rect(&mut b, w, h, split(w, 4) as i32, 0, wi, hi, 255),
        0x2595 => rect(&mut b, w, h, wi - eighth_w(1).min(wi), 0, wi, hi, 255),
        0x2591..=0x2593 => {
            let v = match c {
                0x2591 => 64,
                0x2592 => 128,
                _ => 192,
            };
            b.fill(v);
        }
        _ => {
            // Quadrants: UL=1 UR=2 LL=4 LR=8
            let mask = match c {
                0x2596 => 4,
                0x2597 => 8,
                0x2598 => 1,
                0x2599 => 1 | 4 | 8,
                0x259A => 1 | 8,
                0x259B => 1 | 2 | 4,
                0x259C => 1 | 2 | 8,
                0x259D => 2,
                0x259E => 2 | 4,
                _ => 2 | 4 | 8, // 0x259F
            };
            let (sx, sy) = (split(w, 4) as i32, split(h, 4) as i32);
            if mask & 1 != 0 {
                rect(&mut b, w, h, 0, 0, sx, sy, 255);
            }
            if mask & 2 != 0 {
                rect(&mut b, w, h, sx, 0, wi, sy, 255);
            }
            if mask & 4 != 0 {
                rect(&mut b, w, h, 0, sy, sx, hi, 255);
            }
            if mask & 8 != 0 {
                rect(&mut b, w, h, sx, sy, wi, hi, 255);
            }
        }
    }
    b
}

/// Line thicknesses for a cell width: light, heavy.
fn thickness(w: usize) -> (i32, i32) {
    let t = ((w as f32 / 8.0).round() as i32).max(1);
    (t, (2 * t).max(t + 1))
}

/// Total thickness of a weighted line.
fn thick(wt: u8, t: i32, hv: i32) -> i32 {
    match wt {
        1 => t,
        2 => hv,
        _ => 3 * t,
    }
}

/// Cross-axis extents of the line(s) of weight `wt` centred in `dim`.
fn lines(wt: u8, dim: usize, t: i32, hv: i32) -> ([(i32, i32); 2], usize) {
    let d = dim as i32;
    let total = thick(wt, t, hv);
    let s = (d - total) / 2;
    if wt == 3 {
        ([(s, s + t), (s + 2 * t, s + 3 * t)], 2)
    } else {
        ([(s, s + total), (0, 0)], 1)
    }
}

fn dash_info(c: u32) -> Option<(bool, i32, bool)> {
    // (horizontal, segments, heavy)
    Some(match c {
        0x2504 => (true, 3, false),
        0x2505 => (true, 3, true),
        0x2506 => (false, 3, false),
        0x2507 => (false, 3, true),
        0x2508 => (true, 4, false),
        0x2509 => (true, 4, true),
        0x250A => (false, 4, false),
        0x250B => (false, 4, true),
        0x254C => (true, 2, false),
        0x254D => (true, 2, true),
        0x254E => (false, 2, false),
        0x254F => (false, 2, true),
        _ => return None,
    })
}

fn box_glyph(c: u32, w: usize, h: usize) -> Vec<u8> {
    let mut b = vec![0u8; w * h];
    let (t, hv) = thickness(w);
    let (wi, hi) = (w as i32, h as i32);

    if let Some((horiz, n, heavy)) = dash_info(c) {
        let th = if heavy { hv } else { t };
        let len = if horiz { wi } else { hi };
        let gap = ((len as f32 / n as f32) / 2.0).round().max(1.0);
        let half = gap / 2.0;
        let (s0, s1) = {
            let s = ((if horiz { hi } else { wi }) - th) / 2;
            (s, s + th)
        };
        for i in 0..n {
            let a = (len as f32 * i as f32 / n as f32 + half).round() as i32;
            let mut e = (len as f32 * (i + 1) as f32 / n as f32 - half).round() as i32;
            if e <= a {
                e = a + 1;
            }
            if horiz {
                rect(&mut b, w, h, a, s0, e, s1, 255);
            } else {
                rect(&mut b, w, h, s0, a, s1, e, 255);
            }
        }
        return b;
    }

    let [up, down, left, right] = ARMS[(c - 0x2500) as usize];

    if (0x256D..=0x2570).contains(&c) {
        rounded(&mut b, w, h, t, up > 0, left > 0);
        return b;
    }

    // Span of the vertical-arm lines across x, and of horizontal-arm lines
    // across y.
    let span = |a: &[u8], dim: usize| -> Option<(i32, i32)> {
        a.iter()
            .filter(|&&wt| wt > 0)
            .map(|&wt| {
                let (l, n) = lines(wt, dim, t, hv);
                (l[0].0, l[n - 1].1)
            })
            .reduce(|p, q| (p.0.min(q.0), p.1.max(q.1)))
    };
    let vspan = span(&[up, down], w);
    let hspan = span(&[left, right], h);

    // Vertical arms
    for (is_up, wt) in [(true, up), (false, down)] {
        if wt == 0 {
            continue;
        }
        let (ls, n) = lines(wt, w, t, hv);
        for (k, &(x0, x1)) in ls.iter().enumerate().take(n) {
            let (y0, y1) = match hspan {
                None => {
                    if is_up {
                        (0, (hi + thick(wt, t, hv)) / 2)
                    } else {
                        ((hi - thick(wt, t, hv)) / 2, hi)
                    }
                }
                Some((hs, he)) => {
                    let inner = wt == 3 && ((k == 0 && left > 0) || (k == 1 && right > 0));
                    if is_up {
                        (0, if inner { hs + t } else { he })
                    } else {
                        (if inner { he - t } else { hs }, hi)
                    }
                }
            };
            rect(&mut b, w, h, x0, y0, x1, y1, 255);
        }
    }

    // Horizontal arms
    for (is_left, wt) in [(true, left), (false, right)] {
        if wt == 0 {
            continue;
        }
        let (ls, n) = lines(wt, h, t, hv);
        for (k, &(y0, y1)) in ls.iter().enumerate().take(n) {
            let (x0, x1) = match vspan {
                None => {
                    if is_left {
                        (0, (wi + thick(wt, t, hv)) / 2)
                    } else {
                        ((wi - thick(wt, t, hv)) / 2, wi)
                    }
                }
                Some((vs, ve)) => {
                    let inner = wt == 3 && ((k == 0 && up > 0) || (k == 1 && down > 0));
                    if is_left {
                        (0, if inner { vs + t } else { ve })
                    } else {
                        (if inner { ve - t } else { vs }, wi)
                    }
                }
            };
            rect(&mut b, w, h, x0, y0, x1, y1, 255);
        }
    }
    b
}

/// Rounded corner joining a vertical arm (`up` = towards the top edge) and a
/// horizontal arm (`left` = towards the left edge) with an anti-aliased
/// quarter circle.
fn rounded(b: &mut [u8], w: usize, h: usize, t: i32, up: bool, left: bool) {
    let (wi, hi) = (w as i32, h as i32);
    let x0 = (wi - t) / 2;
    let y0 = (hi - t) / 2;
    let xc = x0 as f32 + t as f32 / 2.0;
    let yc = y0 as f32 + t as f32 / 2.0;
    let dx = if left { -1.0f32 } else { 1.0 };
    let dy = if up { -1.0f32 } else { 1.0 };
    let room_x = if left { xc } else { w as f32 - xc };
    let room_y = if up { yc } else { h as f32 - yc };
    let r = room_x.min(room_y);
    let (ccx, ccy) = (xc + dx * r, yc + dy * r);
    let half = t as f32 / 2.0;

    for py in 0..hi {
        for px in 0..wi {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let in_vcol = px >= x0 && px < x0 + t;
            let in_hrow = py >= y0 && py < y0 + t;
            let beyond_y = (fy - ccy) * dy >= 0.0;
            let beyond_x = (fx - ccx) * dx >= 0.0;
            let cov = if (in_vcol && beyond_y) || (in_hrow && beyond_x) {
                1.0
            } else if !beyond_x && !beyond_y {
                let d = ((fx - ccx).powi(2) + (fy - ccy).powi(2)).sqrt();
                (half + 0.5 - (d - r).abs()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if cov > 0.0 {
                let v = (cov * 255.0).round() as u8;
                let p = &mut b[py as usize * w + px as usize];
                *p = (*p).max(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: [(usize, usize); 3] = [(7, 19), (8, 18), (9, 21)];

    fn g(ch: char, w: usize, h: usize) -> Vec<u8> {
        builtin_glyph(ch, w, h).unwrap_or_else(|| panic!("no glyph for {ch:?}"))
    }

    #[test]
    fn full_block_is_solid() {
        assert!(g('█', 7, 19).iter().all(|&v| v == 255));
    }

    #[test]
    fn halves_partition_the_cell() {
        for (w, h) in SIZES {
            for (a, b) in [('▌', '▐'), ('▀', '▄')] {
                let (ga, gb) = (g(a, w, h), g(b, w, h));
                for i in 0..w * h {
                    assert_eq!(ga[i] as u32 + gb[i] as u32, 255, "{a}{b} {w}x{h} px {i}");
                }
            }
        }
    }

    #[test]
    fn quadrant_unions() {
        let (w, h) = (7, 19);
        let (ul, ur, ll, lr) = (g('▘', w, h), g('▝', w, h), g('▖', w, h), g('▗', w, h));
        let union = |parts: &[&Vec<u8>]| -> Vec<u8> {
            (0..w * h)
                .map(|i| parts.iter().map(|p| p[i]).max().unwrap())
                .collect()
        };
        assert_eq!(g('▛', w, h), union(&[&ul, &ur, &ll]));
        assert_eq!(g('▜', w, h), union(&[&ul, &ur, &lr]));
        assert_eq!(g('▙', w, h), union(&[&ul, &ll, &lr]));
        assert_eq!(g('▟', w, h), union(&[&ur, &ll, &lr]));
    }

    #[test]
    fn straight_lines_reach_edges() {
        for (w, h) in SIZES {
            let v = g('│', w, h);
            let cx = (w - 1) / 2;
            assert_eq!(v[cx], 255, "top edge {w}x{h}");
            assert_eq!(v[(h - 1) * w + cx], 255, "bottom edge {w}x{h}");
            let l = g('─', w, h);
            let cy = (h - 1) / 2;
            assert_eq!(l[cy * w], 255, "left edge {w}x{h}");
            assert_eq!(l[cy * w + w - 1], 255, "right edge {w}x{h}");
        }
    }

    #[test]
    fn rounded_corner_shape() {
        for (w, h) in SIZES {
            let c = g('╭', w, h);
            assert!(c[(h - 1) * w + (w - 1) / 2] > 0, "bottom centre {w}x{h}");
            assert!(c[((h - 1) / 2) * w + w - 1] > 0, "right centre {w}x{h}");
            assert_eq!(c[0], 0, "top-left {w}x{h}");
        }
    }

    #[test]
    fn cross_joins() {
        let (w, h) = (7, 19);
        let c = g('┼', w, h);
        for y in 0..h {
            assert_eq!(c[y * w + (w - 1) / 2], 255);
        }
        for x in 0..w {
            assert_eq!(c[((h - 1) / 2) * w + x], 255);
        }
    }

    #[test]
    fn coverage_of_ranges() {
        for cp in 0x2580u32..=0x259F {
            assert!(
                builtin_glyph(char::from_u32(cp).unwrap(), 7, 19).is_some(),
                "{cp:x}"
            );
        }
        for cp in 0x2500u32..=0x2570 {
            if (0x2571..=0x2573).contains(&cp) {
                continue;
            }
            assert!(
                builtin_glyph(char::from_u32(cp).unwrap(), 7, 19).is_some(),
                "{cp:x}"
            );
        }
        assert!(builtin_glyph('A', 7, 19).is_none());
    }

    #[test]
    fn bitmap_length_all_sizes() {
        for (w, h) in [(1, 1), (3, 5), (7, 19), (8, 18), (9, 21), (20, 40)] {
            for cp in 0x2500u32..=0x259F {
                if let Some(b) = builtin_glyph(char::from_u32(cp).unwrap(), w, h) {
                    assert_eq!(b.len(), w * h, "{cp:x} {w}x{h}");
                }
            }
        }
    }
}
