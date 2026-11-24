// SPDX-License-Identifier: MIT
//! The launcher's look for apps: a pale background, soft rounded shapes, Barlow text and the bar at the bottom, drawn
//! into a full 240 x 240 picture in RAM (115 KB). Draw, mark what changed, and [`Canvas::send`] sends it, at most a
//! budget at a time. It only draws into memory, so it also builds on a computer, with the screen the tests'.
//!
//! The shapes, the background and the text are drawn as apps/picker's scene draws them.
use crate::font::{FONT_B, FONT_XS, Font};
use crate::{Global, display};

pub const N: usize = 240;

pub const fn rgb(r: u32, g: u32, b: u32) -> u16 {
    ((r & 0xf8) << 8 | (g & 0xfc) << 3 | b >> 3) as u16
}

// The launcher's colours.
pub const INK: u16 = rgb(40, 52, 66);
pub const DIM: u16 = rgb(108, 120, 134);
pub const FAINT: u16 = rgb(150, 160, 172);
pub const LINE: u16 = rgb(196, 206, 218);
pub const BLUE: u16 = rgb(56, 136, 228);
pub const WORDMARK: u16 = rgb(70, 140, 215);
pub const ORANGE: u16 = rgb(236, 132, 40);
pub const RED: u16 = rgb(214, 72, 64);
pub const GREEN: u16 = rgb(52, 176, 104);
pub const WHITE: u16 = 0xffff;
pub const SHADOW: u16 = rgb(150, 166, 186);
pub const BAR_Y: i32 = 200;

/// The two faces.
#[derive(Clone, Copy, PartialEq)]
pub enum Face {
    Small,
    Big,
}

fn face(f: Face) -> &'static Font {
    if f == Face::Big { &FONT_B } else { &FONT_XS }
}

/// What a send does around and between the pieces it sends, as the card shares the screen's bus, and sound has to
/// keep time.
pub trait Hooks {
    fn before(&mut self) {}
    fn after(&mut self) {}
    fn between(&mut self) {}
}

/// No hooks.
pub struct Plain;
impl Hooks for Plain {}

#[derive(Clone, Copy, PartialEq)]
struct Dirty {
    x: u8,
    y: u8,
    w: u16,
    h: u16,
}

const MOST_DIRTY: usize = 24;
const OUT: usize = N * 2 * 16;

pub struct Canvas {
    pub fb: [[u16; N]; N],
    dirty: [Dirty; MOST_DIRTY],
    dirty_count: usize,
    budget: u32,
    out: [u8; OUT],
}

static CANVAS: Global<Canvas> = Global::new(Canvas {
    fb: [[0; N]; N],
    dirty: [Dirty { x: 0, y: 0, w: 0, h: 0 }; MOST_DIRTY],
    dirty_count: 0,
    budget: 0,
    out: [0; OUT],
});

/// The canvas, which an app takes once.
///
/// # Safety
/// Called once: there's one canvas, and this is the only reference to it.
pub unsafe fn claim() -> &'static mut Canvas {
    unsafe { CANVAS.get() }
}

static BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Near white at the top, a cool pale blue-grey at the bottom, dithered: the background's colour at a pixel.
pub fn back(x: i32, y: i32) -> u16 {
    let d = u32::from(BAYER[((y & 3) * 4 + (x & 3)) as usize]);
    let y = y as u32;
    let r = (246 - y * 22 / N as u32 + d / 2).min(255);
    let g = (248 - y * 16 / N as u32 + d / 4).min(255);
    let b = (252 - y * 6 / N as u32 + d / 2).min(255);
    (r >> 3 << 11 | g >> 2 << 5 | b >> 3) as u16
}

fn mix(under: u16, over: u16, a: u32, of: u32) -> u16 {
    let (under, over) = (u32::from(under), u32::from(over));
    let part = |shift: u32, mask: u32| {
        let (u, o) = (under >> shift & mask, over >> shift & mask);
        (u * (of - a) + o * a + of / 2) / of
    };
    (part(11, 31) << 11 | part(5, 63) << 5 | part(0, 31)) as u16
}

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

/// The distance from a pixel's centre to a rounded rectangle's edge, in 1/16 pixel, negative inside.
fn distance(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, r: i32) -> i32 {
    let qx = (px * 16 + 8 - (x * 16 + w * 8)).abs() - (w * 8 - r * 16);
    let qy = (py * 16 + 8 - (y * 16 + h * 8)).abs() - (h * 8 - r * 16);
    if qx <= 0 || qy <= 0 {
        return qx.max(qy) - r * 16;
    }
    isqrt((qx * qx + qy * qy) as u32) as i32 - r * 16
}

fn cover(d: i32, soft: i32) -> u32 {
    let f = soft * 16;
    ((f / 2 - d) * 16 / f).clamp(0, 16) as u32
}

/// A glyph's alpha at (gx, gy), 0 outside it.
fn lit(rows: &[u8], w: i32, h: i32, gx: i32, gy: i32) -> u32 {
    if gx < 0 || gy < 0 || gx >= w || gy >= h {
        return 0;
    }
    let a = u32::from(rows[(gy * ((w + 1) / 2) + gx / 2) as usize]);
    if gx & 1 != 0 { a & 15 } else { a >> 4 }
}

/// Text's width at scale/8 its size.
pub fn text_w(font: Face, s: &[u8], scale: i32) -> i32 {
    let f = face(font);
    let w: i32 = s.iter().map(|&c| i32::from(f.adv[f.glyph(c)])).sum();
    w * scale / 8
}

impl Canvas {
    pub fn budget(&mut self, bytes: u32) {
        self.budget = bytes;
    }

    /// Forgets what's marked, as when all of it is drawn again.
    pub fn forget(&mut self) {
        self.dirty_count = 0;
    }

    pub fn mark(&mut self, mut x: i32, mut y: i32, mut w: i32, mut h: i32) {
        if x < 0 {
            w += x;
            x = 0;
        }
        if y < 0 {
            h += y;
            y = 0;
        }
        w = w.min(N as i32 - x);
        h = h.min(N as i32 - y);
        if w <= 0 || h <= 0 {
            return;
        }
        let d = Dirty { x: x as u8, y: y as u8, w: w as u16, h: h as u16 };
        if self.dirty[..self.dirty_count].contains(&d) {
            return;
        }
        if self.dirty_count == MOST_DIRTY {
            self.dirty[0] = Dirty { x: 0, y: 0, w: N as u16, h: N as u16 };
            self.dirty_count = 1;
            return;
        }
        self.dirty[self.dirty_count] = d;
        self.dirty_count += 1;
    }

    /// Sends what's marked, oldest first, up to the budget; the rest goes on the next call.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn send(&mut self, hooks: &mut impl Hooks) {
        if self.dirty_count == 0 {
            return;
        }
        let (mut spent, mut done) = (0u32, 0);
        hooks.before();
        while done < self.dirty_count && (self.budget == 0 || spent < self.budget) {
            let Dirty { x, y, w, h } = self.dirty[done];
            let (x, y, w, h) = (usize::from(x), usize::from(y), usize::from(w), usize::from(h));
            let rows = h.min(OUT / (2 * w));
            let mut p = 0;
            for r in 0..rows {
                for &v in &self.fb[y + r][x..x + w] {
                    self.out[p..p + 2].copy_from_slice(&v.to_be_bytes());
                    p += 2;
                }
            }
            display::pixels(x as u32, y as u32, w as u32, rows as u32, &self.out[..p]);
            hooks.between();
            spent += (2 * w * rows) as u32;
            if rows == h {
                done += 1;
            } else {
                self.dirty[done].y = (y + rows) as u8;
                self.dirty[done].h = (h - rows) as u16;
            }
        }
        self.dirty.copy_within(done..self.dirty_count, 0);
        self.dirty_count -= done;
        hooks.after();
    }

    /// Sends all that's marked, whatever the budget.
    pub fn send_all(&mut self, hooks: &mut impl Hooks) {
        let kept = self.budget;
        self.budget = 0;
        self.send(hooks);
        self.budget = kept;
    }

    /// c at alpha a (out of 16) over what's there.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: u16, a: u32) {
        if x as u32 >= N as u32 || y as u32 >= N as u32 || a == 0 {
            return;
        }
        let p = &mut self.fb[y as usize][x as usize];
        *p = if a >= 16 { c } else { mix(*p, c, a, 16) };
    }

    /// The background over a rectangle.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn clear(&mut self, x: i32, y: i32, w: i32, h: i32) {
        for py in y.max(0)..(y + h).min(N as i32) {
            for px in x.max(0)..(x + w).min(N as i32) {
                self.fb[py as usize][px as usize] = back(px, py);
            }
        }
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: u16) {
        for py in y.max(0)..(y + h).min(N as i32) {
            let row = &mut self.fb[py as usize];
            let (from, to) = (x.max(0), (x + w).min(N as i32));
            if from < to {
                row[from as usize..to as usize].fill(c);
            }
        }
    }

    /// A rounded rectangle in c at alpha a (out of 16), its edge soft pixels wide.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    #[allow(clippy::too_many_arguments)]
    pub fn shape(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: u16, a: u32, soft: i32) {
        let m = soft / 2 + 1;
        for py in y - m..y + h + m {
            for px in x - m..x + w + m {
                // Well inside, with no edge to soften: just the colour.
                if a == 16 && px >= x + r + soft && px < x + w - r - soft && py >= y + soft && py < y + h - soft {
                    if (px as u32) < N as u32 && (py as u32) < N as u32 {
                        self.fb[py as usize][px as usize] = c;
                    }
                    continue;
                }
                self.blend(px, py, c, a * cover(distance(px, py, x, y, w, h, r), soft) / 16);
            }
        }
    }

    /// A white card with the launcher's soft shadow, and a blue ring round it if marked.
    pub fn card(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, marked: bool) {
        self.shape(x, y + 2, w, h, r, SHADOW, 5, 6);
        self.shape(x, y, w, h, r, WHITE, 16, 1);
        if marked {
            // A ring: the blue, then white inside it again.
            self.shape(x, y, w, h, r, BLUE, 16, 1);
            self.shape(x + 2, y + 2, w - 4, h - 4, (r - 2).max(0), WHITE, 16, 1);
        }
    }

    /// RGB565 with 4-bit alpha, two pixels to a byte, the first in the high half.
    pub fn sprite(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: &[u16], alpha: &[u8]) {
        for i in 0..(w * h) as usize {
            let a = u32::from(if i & 1 != 0 { alpha[i / 2] & 15 } else { alpha[i / 2] >> 4 });
            self.blend(x + i as i32 % w, y + i as i32 / w, rgb[i], (a * 17 + 8) >> 4);
        }
    }

    /// Text with the pen at x and the face's top at y, scale/8 its size. Returns the pen after it.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn text(&mut self, x: i32, y: i32, font: Face, s: &[u8], c: u16, scale: i32) -> i32 {
        let f = face(font);
        let mut pen = x * 8;
        for &ch in s {
            let gi = f.glyph(ch);
            let gd = &f.data[usize::from(f.off[gi])..];
            let (w, h) = (i32::from(f.bw[gi]), i32::from(gd[2]));
            let bx = (pen + (i32::from(gd[0]) - i32::from(f.pad)) * scale) / 8;
            let by = y + i32::from(gd[1]) * scale / 8;
            let (ow, oh) = ((w * scale + 7) / 8 + 1, (h * scale + 7) / 8 + 1);
            let rows = &gd[3..];
            for oy in 0..oh {
                for ox in 0..ow {
                    let a = if scale == 8 {
                        lit(rows, w, h, ox, oy)
                    } else {
                        // Where the pixel's centre falls in the glyph, in 1/16, and the four around it.
                        let (sx, sy) = ((2 * ox + 1) * 64 / scale - 8, (2 * oy + 1) * 64 / scale - 8);
                        let (mut gx, mut gy, mut fx, mut fy) = (sx >> 4, sy >> 4, sx & 15, sy & 15);
                        if sx < 0 {
                            gx = -1;
                            fx = sx + 16;
                        }
                        if sy < 0 {
                            gy = -1;
                            fy = sy + 16;
                        }
                        let (fx, fy) = (fx as u32, fy as u32);
                        let top = lit(rows, w, h, gx, gy) * (16 - fx) + lit(rows, w, h, gx + 1, gy) * fx;
                        let bottom = lit(rows, w, h, gx, gy + 1) * (16 - fx) + lit(rows, w, h, gx + 1, gy + 1) * fx;
                        (top * (16 - fy) + bottom * fy) >> 8
                    };
                    self.blend(bx + ox, by + oy, c, (a * 17 + 8) >> 4);
                }
            }
            pen += i32::from(f.adv[gi]) * scale;
        }
        pen / 8
    }

    /// The bar at the bottom, as the launcher's: the wordmark, then two lines on the right.
    pub fn bar(&mut self, top: &[u8], top_colour: u16, bottom: &[u8]) {
        let n = N as i32;
        self.clear(0, BAR_Y, n, n - BAR_Y);
        for y in BAR_Y..n {
            for x in 0..n {
                self.blend(x, y, WHITE, 9);
            }
        }
        self.fill(0, BAR_Y, n, 1, LINE);
        let mark_end = self.text(12, BAR_Y + 7, Face::Big, b"m-apps", WORDMARK, 12);
        let mut room = [0u8; 64];
        for (line, y, colour) in [(top, BAR_Y + 5, top_colour), (bottom, BAR_Y + 21, FAINT)] {
            if !line.is_empty() {
                let t = fit(Face::Small, line, n - 12 - mark_end - 10, &mut room);
                self.text(n - 12 - text_w(Face::Small, t, 8), y, Face::Small, t, colour, 8);
            }
        }
        self.mark(0, BAR_Y, n, n - BAR_Y);
    }
}

/// s cut to fit w at scale 8, ending in "...", in `room` if it had to be cut.
pub fn fit<'a>(font: Face, s: &'a [u8], w: i32, room: &'a mut [u8; 64]) -> &'a [u8] {
    if text_w(font, s, 8) <= w {
        return s;
    }
    let mut n = s.len().min(room.len() - 4);
    room[..n].copy_from_slice(&s[..n]);
    let dots = text_w(font, b"...", 8);
    while n > 0 && text_w(font, &room[..n], 8) + dots > w {
        n -= 1;
    }
    while n > 0 && room[n - 1] == b' ' {
        n -= 1;
    }
    room[n..n + 3].copy_from_slice(b"...");
    &room[..n + 3]
}
