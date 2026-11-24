// SPDX-License-Identifier: MIT
//! The apps' 90s desktop look, shared: a 120 x 120 picture shown at twice that, brushed metal, ridged title bars,
//! black insets with green text, and two pixel fonts (pixel_font.py). Draw into [`Pixel::fb`], mark what changed, and
//! [`Pixel::send`] sends it, at most a budget at a time.
use crate::canvas::Hooks;
use crate::pixel_font::{BIG, SMALL};
use crate::{Global, display};

pub const W: usize = 120;
pub const H: usize = 120;

pub const fn rgb(r: u32, g: u32, b: u32) -> u16 {
    ((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16
}

// The palette.
pub const BODY: u16 = rgb(0x3c, 0x43, 0x58);
pub const BODY_ALT: u16 = rgb(0x37, 0x3e, 0x52);
pub const EDGE_LIGHT: u16 = rgb(0x9a, 0xa4, 0xbe);
pub const EDGE_DARK: u16 = rgb(0x12, 0x15, 0x1f);
pub const INSET_DARK: u16 = rgb(0x0e, 0x10, 0x18);
pub const INSET_LIGHT: u16 = rgb(0x7c, 0x86, 0xa0);
pub const TITLE_BG: u16 = rgb(0x19, 0x1e, 0x2d);
pub const RIDGE_LIGHT: u16 = rgb(0x5c, 0x68, 0x8a);
pub const RIDGE_DARK: u16 = rgb(0x0d, 0x10, 0x18);
pub const GOLD: u16 = rgb(0xf2, 0xc8, 0x5c);
pub const GOLD_SHADOW: u16 = rgb(0x5a, 0x40, 0x10);
pub const LCD: u16 = rgb(0x00, 0x00, 0x00);
pub const LCD_ON: u16 = rgb(0x10, 0xf0, 0x40);
pub const LCD_OFF: u16 = rgb(0x06, 0x1c, 0x0c);
pub const LABEL: u16 = rgb(0xcc, 0xd2, 0xe2);
pub const BUTTON: u16 = rgb(0x5a, 0x64, 0x80);
pub const BUTTON_LIGHT: u16 = rgb(0xb0, 0xba, 0xd4);
pub const BUTTON_DARK: u16 = rgb(0x18, 0x1c, 0x27);
pub const BUTTON_DOWN: u16 = rgb(0x48, 0x50, 0x68);
pub const ICON: u16 = rgb(0x10, 0x13, 0x1b);

#[derive(Clone, Copy, PartialEq)]
struct Dirty {
    x: u8,
    y: u8,
    w: u8,
    h: u8,
}

const MOST_DIRTY: usize = 24;
const OUT: usize = 240 * 2 * 2 * 8;

pub struct Pixel {
    pub fb: [[u16; W]; H],
    dirty: [Dirty; MOST_DIRTY],
    dirty_count: usize,
    budget: u32,
    out: [u8; OUT],
}

static PIXEL: Global<Pixel> = Global::new(Pixel {
    fb: [[0; W]; H],
    dirty: [Dirty { x: 0, y: 0, w: 0, h: 0 }; MOST_DIRTY],
    dirty_count: 0,
    budget: 0,
    out: [0; OUT],
});

/// The picture, which an app takes once.
///
/// # Safety
/// Called once: there's one picture, and this is the only reference to it.
pub unsafe fn claim() -> &'static mut Pixel {
    unsafe { PIXEL.get() }
}

/// On this computer, for the tests: what's sent also goes here, as `display::pixels()` gets it.
#[cfg(not(target_arch = "pi32v2"))]
static WATCH: Global<Option<fn(u32, u32, u32, u32, &[u8])>> = Global::new(None);

/// On this computer, for the tests: `f` sees each piece that's sent, as the screen would, or nothing for None.
#[cfg(not(target_arch = "pi32v2"))]
pub fn watch(f: Option<fn(u32, u32, u32, u32, &[u8])>) {
    WATCH.set(f);
}

/// 3 x 5 text's width (M, N and W wider).
pub fn small_width(s: &[u8]) -> u32 {
    let w: u32 = s
        .iter()
        .map(|&ch| {
            let gw = if (32..=126).contains(&ch) { SMALL[usize::from(ch - 32)][0] } else { 0 };
            u32::from(if gw != 0 { gw } else { 3 }) + 1
        })
        .sum();
    w.saturating_sub(1)
}

impl Pixel {
    /// Forgets what was marked, as after drawing everything again.
    pub fn forget(&mut self) {
        self.dirty_count = 0;
    }

    /// The most bytes one send() sends, or 0 for all. What's left goes next time, so a big redraw doesn't hold up
    /// anything that has to keep time.
    pub fn budget(&mut self, bytes: u32) {
        self.budget = bytes;
    }

    /// Marks part of the picture to be sent.
    pub fn mark(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let d = Dirty { x: x as u8, y: y as u8, w: w as u8, h: h as u8 };
        if self.dirty[..self.dirty_count].contains(&d) {
            return;
        }
        if self.dirty_count == MOST_DIRTY {
            self.dirty[0] = Dirty { x: 0, y: 0, w: W as u8, h: H as u8 };
            self.dirty_count = 1;
            return;
        }
        self.dirty[self.dirty_count] = d;
        self.dirty_count += 1;
    }

    /// Sends all that's marked, whatever the budget.
    pub fn send_all(&mut self, hooks: &mut impl Hooks) {
        let kept = self.budget;
        self.budget = 0;
        self.send(hooks);
        self.budget = kept;
    }

    /// Sends what's waiting, oldest first, up to the budget; the rest goes on the next call. Each pixel goes out as
    /// two by two.
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
            let rows = h.min(OUT / (8 * w));
            let mut p = 0;
            for r in 0..rows {
                let line = p;
                for &v in &self.fb[y + r][x..x + w] {
                    let [hi, lo] = v.to_be_bytes();
                    self.out[p..p + 4].copy_from_slice(&[hi, lo, hi, lo]);
                    p += 4;
                }
                self.out.copy_within(line..line + 4 * w, p);
                p += 4 * w;
            }
            display::pixels(2 * x as u32, 2 * y as u32, 2 * w as u32, 2 * rows as u32, &self.out[..p]);
            #[cfg(not(target_arch = "pi32v2"))]
            if let Some(f) = WATCH.read() {
                f(2 * x as u32, 2 * y as u32, 2 * w as u32, 2 * rows as u32, &self.out[..p]);
            }
            hooks.between();
            spent += (8 * w * rows) as u32;
            if rows == h {
                done += 1;
            } else {
                self.dirty[done].y = (y + rows) as u8;
                self.dirty[done].h = (h - rows) as u8;
            }
        }
        self.dirty.copy_within(done..self.dirty_count, 0);
        self.dirty_count -= done;
        hooks.after();
    }

    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: u16) {
        for r in y..(y + h).min(H as u32) {
            let row = &mut self.fb[r as usize];
            let to = (x + w).min(W as u32);
            if x < to {
                row[x as usize..to as usize].fill(c);
            }
        }
    }

    #[inline]
    pub fn pixel(&mut self, x: i32, y: i32, c: u16) {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            self.fb[y as usize][x as usize] = c;
        }
    }

    /// A raised or sunken edge, one pixel wide.
    pub fn edge(&mut self, x: u32, y: u32, w: u32, h: u32, top_left: u16, bottom_right: u16) {
        self.fill(x, y, w, 1, top_left);
        self.fill(x, y, 1, h, top_left);
        self.fill(x, y + h - 1, w, 1, bottom_right);
        self.fill(x + w - 1, y, 1, h, bottom_right);
    }

    /// Brushed metal.
    pub fn body(&mut self, x: u32, y: u32, w: u32, h: u32) {
        for r in y..y + h {
            self.fill(x, r, w, 1, if r & 1 != 0 { BODY_ALT } else { BODY });
        }
    }

    /// 5 x 7 text, 6 apart, clipped to [clip_x0, clip_x1). Returns where the next character would go.
    pub fn big_text(&mut self, mut x: i32, y: i32, s: &[u8], c: u16, clip_x0: i32, clip_x1: i32) -> i32 {
        for &ch in s {
            let g = &BIG[usize::from(if (32..=126).contains(&ch) { ch } else { b'?' } - 32)];
            for (r, &bits) in g.iter().enumerate() {
                for k in 0..5 {
                    if bits & 1 << k != 0 && x + k >= clip_x0 && x + k < clip_x1 {
                        self.pixel(x + k, y + r as i32, c);
                    }
                }
            }
            x += 6;
        }
        x
    }

    /// 3 x 5 text. Returns where the next character would go.
    pub fn small_text(&mut self, mut x: u32, y: u32, s: &[u8], c: u16) -> u32 {
        for &ch in s {
            let g = if (32..=126).contains(&ch) { &SMALL[usize::from(ch - 32)] } else { &SMALL[0] };
            let gw = if g[0] != 0 { u32::from(g[0]) } else { 3 };
            if g[0] != 0 {
                for r in 0..5 {
                    for k in 0..gw {
                        if g[1 + r] & 1 << k != 0 {
                            self.pixel((x + k) as i32, (y + r as u32) as i32, c);
                        }
                    }
                }
            }
            x += gw + 1;
        }
        x
    }

    /// A title bar 7 high across the picture, with the title in gold between ridges, or ridges right across for "".
    pub fn title_bar(&mut self, y: u32, title: &[u8]) {
        let w = W as u32;
        self.fill(1, y, w - 2, 7, TITLE_BG);
        let tw = small_width(title);
        let tx = (w - tw) / 2;
        for r in 1..6 {
            let c = if r & 1 != 0 { RIDGE_LIGHT } else { RIDGE_DARK };
            if tw == 0 {
                self.fill(10, y + r, w - 24, 1, c);
                continue;
            }
            self.fill(10, y + r, tx - 14, 1, c);
            self.fill(tx + tw + 4, y + r, w - 14 - (tx + tw + 4), 1, c);
        }
        self.small_text(tx, y + 1, title, GOLD);
        // A menu box on the left and two on the right, as the windows of the time had.
        for bx in [3, 104, 111] {
            self.fill(bx, y + 1, 5, 5, BUTTON);
            self.edge(bx, y + 1, 5, 5, BUTTON_LIGHT, BUTTON_DARK);
        }
        self.pixel(5, y as i32 + 3, ICON);
        self.fill(106, y + 4, 1, 1, ICON);
        self.fill(113, y + 3, 1, 1, ICON);
    }

    /// A sprite of w x h colours, skipping the transparent one.
    pub fn sprite(&mut self, x: i32, y: i32, w: u32, h: u32, data: &[u16], transparent: u16) {
        for r in 0..h {
            for c in 0..w {
                let v = data[(r * w + c) as usize];
                if v != transparent {
                    self.pixel(x + c as i32, y + r as i32, v);
                }
            }
        }
    }
}
