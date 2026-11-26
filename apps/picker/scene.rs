// SPDX-License-Identifier: MIT
//! The launcher's screen: a light background, bubbles rising behind everything, and a layer of tiles and text over
//! them. Only the bubbles move, so a frame redraws just the rectangles they pass through. It draws into memory only, so
//! it also builds on a computer (tests/scene_host.rs).
use kit::font::{FONT_B, FONT_XS, Font};

pub const SIZE: i32 = 240;
const N: i32 = SIZE;
const PIXELS: usize = (N * N) as usize;
/// Three tiles show at once, above the bar.
pub const TILES: u32 = 3;
pub const ICON: usize = 44;
/// An icon's pixels: ICON x ICON, RGBA.
pub const ICON_BYTES: usize = ICON * ICON * 4;
pub const BAR_Y: i32 = 200;
pub const RED: u16 = 0xc8a4;

/// r, g, b out of 255 to RGB565
const fn rgb(r: u32, g: u32, b: u32) -> u16 {
    ((r & 0xf8) << 8 | (g & 0xfc) << 3 | b >> 3) as u16
}
const INK: u16 = rgb(40, 52, 66);
const DIM: u16 = rgb(108, 120, 134);
const FAINT: u16 = rgb(150, 160, 172);
const LINE: u16 = rgb(196, 206, 218);
const BLUE: u16 = rgb(56, 136, 228);
const WORDMARK: u16 = rgb(70, 140, 215);
const BUBBLES: usize = 12;
const TILE_X: i32 = 8;
const TILE_W: i32 = 224;
const TILE_H: i32 = 56;
const TILE_Y0: i32 = 10;
const TILE_STEP: i32 = 62;
const TILE_R: i32 = 12;

/// The layer: RGB565 colour and alpha out of 16 for each pixel, 3 bytes of RAM a pixel in all. Plain numbers only, so
/// it can be in RAM that holds whatever was there.
#[repr(C)]
pub struct Layer {
    colour: [u16; PIXELS],
    alpha: [u8; PIXELS],
}

impl Layer {
    /// For the tests on this computer: the launcher's is in the apps' slot.
    #[allow(dead_code)]
    pub const fn new() -> Layer {
        Layer { colour: [0; PIXELS], alpha: [0; PIXELS] }
    }
}

/// What a tile shows. icon: ICON x ICON RGBA pixels, or None for a letter on a colour.
pub struct Tile<'a> {
    pub name: &'a [u8],
    pub about: &'a [u8],
    pub version: &'a [u8],
    pub icon: Option<&'a [u8; ICON_BYTES]>,
    pub colour: u16,
    pub letter: u8,
}

#[derive(Clone, Copy)]
struct Bubble {
    x: i32,
    y: i32,
    r: i32,
    speed: i32,
    phase: i32,
    ox: i32,
    oy: i32,
}

pub struct Scene<'a> {
    layer: &'a mut Layer,
    bubbles: [Bubble; BUBBLES],
    seed: u32,
}

// ---- colour ----

fn mix(under: u16, over: u16, a: u32, of: u32) -> u16 {
    let (r, g, b) = (u32::from(under >> 11), u32::from(under >> 5) & 63, u32::from(under) & 31);
    let (rr, gg, bb) = (u32::from(over >> 11), u32::from(over >> 5) & 63, u32::from(over) & 31);
    let r = (r * (of - a) + rr * a + of / 2) / of;
    let g = (g * (of - a) + gg * a + of / 2) / of;
    let b = (b * (of - a) + bb * a + of / 2) / of;
    (r << 11 | g << 5 | b) as u16
}

// ---- shapes, by signed distance in 1/16 pixel ----

fn isqrt(mut v: u32) -> u32 {
    let (mut r, mut bit) = (0u32, 1u32 << 30);
    while bit > v {
        bit >>= 2;
    }
    while bit != 0 {
        if v >= r + bit {
            v -= r + bit;
            r = (r >> 1) + bit;
        } else {
            r >>= 1;
        }
        bit >>= 2;
    }
    r
}

/// the distance from pixel px, py's centre to a rounded rectangle's edge, negative inside
#[allow(clippy::too_many_arguments)]
fn distance(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, r: i32) -> i32 {
    let (qx, qy) = (px * 16 + 8 - (x * 16 + w * 8), py * 16 + 8 - (y * 16 + h * 8));
    let qx = qx.abs() - (w * 8 - r * 16);
    let qy = qy.abs() - (h * 8 - r * 16);
    // beside a straight edge, or inside
    if qx <= 0 || qy <= 0 {
        return qx.max(qy) - r * 16;
    }
    isqrt((qx * qx + qy * qy) as u32) as i32 - r * 16
}

/// how much of a pixel at distance d is covered, out of 16, with an edge soft pixels wide
fn cover(d: i32, soft: i32) -> u32 {
    let f = soft * 16;
    let a = (f / 2 - d) * 16 / f;
    a.clamp(0, 16) as u32
}

// ---- text ----

/// the width of s at scale/8
fn text_w(f: &Font, s: &[u8], scale: i32) -> i32 {
    let w: i32 = s.iter().map(|&c| i32::from(f.adv[f.glyph(c)])).sum();
    w * scale / 8
}

/// a glyph's alpha, 0..15, at gx, gy of its lit box, and 0 outside it
fn lit(rows: &[u8], w: i32, h: i32, gx: i32, gy: i32) -> u32 {
    if gx < 0 || gy < 0 || gx >= w || gy >= h {
        return 0;
    }
    let a = u32::from(rows[(gy * ((w + 1) / 2) + gx / 2) as usize]);
    if gx & 1 != 0 { a & 15 } else { a >> 4 }
}

/// Up to 60 bytes of a string, as fit() makes it.
pub struct Fitted {
    bytes: [u8; 64],
    n: usize,
}

impl Fitted {
    fn get(&self) -> &[u8] {
        &self.bytes[..self.n]
    }
}

/// s cut to fit w, ending in "..."
fn fit(f: &Font, s: &[u8], w: i32) -> Fitted {
    let mut out = Fitted { bytes: [0; 64], n: s.len().min(60) };
    out.bytes[..out.n].copy_from_slice(&s[..out.n]);
    if text_w(f, out.get(), 8) <= w {
        return out;
    }
    while out.n > 0 && text_w(f, out.get(), 8) + text_w(f, b"...", 8) > w {
        out.n -= 1;
    }
    while out.n > 0 && out.bytes[out.n - 1] == b' ' {
        out.n -= 1;
    }
    out.bytes[out.n..out.n + 3].copy_from_slice(b"...");
    out.n += 3;
    out
}

// ---- the background ----

const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// near white at the top, a cool pale blue-grey at the bottom, dithered
fn back(x: i32, y: i32) -> u16 {
    let d = u32::from(BAYER[((y & 3) * 4 + (x & 3)) as usize]);
    let y = y as u32;
    let r = 246 - y * 22 / N as u32 + d / 2;
    let g = 248 - y * 16 / N as u32 + d / 4;
    let b = 252 - y * 6 / N as u32 + d / 2;
    (r.min(255) >> 3 << 11 | g.min(255) >> 2 << 5 | b.min(255) >> 3) as u16
}

// ---- bubbles ----

/// -8..8, a period of 64 steps
fn sway(p: i32) -> i32 {
    const Q: [i8; 16] = [0, 1, 2, 2, 3, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8];
    let p = (p & 63) as usize;
    i32::from(if p < 16 {
        Q[p]
    } else if p < 32 {
        Q[31 - p]
    } else if p < 48 {
        -Q[p - 32]
    } else {
        -Q[63 - p]
    })
}

/// a bubble over colour c at x, y: a faint body, a stronger rim, a soft edge, a highlight up and to the left
fn bubble_px(mut c: u16, b: &Bubble, x: i32, y: i32) -> u16 {
    let (dx, dy, r) = (2 * (x - (b.x >> 4)) + 1, 2 * (y - (b.y >> 4)) + 1, b.r);
    // in half pixels
    let (d2, r2) = (dx * dx + dy * dy, 4 * r * r);
    if d2 >= r2 {
        return c;
    }
    // 0 centre .. 63 edge
    let t = (d2 * 64 / r2) as u32;
    let a = if t < 36 {
        4
    } else if t < 58 {
        4 + (t - 36) * 12 / 22
    } else {
        16 - (t - 58) * 3
    };
    c = mix(c, rgb(110, 170, 236), a, 32);
    // the highlight: an ellipse 0.38r by 0.24r, its centre at -0.4r, -0.42r
    let u = (dx * 50 + 80 * r) * 100 / (76 * r);
    let v = (dy * 50 + 84 * r) * 100 / (48 * r);
    let e = u * u + v * v;
    if e < 10000 {
        let k = ((10000 - e) * 26 / 10000) as u32;
        c = mix(c, 0xffff, k.min(20), 32);
    }
    c
}

/// The rows a tile covers, its shadow and glow included, so as to draw them again.
pub fn tile_rows(slot: u32) -> (i32, i32) {
    let y = TILE_Y0 + slot as i32 * TILE_STEP;
    ((y - 8).max(0), (y + TILE_H + 10).min(BAR_Y))
}

// ---- the scene ----

impl<'a> Scene<'a> {
    /// seed: where the bubbles start.
    pub fn new(layer: &'a mut Layer, seed: u32) -> Scene<'a> {
        layer.alpha.fill(0);
        let none = Bubble { x: 0, y: 0, r: 0, speed: 0, phase: 0, ox: 0, oy: 0 };
        let mut s = Scene { layer, bubbles: [none; BUBBLES], seed };
        for i in 0..BUBBLES {
            s.spawn(i, true);
        }
        s
    }

    fn rnd(&mut self, n: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(1103515245).wrapping_add(12345);
        (self.seed >> 16) % n
    }

    /// x and y in 1/16 pixel
    fn spawn(&mut self, i: usize, anywhere: bool) {
        let r = 5 + self.rnd(20) as i32;
        let x = (self.rnd(N as u32) as i32) << 4;
        let y = (if anywhere { self.rnd(N as u32) as i32 } else { N + r + 1 }) << 4;
        // the small ones a little faster
        let speed = 5 + self.rnd(8) as i32 + (26 - r) / 3;
        let phase = self.rnd(64) as i32;
        self.bubbles[i] = Bubble { x, y, r, speed, phase, ox: x >> 4, oy: y >> 4 };
    }

    /// colour c at alpha a (0..16) over the layer at i
    fn over(&mut self, i: usize, c: u16, a: u32) {
        if a == 0 {
            return;
        }
        let a = a.min(16);
        let under = u32::from(self.layer.alpha[i]);
        if under == 0 || a == 16 {
            self.layer.colour[i] = c;
            self.layer.alpha[i] = a as u8;
            return;
        }
        // straight alpha: the colour weighted by what shows of each
        let out = a + under * (16 - a) / 16;
        self.layer.colour[i] = mix(self.layer.colour[i], c, a * 16 / out, 16);
        self.layer.alpha[i] = out as u8;
    }

    /// a rounded rectangle in colour c at alpha a, its edge soft pixels wide; ring > 0 draws only a band that wide
    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u16, a: u32, soft: i32, ring: i32) {
        let m = soft / 2 + 1;
        for py in (y - m).max(0)..(y + h + m).min(N) {
            for px in (x - m).max(0)..(x + w + m).min(N) {
                let d = distance(px, py, x, y, w, h, r);
                let mut k = cover(d, soft);
                if ring != 0 && k != 0 {
                    k = k * (16 - cover(d + ring * 16, soft)) / 16;
                }
                self.over((py * N + px) as usize, c, a * k / 16);
            }
        }
    }

    /// s with the pen at x and the face's top at y, scale/8 times its size, smoothed when scaled: the pen after it
    fn text(&mut self, x: i32, y: i32, f: &Font, s: &[u8], c: u16, scale: i32) -> i32 {
        let mut pen = x * 8;
        for &ch in s {
            let gi = f.glyph(ch);
            let gd = &f.data[usize::from(f.off[gi])..];
            let (w, h) = (i32::from(f.bw[gi]), i32::from(gd[2]));
            let rows = &gd[3..];
            let bx = (pen + (i32::from(gd[0]) - i32::from(f.pad)) * scale) / 8;
            let by = y + i32::from(gd[1]) * scale / 8;
            let (ow, oh) = ((w * scale + 7) / 8 + 1, (h * scale + 7) / 8 + 1);
            for oy in 0..oh {
                for ox in 0..ow {
                    let a = if scale == 8 {
                        lit(rows, w, h, ox, oy)
                    } else {
                        // where the pixel's centre falls in the glyph, in 1/16, and the four around it
                        let (sx, sy) = ((2 * ox + 1) * 64 / scale - 8, (2 * oy + 1) * 64 / scale - 8);
                        let (mut gx, mut gy, mut fx, mut fy) = (sx >> 4, sy >> 4, sx & 15, sy & 15);
                        if sx < 0 {
                            (gx, fx) = (-1, sx + 16);
                        }
                        if sy < 0 {
                            (gy, fy) = (-1, sy + 16);
                        }
                        let (fx, fy) = (fx as u32, fy as u32);
                        let top = lit(rows, w, h, gx, gy) * (16 - fx) + lit(rows, w, h, gx + 1, gy) * fx;
                        let bottom = lit(rows, w, h, gx, gy + 1) * (16 - fx) + lit(rows, w, h, gx + 1, gy + 1) * fx;
                        (top * (16 - fy) + bottom * fy) >> 8
                    };
                    let (px, py) = (bx + ox, by + oy);
                    if a != 0 && (0..N).contains(&px) && (0..N).contains(&py) {
                        self.over((py * N + px) as usize, c, (a * 17 + 8) >> 4);
                    }
                }
            }
            pen += i32::from(f.adv[gi]) * scale;
        }
        pen / 8
    }

    /// The tiles' part of the layer, cleared.
    pub fn clear_tiles(&mut self) {
        self.layer.alpha[..(BAR_Y * N) as usize].fill(0);
    }

    fn icon(&mut self, x: i32, y: i32, t: &Tile<'_>) {
        let s = ICON as i32;
        if let Some(icon) = t.icon {
            for j in 0..s {
                for i in 0..s {
                    let p = &icon[(4 * (j * s + i)) as usize..][..4];
                    let a = (u32::from(p[3]) * cover(distance(x + i, y + j, x, y, s, s, 11), 1) + 127) / 255;
                    let c = rgb(u32::from(p[0]), u32::from(p[1]), u32::from(p[2]));
                    self.over(((y + j) * N + x + i) as usize, c, a);
                }
            }
            return;
        }
        let (rr, gg, bb) = (u32::from(t.colour >> 11) << 3, (u32::from(t.colour >> 5) & 63) << 2, (u32::from(t.colour) & 31) << 3);
        for j in 0..s {
            // lighter at the top
            let k = (s - j) as u32 * 3;
            let c = rgb(rr + (255 - rr) * k / 400, gg + (255 - gg) * k / 400, bb + (255 - bb) * k / 400);
            for i in 0..s {
                self.over(((y + j) * N + x + i) as usize, c, cover(distance(x + i, y + j, x, y, s, s, 11), 1));
            }
        }
        // a gloss on the top half
        self.shape(x + 2, y + 2, s - 4, s / 2 - 3, 9, 0xffff, 4, 2, 0);
        let letter = [t.letter];
        self.text(x + s / 2 - text_w(&FONT_B, &letter, 18) / 2, y + 7, &FONT_B, &letter, 0xffff, 18);
    }

    /// Tile 0..2 from the top, marked or not.
    pub fn tile(&mut self, slot: u32, t: &Tile<'_>, selected: bool) {
        let (x, y, w, h) = (TILE_X, TILE_Y0 + slot as i32 * TILE_STEP, TILE_W, TILE_H);
        // shadow
        self.shape(x, y + 3, w, h, TILE_R, rgb(30, 50, 80), if selected { 5 } else { 3 }, 10, 0);
        if selected {
            // glow
            self.shape(x - 3, y - 3, w + 6, h + 6, TILE_R + 3, BLUE, 6, 8, 0);
        }
        self.shape(x, y, w, h, TILE_R, 0xffff, if selected { 15 } else { 12 }, 1, 0);
        self.shape(x, y, w, h, TILE_R, if selected { BLUE } else { LINE }, 16, 1, if selected { 2 } else { 1 });
        self.icon(x + 7, y + 6, t);
        let vw = if t.version.is_empty() { 0 } else { text_w(&FONT_XS, t.version, 8) };
        self.text(x + 60, y + 9, &FONT_B, fit(&FONT_B, t.name, w - 70 - vw).get(), INK, 8);
        if vw != 0 {
            self.text(x + w - 10 - vw, y + 10, &FONT_XS, t.version, FAINT, 8);
        }
        if !t.about.is_empty() {
            self.text(x + 60, y + 30, &FONT_XS, fit(&FONT_XS, t.about, w - 68).get(), DIM, 8);
        }
    }

    /// Small marks at the top and the bottom of the list, when there's more that way.
    pub fn more(&mut self, above: bool, below: bool) {
        if above {
            self.shape(N / 2 - 10, 2, 20, 4, 2, BLUE, 10, 1, 0);
        }
        if below {
            self.shape(N / 2 - 10, BAR_Y - 6, 20, 4, 2, BLUE, 10, 1, 0);
        }
    }

    /// A panel instead of the tiles, for a card that can't be read or has no apps.
    pub fn message(&mut self, title: &[u8], mut s: &[u8]) {
        let (x, y, w, h) = (20, 40, N - 40, 120);
        self.shape(x, y + 3, w, h, 14, rgb(30, 50, 80), 4, 10, 0);
        self.shape(x, y, w, h, 14, 0xffff, 15, 1, 0);
        self.shape(x, y, w, h, 14, LINE, 16, 1, 1);
        self.text(x + (w - text_w(&FONT_B, title, 8)) / 2, y + 16, &FONT_B, title, INK, 8);
        // the text by words, on up to four lines
        let mut line = [0u8; 64];
        let mut ly = y + 44;
        while !s.is_empty() && ly < y + h - 16 {
            let (mut n, mut fit_n) = (0, 0);
            while n < s.len() && n < line.len() - 1 {
                line[n] = s[n];
                if text_w(&FONT_XS, &line[..n + 1], 8) > w - 28 {
                    break;
                }
                n += 1;
                if n == s.len() || s[n] == b' ' {
                    fit_n = n;
                }
            }
            if fit_n == 0 {
                fit_n = if n != 0 { n } else { 1 };
            }
            let piece = &s[..fit_n];
            self.text(x + (w - text_w(&FONT_XS, piece, 8)) / 2, ly, &FONT_XS, piece, DIM, 8);
            s = &s[fit_n..];
            while let [b' ', rest @ ..] = s {
                s = rest;
            }
            ly += 17;
        }
    }

    /// The bar at the bottom: where the selection is (such as 3 / 7), and the keys, or a note in its colour instead.
    pub fn bar(&mut self, at: &[u8], keys: &[u8], note: &[u8], note_colour: u16) {
        self.layer.alpha[(BAR_Y * N) as usize..].fill(0);
        self.shape(0, BAR_Y, N, N - BAR_Y, 0, 0xffff, 9, 1, 0);
        self.shape(0, BAR_Y, N, 1, 0, LINE, 16, 1, 0);
        let mark = self.text(12, BAR_Y + 7, &FONT_B, b"m-apps", WORDMARK, 12);
        if !at.is_empty() {
            self.text(N - 12 - text_w(&FONT_XS, at, 8), BAR_Y + 5, &FONT_XS, at, DIM, 8);
        }
        let last = if note.is_empty() { keys } else { note };
        if !last.is_empty() {
            let s = fit(&FONT_XS, last, N - 12 - mark - 10);
            let colour = if note.is_empty() { FAINT } else { note_colour };
            self.text(N - 12 - text_w(&FONT_XS, s.get(), 8), BAR_Y + 21, &FONT_XS, s.get(), colour, 8);
        }
    }

    /// The pixels of x0..x1 by y0..y1 (exclusive), RGB565 big-endian, x1-x0 to a row.
    pub fn render(&self, x0: i32, y0: i32, x1: i32, y1: i32, out: &mut [[u8; 2]]) {
        if x1 <= x0 {
            return;
        }
        for (y, out) in (y0..y1).zip(out.chunks_exact_mut((x1 - x0) as usize)) {
            // the bubbles on this row
            let mut on = [&self.bubbles[0]; BUBBLES];
            let mut n = 0;
            for b in &self.bubbles {
                let (cy, cx, r) = (b.y >> 4, b.x >> 4, b.r);
                if y >= cy - r && y < cy + r && cx + r > x0 && cx - r < x1 {
                    on[n] = b;
                    n += 1;
                }
            }
            let on = &on[..n];
            let row = (y * N) as usize;
            let (a, c) = (row + x0 as usize, row + x1 as usize);
            let (a, c) = (&self.layer.alpha[a..c], &self.layer.colour[a..c]);
            for (((x, out), &ax), &cx) in (x0..x1).zip(out).zip(a).zip(c) {
                let p = if ax >= 16 {
                    cx
                } else {
                    let mut p = back(x, y);
                    for b in on {
                        p = bubble_px(p, b, x, y);
                    }
                    if ax != 0 { mix(p, cx, u32::from(ax), 16) } else { p }
                };
                *out = p.to_be_bytes();
            }
        }
    }

    /// The bubbles a step on. `dirty` is called with each rectangle that needs drawing again, and the scene as it is
    /// then.
    pub fn step(&mut self, mut dirty: impl FnMut(&Scene<'_>, i32, i32, i32, i32)) {
        for i in 0..BUBBLES {
            let b = &mut self.bubbles[i];
            b.ox = b.x >> 4;
            b.oy = b.y >> 4;
            b.y -= b.speed;
            b.phase = b.phase.wrapping_add(1);
            b.x += sway(b.phase);
            let (ox, oy, r) = (b.ox, b.oy, b.r);
            if (b.y >> 4) + r < 0 {
                dirty(self, ox - r - 1, oy - r - 1, ox + r + 1, oy + r + 1);
                self.spawn(i, false);
                continue;
            }
            let (nx, ny, r) = (b.x >> 4, b.y >> 4, r + 1);
            dirty(self, nx.min(ox) - r, ny.min(oy) - r, nx.max(ox) + r, ny.max(oy) + r);
        }
    }
}
