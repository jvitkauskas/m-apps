// SPDX-License-Identifier: MIT
//! What usbdisk shows, in the launcher's look: an SD card cabled to a laptop, with dots running along the cable while
//! data moves, and what's happening in a panel below. The same on both boards, but for the name of the key to leave.
//!
//! Drawn into a full-size picture, with soft edges, in the launcher's colours and font (apps/picker). Only what changed
//! is sent. It only draws on the canvas, so it also draws on a computer (tests/usbdisk_look_host.rs).
#[path = "art.rs"]
mod art;

use art::*;
use kit::canvas::{BLUE, Canvas, DIM, Face, Hooks, INK, LINE, N, ORANGE, RED, WHITE, rgb, text_w};

const SCREEN: u16 = rgb(236, 243, 251);

// Where things go.
const CARD_X: i32 = 16;
const CARD_Y: i32 = 50;
const LAPTOP_X: i32 = 124;
const LAPTOP_Y: i32 = CARD_Y + SD_CARD_PORT_Y - LAPTOP_PORT_Y;
const CABLE_Y: i32 = CARD_Y + SD_CARD_PORT_Y;
const CABLE_X0: i32 = CARD_X + SD_CARD_W + 4;
const CABLE_X1: i32 = LAPTOP_X - 4;
const PANEL_X: i32 = 12;
const PANEL_Y: i32 = 130;
const PANEL_W: i32 = 216;
const PANEL_H: i32 = 58;
const DOT_GAP: i32 = 10;

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Starting,
    NoCard,
    Waiting,
    Connected,
    Ejected,
    Leaving,
}

/// What there is to show.
#[derive(Clone, Copy)]
pub struct Model {
    pub state: State,
    pub card_mib: u32,
    /// Lately, in KB/s.
    pub read_kbs: u32,
    pub write_kbs: u32,
    /// Since it started.
    pub read_kib: u32,
    pub written_kib: u32,
}

impl Model {
    pub const fn new(state: State) -> Model {
        Model { state, card_mib: 0, read_kbs: 0, write_kbs: 0, read_kib: 0, written_kib: 0 }
    }
}

// ---- numbers ----

/// A line of text being put together.
struct Line {
    bytes: [u8; 48],
    n: usize,
}

impl Line {
    fn new() -> Line {
        Line { bytes: [0; 48], n: 0 }
    }

    fn text(&mut self, s: &[u8]) -> &mut Line {
        self.bytes[self.n..self.n + s.len()].copy_from_slice(s);
        self.n += s.len();
        self
    }

    fn number(&mut self, mut v: u32) -> &mut Line {
        let mut tmp = [0u8; 10];
        let mut n = 0;
        loop {
            tmp[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self.bytes[self.n] = tmp[n];
            self.n += 1;
        }
        self
    }

    /// MB, or GB with a tenth.
    fn size(&mut self, mib: u32) -> &mut Line {
        if mib < 1024 {
            return self.number(mib).text(b" MB");
        }
        self.number(mib / 1024).text(&[b'.', b'0' + (mib % 1024 * 10 / 1024) as u8]).text(b" GB")
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.n]
    }
}

// ---- the parts ----

#[derive(Clone, Copy)]
struct Last {
    /// None before the first update since start().
    state: Option<State>,
    screen_phase: u32,
    read_kbs: u32,
    write_kbs: u32,
    read_mib: u32,
    written_mib: u32,
    flow: i32,
}

const NOTHING_YET: Last =
    Last { state: None, screen_phase: 0, read_kbs: 0, write_kbs: 0, read_mib: 0, written_mib: 0, flow: 0 };

/// What's on the screen, to redraw only what changed.
pub struct Look {
    board: u32,
    last: Last,
    dots_ms: u32,
    screen_ms: u32,
    phase: u32,
    card_mib: u32,
}

impl Look {
    pub const fn new() -> Look {
        Look { board: 0, last: NOTHING_YET, dots_ms: 0, screen_ms: 0, phase: 0, card_mib: 0 }
    }

    /// The cable: a line, with dots moving along it while data does, towards the laptop when reading.
    fn draw_cable(&self, c: &mut Canvas, flow: i32) {
        let (x0, x1, y) = (CABLE_X0, CABLE_X1, CABLE_Y);
        c.clear(x0 - 3, y - 4, x1 - x0 + 7, 9);
        c.shape(x0, y - 1, x1 - x0, 2, 1, LINE, 16, 1);
        if flow != 0 {
            let colour = if flow > 0 { BLUE } else { ORANGE };
            let shift = (self.phase % DOT_GAP as u32) as i32;
            let mut x = x0 + if flow > 0 { shift } else { DOT_GAP - 1 - shift };
            while x < x1 - 2 {
                c.shape(x, y - 3, 6, 6, 3, colour, 16, 1);
                x += DOT_GAP;
            }
        }
        c.mark(x0 - 3, y - 4, x1 - x0 + 7, 9);
    }
}

/// A bar under the folder on the laptop's screen, with a piece running along it while data moves.
fn draw_bar(c: &mut Canvas, screen_phase: u32, flow: i32) {
    let bx = LAPTOP_X + LAPTOP_SCREEN_X + LAPTOP_SCREEN_W / 2 - 20;
    let by = LAPTOP_Y + LAPTOP_SCREEN_Y + LAPTOP_SCREEN_H / 2 + 12;
    for y in by - 1..by + 5 {
        c.fb[y as usize][(bx - 1) as usize..(bx + 41) as usize].fill(SCREEN);
    }
    if flow != 0 {
        c.shape(bx, by, 40, 4, 2, rgb(214, 224, 236), 16, 1);
        c.shape(bx + (screen_phase % 5) as i32 * 7, by, 12, 4, 2, if flow > 0 { BLUE } else { ORANGE }, 16, 1);
    }
    c.mark(bx - 1, by - 1, 42, 6);
}

/// The laptop's screen: what the computer sees.
fn draw_screen(c: &mut Canvas, state: State) {
    let (sx, sy) = (LAPTOP_X + LAPTOP_SCREEN_X, LAPTOP_Y + LAPTOP_SCREEN_Y);
    let (cx, cy) = (sx + LAPTOP_SCREEN_W / 2, sy + LAPTOP_SCREEN_H / 2);
    c.shape(sx, sy, LAPTOP_SCREEN_W, LAPTOP_SCREEN_H, 2, SCREEN, 16, 1);
    match state {
        State::Starting | State::Waiting => c.sprite(
            cx - FOLDER_DIM_W / 2,
            cy - FOLDER_DIM_H / 2 - 2,
            FOLDER_DIM_W,
            FOLDER_DIM_H,
            &FOLDER_DIM_RGB,
            &FOLDER_DIM_ALPHA,
        ),
        State::NoCard => c.sprite(cx - CROSS_W / 2, cy - CROSS_H / 2, CROSS_W, CROSS_H, &CROSS_RGB, &CROSS_ALPHA),
        State::Connected => {
            c.sprite(cx - FOLDER_W / 2, cy - FOLDER_H / 2 - 4, FOLDER_W, FOLDER_H, &FOLDER_RGB, &FOLDER_ALPHA)
        }
        _ => c.sprite(cx - CHECK_W / 2, cy - CHECK_H / 2, CHECK_W, CHECK_H, &CHECK_RGB, &CHECK_ALPHA),
    }
    c.mark(sx, sy, LAPTOP_SCREEN_W, LAPTOP_SCREEN_H);
}

fn state_title(state: State, flow: i32) -> &'static [u8] {
    match state {
        State::Starting => b"Starting",
        State::NoCard => b"No card found",
        State::Waiting => b"Connect USB",
        State::Connected if flow > 0 => b"Reading",
        State::Connected if flow < 0 => b"Writing",
        State::Connected => b"Connected",
        State::Ejected => b"Ejected",
        State::Leaving => b"Leaving",
    }
}

/// The panel's words: what's happening, the speed, and a line about it. White goes under them, so the panel's edge
/// and shadow are drawn only when it's new.
fn draw_words(c: &mut Canvas, m: &Model, flow: i32) {
    let (x, y) = (PANEL_X, PANEL_Y);
    for py in y + 6..y + PANEL_H - 6 {
        c.fb[py as usize][(x + 10) as usize..(x + PANEL_W - 10) as usize].fill(WHITE);
    }
    c.text(x + 14, y + 9, Face::Big, state_title(m.state, flow), if m.state == State::NoCard { RED } else { INK }, 8);
    let mut line = Line::new();
    if flow != 0 {
        line.number(if flow > 0 { m.read_kbs } else { m.write_kbs }).text(b" KB/s");
        let speed = line.as_bytes();
        let colour = if flow > 0 { BLUE } else { ORANGE };
        c.text(x + PANEL_W - 14 - text_w(Face::Big, speed, 8), y + 9, Face::Big, speed, colour, 8);
    }
    line.n = 0;
    let detail: &[u8] = match m.state {
        State::Starting => b"Getting the card ready",
        State::NoCard => b"Check the card, then try again",
        State::Waiting => line.size(m.card_mib).text(b" card, ready to share").as_bytes(),
        State::Ejected => b"You can unplug the cable now",
        State::Leaving => b"Back to the menu",
        State::Connected => {
            if m.read_kib >= 1024 || m.written_kib >= 1024 {
                line.size(m.read_kib / 1024).text(b" read, ").size(m.written_kib / 1024).text(b" written").as_bytes()
            } else {
                line.size(m.card_mib).text(b" card").as_bytes()
            }
        }
    };
    c.text(x + 14, y + 33, Face::Small, detail, DIM, 8);
    c.mark(x + 10, y + 6, PANEL_W - 20, PANEL_H - 12);
}

fn draw_panel(c: &mut Canvas) {
    let (x, y) = (PANEL_X, PANEL_Y);
    c.clear(x - 4, y - 4, PANEL_W + 8, PANEL_H + 10);
    c.card(x, y, PANEL_W, PANEL_H, 12, false);
    c.mark(x - 4, y - 4, PANEL_W + 8, PANEL_H + 10);
}

fn draw_static(c: &mut Canvas, board: u32) {
    let n = N as i32;
    c.clear(0, 0, n, n);
    c.text(16, 14, Face::Big, b"USB Disk", INK, 8);
    c.sprite(CARD_X, CARD_Y, SD_CARD_W, SD_CARD_H, &SD_CARD_RGB, &SD_CARD_ALPHA);
    c.sprite(LAPTOP_X, LAPTOP_Y, LAPTOP_W, LAPTOP_H, &LAPTOP_RGB, &LAPTOP_ALPHA);
    c.bar(b"Eject before unplugging", DIM, if board == 2 { b"SEQ REC LEAVE" } else { b"REC LEAVE" });
    c.mark(0, 0, n, n);
}

fn draw_size(c: &mut Canvas, card_mib: u32) {
    c.clear(130, 12, 98, 20);
    if card_mib != 0 {
        let mut line = Line::new();
        let text = line.size(card_mib).text(b" card").as_bytes();
        c.text(N as i32 - 14 - text_w(Face::Small, text, 8), 17, Face::Small, text, DIM, 8);
    }
    c.mark(130, 12, 98, 20);
}

impl Look {
    /// Draws it all for this board, and sends it.
    pub fn start(&mut self, c: &mut Canvas, board: u32, hooks: &mut impl Hooks) {
        *self = Look { board, ..Look::new() };
        c.forget();
        draw_static(c, self.board);
        draw_panel(c);
        self.draw_cable(c, 0);
        c.send_all(hooks);
    }

    /// Redraws what changed, ms after the last call, and sends it, up to the canvas's budget; what's left goes next
    /// time. The hooks are called around sending, as the card shares the screen's bus.
    pub fn update(&mut self, c: &mut Canvas, m: &Model, ms: u32, hooks: &mut impl Hooks) {
        let flow = if m.state != State::Connected {
            0
        } else if m.read_kbs > m.write_kbs {
            1
        } else if m.write_kbs != 0 {
            -1
        } else {
            0
        };
        // The dots move a pixel each 40 ms, and the bar on the screen a step each 150 ms.
        self.dots_ms += ms;
        let mut moved = false;
        if flow != 0 && self.dots_ms >= 40 {
            self.phase += self.dots_ms / 40;
            self.dots_ms %= 40;
            moved = true;
        }
        self.screen_ms += ms;
        let screen_phase = if flow != 0 { self.screen_ms / 150 } else { 0 };
        let last = self.last;
        if flow != last.flow || moved {
            self.draw_cable(c, flow);
        }
        // The bar is only there while connected, under the folder.
        if Some(m.state) != last.state {
            draw_screen(c, m.state);
            if m.state == State::Connected {
                draw_bar(c, screen_phase, flow);
            }
        } else if m.state == State::Connected && (flow != last.flow || screen_phase != last.screen_phase) {
            draw_bar(c, screen_phase, flow);
        }
        if m.card_mib != self.card_mib {
            self.card_mib = m.card_mib;
            draw_size(c, self.card_mib);
        }
        let (read_mib, written_mib) = (m.read_kib / 1024, m.written_kib / 1024);
        if Some(m.state) != last.state
            || flow != last.flow
            || (flow > 0 && m.read_kbs != last.read_kbs)
            || (flow < 0 && m.write_kbs != last.write_kbs)
            || read_mib != last.read_mib
            || written_mib != last.written_mib
        {
            draw_words(c, m, flow);
        }
        self.last = Last {
            state: Some(m.state),
            screen_phase,
            read_kbs: m.read_kbs,
            write_kbs: m.write_kbs,
            read_mib,
            written_mib,
            flow,
        };
        c.send(hooks);
    }
}

impl Default for Look {
    fn default() -> Look {
        Look::new()
    }
}
