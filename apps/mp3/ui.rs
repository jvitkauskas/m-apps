// SPDX-License-Identifier: MIT
//! The player's look: a 90s desktop player, the main window over its playlist, drawn at 120 x 120 and shown at twice
//! that. It keeps what it last drew, and only redraws and sends what changed.
//!
//! An original skin in the style of the 90s desktop players: brushed metal, ridged title bars, a black LCD with segment
//! digits and an analyser, and a green-on-black playlist. Everything is drawn here, nothing is a bitmap.
use crate::spectrum::{BANDS, TOP};
use kit::canvas::Hooks;
use kit::pixel::{self, H, Pixel, W, rgb};

pub const STOPPED: u32 = 0;
pub const PLAYING: u32 = 1;
pub const PAUSED: u32 = 2;

/// The longest name a track can have for the screen, in bytes.
pub const NAME_BYTES: usize = kit::fat::NAME_BYTES;

/// The tracks' names, for the screen.
pub trait Names {
    /// Track `track`'s name for the screen, without .MP3, written to `out`: returns how many bytes it took.
    fn name(&self, track: u32, out: &mut [u8; NAME_BYTES]) -> usize;
}

pub struct Model<'a> {
    pub mode: u32,
    /// Seconds.
    pub elapsed: u32,
    pub total: u32,
    /// 0 until a file says.
    pub kbps: u32,
    pub hz: u32,
    pub channels: u32,
    /// Tracks; the playlist's last row, count, is "back".
    pub count: u32,
    pub selected: u32,
    /// The track playing or paused, or -1.
    pub playing: i32,
    pub names: &'a dyn Names,
    /// Shown instead of the list, and in the title, if set.
    pub note: Option<&'static [u8]>,
    /// Held, as board_keys() says.
    pub keys: u32,
    /// The bars, if there are any.
    pub levels: Option<[u8; BANDS]>,
}

// The palette.
const GROOVE: u16 = rgb(0x10, 0x13, 0x1b);
const LIST_TEXT: u16 = rgb(0x10, 0xe8, 0x30);
const LIST_PLAYING: u16 = rgb(0xff, 0xff, 0xff);
const LIST_SELECTED: u16 = rgb(0x10, 0x28, 0xc8);
const LIST_BACK: u16 = rgb(0x80, 0x90, 0xa8);
const SCROLL_THUMB: u16 = rgb(0x7c, 0x86, 0xa0);
const PEAK: u16 = rgb(0xd8, 0xd8, 0xe0);
const PAUSE_YELLOW: u16 = rgb(0xf0, 0xd0, 0x30);
const STOP_RED: u16 = rgb(0xe0, 0x40, 0x30);
const SCROLL_TRACK: u16 = rgb(0x1a, 0x1e, 0x2a);

// Where things are, in the 120 x 120 picture.
const MAIN_H: u32 = 58;
const LCD_X: u32 = 3;
const LCD_Y: u32 = 9;
const LCD_W: u32 = 48;
const LCD_H: u32 = 31;
const ICON_X: u32 = 5;
const ICON_Y: u32 = 13;
const DIGITS_Y: u32 = 11;
const BARS_X: u32 = 6;
const BARS_Y: u32 = 24;
const BAR_W: u32 = 2;
const BAR_PITCH: u32 = 3;
const MARQUEE_X: u32 = 53;
const MARQUEE_Y: u32 = 9;
const MARQUEE_W: u32 = 64;
const MARQUEE_H: u32 = 11;
const INFO_Y: u32 = 22;
const LIGHTS_Y: u32 = 31;
const POS_X: u32 = 3;
const POS_Y: u32 = 42;
const POS_W: u32 = 114;
const POS_H: u32 = 6;
const THUMB_W: u32 = 10;
const BUTTONS_Y: u32 = 49;
const BUTTON_H: u32 = 8;
const LIST_TOP: u32 = 58;
const LIST_X: u32 = 3;
const LIST_Y: u32 = 66;
const LIST_W: u32 = 114;
const LIST_H: u32 = 47;
const ROW_H: u32 = 9;
const ROWS: u32 = 5;
const ROW_CHARS: usize = 17;
const FOOTER_Y: u32 = 114;
const MARQUEE_BYTES: usize = 200;

/// What was drawn, to find what changed.
#[derive(Clone, Copy)]
struct Last {
    mode: u32,
    elapsed: u32,
    kbps: u32,
    khz: u32,
    channels: u32,
    selected: u32,
    top: u32,
    count: u32,
    thumb: u32,
    buttons: u32,
    playing: i32,
    digits_shown: bool,
    note: Option<&'static [u8]>,
    started: bool,
}

const LAST_NONE: Last = Last {
    mode: 0,
    elapsed: 0,
    kbps: 0,
    khz: 0,
    channels: 0,
    selected: 0,
    top: 0,
    count: 0,
    thumb: 0,
    buttons: 0,
    playing: 0,
    digits_shown: false,
    note: None,
    started: false,
};

pub struct Ui {
    board: u32,
    last: Last,
    marquee: [u8; MARQUEE_BYTES],
    marquee_length: usize,
    marquee_offset: u32,
    marquee_ms: u32,
    blink_ms: u32,
    bars_ms: u32,
    bars: [u8; BANDS],
    peaks: [u8; BANDS],
    drawn_bars: [u8; BANDS],
    drawn_peaks: [u8; BANDS],
    peak_hold: [u16; BANDS],
}

/// Triangles 5 tall: three wide (small) or five (large), pointing right or left.
fn triangle(pix: &mut Pixel, x: u32, y: u32, large: bool, left: bool, c: u16) {
    const SMALL_COLS: [[u8; 2]; 3] = [[0, 5], [1, 3], [2, 1]];
    const LARGE_COLS: [[u8; 2]; 5] = [[0, 5], [0, 5], [1, 3], [1, 3], [2, 1]];
    let n = if large { 5 } else { 3 };
    for k in 0..n {
        let col = if large { LARGE_COLS[k as usize] } else { SMALL_COLS[k as usize] };
        pix.fill(if left { x + n - 1 - k } else { x + k }, y + u32::from(col[0]), 1, u32::from(col[1]), c);
    }
}

/// Segment digits in a 7 x 11 cell.
fn digit(pix: &mut Pixel, x: u32, y: u32, d: u32, shown: bool) {
    const SEGMENTS: [u8; 10] = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];
    const SEG: [[u8; 4]; 7] =
        [[1, 0, 5, 1], [6, 1, 1, 4], [6, 6, 1, 4], [1, 10, 5, 1], [0, 6, 1, 4], [0, 1, 1, 4], [1, 5, 5, 1]];
    let on = if d <= 9 && shown { SEGMENTS[d as usize] } else { 0 };
    pix.fill(x, y, 7, 11, pixel::LCD);
    for (s, &[sx, sy, w, h]) in SEG.iter().enumerate() {
        let c = if on & 1 << s != 0 { pixel::LCD_ON } else { pixel::LCD_OFF };
        pix.fill(x + u32::from(sx), y + u32::from(sy), u32::from(w), u32::from(h), c);
    }
}

fn draw_time(pix: &mut Pixel, seconds: u32, shown: bool) {
    let m = (seconds / 60).min(99);
    let s = seconds % 60;
    digit(pix, 12, DIGITS_Y, m / 10, shown);
    digit(pix, 20, DIGITS_Y, m % 10, shown);
    pix.fill(28, DIGITS_Y, 1, 11, pixel::LCD);
    let dots = if shown { pixel::LCD_ON } else { pixel::LCD_OFF };
    pix.pixel(28, DIGITS_Y as i32 + 3, dots);
    pix.pixel(28, DIGITS_Y as i32 + 7, dots);
    digit(pix, 30, DIGITS_Y, s / 10, shown);
    digit(pix, 38, DIGITS_Y, s % 10, shown);
    pix.mark(12, DIGITS_Y, 33, 11);
}

fn draw_state(pix: &mut Pixel, mode: u32) {
    pix.fill(ICON_X, ICON_Y, 5, 5, pixel::LCD);
    if mode == PLAYING {
        triangle(pix, ICON_X, ICON_Y, true, false, pixel::LCD_ON);
    } else if mode == PAUSED {
        pix.fill(ICON_X, ICON_Y, 2, 5, PAUSE_YELLOW);
        pix.fill(ICON_X + 3, ICON_Y, 2, 5, PAUSE_YELLOW);
    } else {
        pix.fill(ICON_X, ICON_Y, 5, 5, STOP_RED);
    }
    pix.mark(ICON_X, ICON_Y, 5, 5);
}

/// Bar colours, top row first: red, orange, yellow, then greens.
fn bar_colour(row_from_top: u32) -> u16 {
    const C: [u16; TOP as usize] = [
        rgb(0xff, 0x30, 0x20),
        rgb(0xff, 0x50, 0x20),
        rgb(0xff, 0x80, 0x18),
        rgb(0xff, 0xa8, 0x18),
        rgb(0xff, 0xd0, 0x20),
        rgb(0xf0, 0xf0, 0x20),
        rgb(0xc8, 0xf0, 0x20),
        rgb(0xa0, 0xf0, 0x20),
        rgb(0x78, 0xf0, 0x20),
        rgb(0x50, 0xe8, 0x20),
        rgb(0x30, 0xe0, 0x28),
        rgb(0x20, 0xd0, 0x28),
        rgb(0x18, 0xc0, 0x28),
        rgb(0x10, 0xb0, 0x28),
    ];
    C[(row_from_top as usize).min(TOP as usize - 1)]
}

/// Writes v in `out.len()` digits, with leading zeros.
fn number(out: &mut [u8], mut v: u32) {
    for o in out.iter_mut().rev() {
        *o = b'0' + (v % 10) as u8;
        v /= 10;
    }
}

fn draw_info(pix: &mut Pixel, kbps: u32, khz: u32, channels: u32) {
    let mut t = [0u8; 3];
    pix.fill(MARQUEE_X + 1, INFO_Y + 1, 13, 5, pixel::LCD);
    if kbps != 0 {
        number(&mut t, kbps.min(999));
        pix.small_text(MARQUEE_X + 1, INFO_Y + 1, &t, pixel::LCD_ON);
    }
    pix.fill(89, INFO_Y + 1, 9, 5, pixel::LCD);
    if khz != 0 {
        number(&mut t[..2], khz.min(99));
        pix.small_text(90, INFO_Y + 1, &t[..2], pixel::LCD_ON);
    }
    pix.small_text(MARQUEE_X, LIGHTS_Y, b"MONO", if channels == 1 { pixel::LCD_ON } else { pixel::LCD_OFF });
    pix.small_text(74, LIGHTS_Y, b"STEREO", if channels == 2 { pixel::LCD_ON } else { pixel::LCD_OFF });
    pix.mark(MARQUEE_X, INFO_Y, 64, 14);
}

fn thumb_at(elapsed: u32, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    elapsed.min(total) * (POS_W - THUMB_W) / total
}

fn draw_position(pix: &mut Pixel, at: u32) {
    pix.fill(POS_X, POS_Y, POS_W, POS_H, GROOVE);
    pix.edge(POS_X, POS_Y, POS_W, POS_H, pixel::INSET_DARK, pixel::INSET_LIGHT);
    let x = POS_X + at;
    pix.fill(x, POS_Y, THUMB_W, POS_H, pixel::BUTTON);
    pix.edge(x, POS_Y, THUMB_W, POS_H, pixel::BUTTON_LIGHT, pixel::BUTTON_DARK);
    pix.fill(x + 4, POS_Y + 2, 2, 2, pixel::BUTTON_DARK);
    pix.mark(POS_X, POS_Y, POS_W, POS_H);
}

const B_PREV: u32 = 0;
const B_PLAY: u32 = 1;
const B_PAUSE: u32 = 2;
const B_STOP: u32 = 3;
const B_NEXT: u32 = 4;
const B_EJECT: u32 = 5;
const BUTTONS: usize = 6;
const BUTTON_X: [u8; BUTTONS] = [3, 15, 27, 39, 51, 66];
const BUTTON_W: [u8; BUTTONS] = [12, 12, 12, 12, 12, 11];

fn draw_icon(pix: &mut Pixel, which: u32, x: u32, y: u32, c: u16) {
    match which {
        B_PREV => {
            pix.fill(x, y, 1, 5, c);
            triangle(pix, x + 1, y, false, true, c);
            triangle(pix, x + 4, y, false, true, c);
        }
        B_NEXT => {
            triangle(pix, x, y, false, false, c);
            triangle(pix, x + 3, y, false, false, c);
            pix.fill(x + 6, y, 1, 5, c);
        }
        B_PLAY => triangle(pix, x + 1, y, true, false, c),
        B_PAUSE => {
            pix.fill(x + 1, y, 2, 5, c);
            pix.fill(x + 4, y, 2, 5, c);
        }
        B_STOP => pix.fill(x + 1, y, 5, 5, c),
        B_EJECT => {
            for k in 0..3 {
                pix.fill(x + 2 - k, y + k, 1 + 2 * k, 1, c);
            }
            pix.fill(x, y + 4, 5, 1, c);
        }
        _ => {}
    }
}

fn draw_buttons(pix: &mut Pixel, down: u32) {
    for b in 0..BUTTONS as u32 {
        let (x, w) = (u32::from(BUTTON_X[b as usize]), u32::from(BUTTON_W[b as usize]));
        let d = down & 1 << b != 0;
        pix.fill(x, BUTTONS_Y, w, BUTTON_H, if d { pixel::BUTTON_DOWN } else { pixel::BUTTON });
        let (light, dark) =
            if d { (pixel::BUTTON_DARK, pixel::BUTTON_LIGHT) } else { (pixel::BUTTON_LIGHT, pixel::BUTTON_DARK) };
        pix.edge(x, BUTTONS_Y, w, BUTTON_H, light, dark);
        let iw = if b == B_EJECT { 5 } else { 7 };
        let pushed = u32::from(d);
        draw_icon(pix, b, x + (w - iw) / 2 + pushed, BUTTONS_Y + 1 + pushed + (BUTTON_H - 2 - 5) / 2, pixel::ICON);
    }
    pix.mark(3, BUTTONS_Y, 74, BUTTON_H);
}

/// The badge: a gold M with its shadow, in a dark square.
fn draw_badge(pix: &mut Pixel) {
    let (x, y) = (104, BUTTONS_Y - 1);
    pix.fill(x, y, 13, 10, pixel::TITLE_BG);
    pix.edge(x, y, 13, 10, pixel::INSET_DARK, pixel::INSET_LIGHT);
    pix.big_text(x as i32 + 5, y as i32 + 2, b"M", pixel::GOLD_SHADOW, 0, W as i32);
    pix.big_text(x as i32 + 4, y as i32 + 1, b"M", pixel::GOLD, 0, W as i32);
}

fn draw_list(pix: &mut Pixel, m: &Model, top: u32) {
    let (x0, y0, inner_w) = (LIST_X + 1, LIST_Y + 1, LIST_W - 2);
    pix.fill(x0, y0, inner_w, LIST_H - 2, pixel::LCD);
    let note = if m.count == 0 { m.note } else { None };
    if let Some(note) = note {
        let w = (note.len() as u32 * 6).wrapping_sub(1);
        // As C's unsigned sums: a note too wide for the list starts far to the left, and isn't seen.
        let x = x0.wrapping_add((inner_w - 4).wrapping_sub(w) / 2) as i32;
        pix.big_text(x, (y0 + 2 * ROW_H + 1) as i32, note, pixel::LCD_ON, x0 as i32, (x0 + inner_w - 4) as i32);
    }
    for i in 0..ROWS {
        if note.is_some() {
            break;
        }
        let (item, y) = (top + i, y0 + i * ROW_H);
        if item > m.count {
            break;
        }
        if item == m.selected {
            pix.fill(x0, y, inner_w - 5, ROW_H, LIST_SELECTED);
        }
        let mut line = [0u8; ROW_CHARS];
        let mut n = 0;
        let c;
        if item == m.count {
            n = 7;
            line[..n].copy_from_slice(b"<< BACK");
            c = LIST_BACK;
        } else {
            let number_of = item + 1;
            if number_of >= 10 {
                line[n] = b'0' + (number_of / 10 % 10) as u8;
                n += 1;
            }
            line[n] = b'0' + (number_of % 10) as u8;
            line[n + 1] = b'.';
            line[n + 2] = b' ';
            n += 3;
            let mut name = [0u8; NAME_BYTES];
            let length = m.names.name(item, &mut name);
            for &ch in &name[..length] {
                if n >= ROW_CHARS {
                    break;
                }
                line[n] = ch;
                n += 1;
            }
            c = if item as i32 == m.playing { LIST_PLAYING } else { LIST_TEXT };
        }
        pix.big_text(x0 as i32 + 1, y as i32 + 1, &line[..n], c, x0 as i32, (x0 + inner_w - 5) as i32);
    }
    // The scroll bar, on the right.
    let (bx, bh, items) = (x0 + inner_w - 4, LIST_H - 2, m.count + 1);
    pix.fill(bx, y0, 4, bh, SCROLL_TRACK);
    let mut th = if items > ROWS { bh * ROWS / items } else { bh };
    let ty = if items > ROWS { (bh - th) * top / (items - ROWS) } else { 0 };
    if th < 4 {
        th = 4;
    }
    pix.fill(bx, y0 + ty, 4, th, SCROLL_THUMB);
    pix.edge(bx, y0 + ty, 4, th, pixel::BUTTON_LIGHT, pixel::BUTTON_DARK);
    pix.mark(LIST_X + 1, LIST_Y + 1, LIST_W - 2, LIST_H - 2);
}

fn draw_static(pix: &mut Pixel) {
    let (w, h) = (W as u32, H as u32);
    pix.body(0, 0, w, h);
    pix.edge(0, 0, w, MAIN_H, pixel::EDGE_LIGHT, pixel::EDGE_DARK);
    pix.edge(0, LIST_TOP, w, h - LIST_TOP, pixel::EDGE_LIGHT, pixel::EDGE_DARK);
    pix.title_bar(1, b"");
    pix.title_bar(LIST_TOP + 1, b"PLAYLIST");
    pix.fill(LCD_X, LCD_Y, LCD_W, LCD_H, pixel::LCD);
    pix.edge(LCD_X, LCD_Y, LCD_W, LCD_H, pixel::INSET_DARK, pixel::INSET_LIGHT);
    pix.fill(MARQUEE_X, MARQUEE_Y, MARQUEE_W, MARQUEE_H, pixel::LCD);
    pix.edge(MARQUEE_X, MARQUEE_Y, MARQUEE_W, MARQUEE_H, pixel::INSET_DARK, pixel::INSET_LIGHT);
    pix.fill(MARQUEE_X, INFO_Y, 15, 7, pixel::LCD);
    pix.edge(MARQUEE_X, INFO_Y, 15, 7, pixel::INSET_DARK, pixel::INSET_LIGHT);
    pix.small_text(70, INFO_Y + 1, b"KBPS", pixel::LABEL);
    pix.fill(88, INFO_Y, 11, 7, pixel::LCD);
    pix.edge(88, INFO_Y, 11, 7, pixel::INSET_DARK, pixel::INSET_LIGHT);
    pix.small_text(101, INFO_Y + 1, b"KHZ", pixel::LABEL);
    pix.fill(LIST_X, LIST_Y, LIST_W, LIST_H, pixel::LCD);
    pix.edge(LIST_X, LIST_Y, LIST_W, LIST_H, pixel::INSET_DARK, pixel::INSET_LIGHT);
    draw_badge(pix);
    pix.mark(0, 0, w, h);
}

/// m:ss, or mm:ss from ten minutes. Returns how many bytes it wrote.
fn put_time(out: &mut [u8], seconds: u32) -> usize {
    let m = seconds / 60;
    let mut n = 0;
    if m >= 10 {
        out[n] = b'0' + (m / 10 % 10) as u8;
        n += 1;
    }
    out[n] = b'0' + (m % 10) as u8;
    out[n + 1] = b':';
    out[n + 2] = b'0' + (seconds % 60 / 10) as u8;
    out[n + 3] = b'0' + (seconds % 10) as u8;
    n + 4
}

/// The title: the note, or the track and its length, or where "back" goes. Returns how many bytes it wrote.
fn make_marquee(m: &Model, out: &mut [u8; MARQUEE_BYTES]) -> usize {
    let mut n = 0;
    let mut track = 0;
    let mut buffer = [0u8; NAME_BYTES];
    let name: &[u8] = if let Some(note) = m.note {
        note
    } else if m.playing >= 0 {
        track = m.playing as u32 + 1;
        let length = m.names.name(m.playing as u32, &mut buffer);
        &buffer[..length]
    } else if m.selected < m.count {
        track = m.selected + 1;
        let length = m.names.name(m.selected, &mut buffer);
        &buffer[..length]
    } else if m.count != 0 {
        b"BACK TO THE MENU"
    } else {
        b"MP3 PLAYER"
    };
    if track != 0 {
        if track >= 10 {
            out[n] = b'0' + (track / 10 % 10) as u8;
            n += 1;
        }
        out[n] = b'0' + (track % 10) as u8;
        out[n + 1] = b'.';
        out[n + 2] = b' ';
        n += 3;
    }
    for &c in name {
        if n + 12 >= MARQUEE_BYTES {
            break;
        }
        out[n] = c;
        n += 1;
    }
    if m.playing >= 0 && m.note.is_none() && m.total != 0 {
        out[n] = b' ';
        out[n + 1] = b'(';
        n += 2;
        n += put_time(&mut out[n..], m.total);
        out[n] = b')';
        n += 1;
    }
    n
}

fn pressed_buttons(m: &Model) -> u32 {
    let mut down = 0;
    if m.keys & 1 != 0 {
        down |= 1 << B_PREV;
    }
    if m.keys & 2 != 0 {
        down |= 1 << B_NEXT;
    }
    if m.keys & 4 != 0 {
        down |= 1
            << if m.selected >= m.count {
                B_EJECT
            } else if m.mode == PLAYING {
                B_PAUSE
            } else {
                B_PLAY
            };
    }
    if m.keys & 8 != 0 {
        down |= 1 << B_EJECT;
    }
    down
}

impl Ui {
    pub const fn new() -> Ui {
        Ui {
            board: 0,
            last: LAST_NONE,
            marquee: [0; MARQUEE_BYTES],
            marquee_length: 0,
            marquee_offset: 0,
            marquee_ms: 0,
            blink_ms: 0,
            bars_ms: 0,
            bars: [0; BANDS],
            peaks: [0; BANDS],
            drawn_bars: [0; BANDS],
            drawn_peaks: [0; BANDS],
            peak_hold: [0; BANDS],
        }
    }

    fn draw_bars(&mut self, pix: &mut Pixel) {
        for b in 0..BANDS {
            let x = BARS_X + b as u32 * BAR_PITCH;
            for r in 0..u32::from(TOP) {
                let level = u32::from(TOP) - r; // the level this row shows
                let c = if level <= u32::from(self.bars[b]) {
                    bar_colour(r)
                } else if level == u32::from(self.peaks[b]) && self.peaks[b] != 0 {
                    PEAK
                } else {
                    pixel::LCD
                };
                pix.fill(x, BARS_Y + r, BAR_W, 1, c);
            }
            self.drawn_bars[b] = self.bars[b];
            self.drawn_peaks[b] = self.peaks[b];
        }
        pix.mark(BARS_X, BARS_Y, BANDS as u32 * BAR_PITCH, u32::from(TOP));
    }

    fn draw_marquee(&self, pix: &mut Pixel) {
        pix.fill(MARQUEE_X + 1, MARQUEE_Y + 1, MARQUEE_W - 2, MARQUEE_H - 2, pixel::LCD);
        let (x0, x1) = (MARQUEE_X as i32 + 2, (MARQUEE_X + MARQUEE_W) as i32 - 2);
        let y = MARQUEE_Y as i32 + 2;
        let text = &self.marquee[..self.marquee_length];
        if self.marquee_length as u32 * 6 <= (x1 - x0) as u32 {
            pix.big_text(x0, y, text, pixel::LCD_ON, x0, x1);
        } else {
            // It goes round: the text, a gap, the text again.
            let mut x = x0 - self.marquee_offset as i32;
            x = pix.big_text(x, y, text, pixel::LCD_ON, x0, x1);
            x = pix.big_text(x, y, b" *** ", pixel::LCD_ON, x0, x1);
            pix.big_text(x, y, text, pixel::LCD_ON, x0, x1);
        }
        pix.mark(MARQUEE_X + 1, MARQUEE_Y + 1, MARQUEE_W - 2, MARQUEE_H - 2);
    }

    fn list_hint(&self, m: &Model) -> &'static [u8] {
        if m.selected >= m.count {
            return if m.mode == STOPPED || m.count == 0 { b"PLAY: LEAVE" } else { b"PLAY: STOP AND LEAVE" };
        }
        if m.mode != STOPPED {
            return b"TAP: TRACK HOLD: SEEK";
        }
        if self.board == 2 { b"< > PICK  PLAY GO" } else { b"OCT-/+ PICK PLAY GO" }
    }

    fn draw_footer(&self, pix: &mut Pixel, m: &Model) {
        pix.body(LIST_X, FOOTER_Y, LIST_W, 5);
        let mut t = [0u8; 5];
        let shown = if m.selected < m.count { m.selected + 1 } else { m.count };
        number(&mut t[..2], shown.min(99));
        t[2] = b'/';
        number(&mut t[3..], m.count.min(99));
        pix.small_text(LIST_X + 1, FOOTER_Y, &t, pixel::LABEL);
        let hint = self.list_hint(m);
        pix.small_text(LIST_X + LIST_W - pixel::small_width(hint), FOOTER_Y, hint, pixel::LABEL);
        pix.mark(LIST_X, FOOTER_Y, LIST_W, 5);
    }

    /// Draws all of it and sends it. board is APP_BOARD, for the key names.
    pub fn start(&mut self, pix: &mut Pixel, board: u32, hooks: &mut impl Hooks) {
        self.board = board;
        self.last = LAST_NONE;
        self.bars = [0; BANDS];
        self.peaks = [0; BANDS];
        self.peak_hold = [0; BANDS];
        self.marquee_length = 0;
        self.marquee_offset = 0;
        self.marquee_ms = 0;
        self.blink_ms = 0;
        self.bars_ms = 0;
        pix.forget();
        draw_static(pix);
        draw_time(pix, 0, true);
        draw_state(pix, STOPPED);
        self.draw_bars(pix);
        draw_info(pix, 0, 0, 0);
        draw_position(pix, 0);
        draw_buttons(pix, 0);
        pix.send_all(hooks);
    }

    /// Redraws what changed, ms after the last call (for scrolling, blinking and the bars falling), and sends it. What
    /// it sends at a time is the picture's budget, and the hooks are what's done around it.
    pub fn update(&mut self, pix: &mut Pixel, m: &Model, ms: u32, hooks: &mut impl Hooks) {
        let last = &mut self.last;
        if !last.started {
            last.started = true;
            last.mode = 0xff;
            last.selected = 0xffff;
            last.count = 0xffff;
            last.top = 0xffff;
            last.thumb = 0xffff;
            last.buttons = 0xffff;
            last.elapsed = 0xffff;
            last.kbps = 0xffff;
        }
        // The time, which blinks while paused.
        self.blink_ms = if m.mode == PAUSED { self.blink_ms + ms } else { 0 };
        let shown = m.mode != PAUSED || (self.blink_ms / 500) % 2 == 0;
        let elapsed = if m.playing >= 0 { m.elapsed } else { 0 };
        if elapsed != self.last.elapsed || shown != self.last.digits_shown {
            draw_time(pix, elapsed, shown);
            self.last.elapsed = elapsed;
            self.last.digits_shown = shown;
        }
        if m.mode != self.last.mode {
            draw_state(pix, m.mode);
            self.draw_footer(pix, m);
        }
        // The bars rise at once, and fall a step at a time; the peaks wait, then fall.
        self.bars_ms += ms;
        if self.bars_ms >= 40 {
            let steps = self.bars_ms / 40;
            self.bars_ms %= 40;
            for b in 0..BANDS {
                let want = match m.levels {
                    Some(levels) if m.mode == PLAYING => u32::from(levels[b]),
                    _ => 0,
                };
                let bar = u32::from(self.bars[b]);
                self.bars[b] = if want >= bar { want } else { bar.saturating_sub(steps) } as u8;
                let hold = u32::from(self.peak_hold[b]);
                if self.bars[b] >= self.peaks[b] {
                    self.peaks[b] = self.bars[b];
                    self.peak_hold[b] = 400;
                } else if hold > 40 * steps {
                    self.peak_hold[b] = (hold - 40 * steps) as u16;
                } else if self.peaks[b] != 0 {
                    self.peaks[b] -= 1;
                    self.peak_hold[b] = 40;
                }
            }
            if self.bars != self.drawn_bars || self.peaks != self.drawn_peaks {
                self.draw_bars(pix);
            }
        }
        // The title, scrolling a pixel each 60 ms when it doesn't fit.
        let mut text = [0u8; MARQUEE_BYTES];
        let length = make_marquee(m, &mut text);
        if length != self.marquee_length || text[..length] != self.marquee[..length] {
            self.marquee[..length].copy_from_slice(&text[..length]);
            self.marquee_length = length;
            self.marquee_offset = 0;
            self.marquee_ms = 0;
            self.draw_marquee(pix);
        } else if self.marquee_length as u32 * 6 > MARQUEE_W - 4 {
            self.marquee_ms += ms;
            if self.marquee_ms >= 60 {
                self.marquee_offset =
                    (self.marquee_offset + self.marquee_ms / 60) % ((self.marquee_length as u32 + 5) * 6);
                self.marquee_ms %= 60;
                self.draw_marquee(pix);
            }
        }
        let khz = (m.hz + 500) / 1000;
        if m.kbps != self.last.kbps || khz != self.last.khz || m.channels != self.last.channels {
            draw_info(pix, m.kbps, khz, m.channels);
            self.last.kbps = m.kbps;
            self.last.khz = khz;
            self.last.channels = m.channels;
        }
        let thumb = if m.playing >= 0 { thumb_at(m.elapsed, m.total) } else { 0 };
        if thumb != self.last.thumb {
            draw_position(pix, thumb);
            self.last.thumb = thumb;
        }
        let down = pressed_buttons(m);
        if down != self.last.buttons {
            draw_buttons(pix, down);
            self.last.buttons = down;
        }
        let mut top = if self.last.top == 0xffff { 0 } else { self.last.top };
        if m.selected < top {
            top = m.selected;
        }
        if m.selected >= top + ROWS {
            top = m.selected - ROWS + 1;
        }
        let last = self.last;
        if top != last.top
            || m.selected != last.selected
            || m.playing != last.playing
            || m.count != last.count
            || m.note != last.note
        {
            draw_list(pix, m, top);
            self.draw_footer(pix, m);
            self.last.top = top;
            self.last.selected = m.selected;
            self.last.playing = m.playing;
            self.last.count = m.count;
            self.last.note = m.note;
        }
        self.last.mode = m.mode;
        pix.send(hooks);
    }
}
