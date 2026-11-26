// SPDX-License-Identifier: MIT
//! What the WiFi app shows, in the launcher's look, drawn on the canvas only, so they also draw on a computer
//! (tests/wifi_screens_host.rs).
use crate::network::{Ap, Joined, OPEN, OTHER, STEPS};
use crate::wifilist::List;
use kit::canvas::{BAR_Y, BLUE, Canvas, DIM, FAINT, Face, GREEN, INK, LINE, N as CANVAS_N, ORANGE, RED, WHITE, fit, rgb, text_w};
use kit::textentry;

const N: i32 = CANVAS_N as i32;
const TILE_X: i32 = 8;
const TILE_W: i32 = 224;
const TILE_H: i32 = 44;
const TILE_Y0: i32 = 46;
const TILE_STEP: i32 = 50;
const ROWS: usize = 3;
const ICON: i32 = 34;
const GREY: u16 = rgb(170, 180, 194);

/// A line of text made here, cut at its room.
struct Line<const M: usize> {
    b: [u8; M],
    n: usize,
}

impl<const M: usize> Line<M> {
    fn new() -> Line<M> {
        Line { b: [0; M], n: 0 }
    }

    fn put(&mut self, s: &[u8]) -> &mut Self {
        let k = s.len().min(M - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s[..k]);
        self.n += k;
        self
    }

    fn number(&mut self, v: i32) -> &mut Self {
        if v < 0 {
            self.put(b"-");
        }
        let (mut tmp, mut k, mut u) = ([0u8; 10], 0, v.unsigned_abs());
        loop {
            tmp[k] = b'0' + (u % 10) as u8;
            k += 1;
            u /= 10;
            if u == 0 {
                break;
            }
        }
        tmp[..k].reverse();
        self.put(&tmp[..k])
    }

    fn ip(&mut self, ip: u32) -> &mut Self {
        for i in (0..4).rev() {
            self.number((ip >> (8 * i) & 255) as i32);
            if i != 0 {
                self.put(b".");
            }
        }
        self
    }

    fn text(&self) -> &[u8] {
        &self.b[..self.n]
    }
}

/// The keys' line: moving, then what PLAY does.
fn keys_line(board: u32, play: &[u8]) -> Line<47> {
    let mut line = Line::new();
    line.put(if board == 2 { b"<> MOVE  SEQ PLAY " } else { b"-/+ MOVE  PLAY " }).put(play);
    line
}

fn back_key(board: u32) -> &'static [u8] {
    if board == 2 { b"SEQ REC BACK" } else { b"REC BACK" }
}

/// The title big, or smaller and cut to fit, as a network's name may need.
fn header(c: &mut Canvas, title: &[u8], right: &[u8]) {
    c.clear(0, 0, N, BAR_Y);
    let mut room = N - 32;
    if !right.is_empty() {
        let mut cut = [0u8; 64];
        let right = fit(Face::Small, right, 110, &mut cut);
        let w = text_w(Face::Small, right, 8);
        c.text(N - 16 - w, 17, Face::Small, right, DIM, 8);
        room -= w + 8;
    }
    if text_w(Face::Big, title, 11) <= room {
        c.text(16, 12, Face::Big, title, INK, 11);
    } else {
        let mut cut = [0u8; 64];
        c.text(16, 15, Face::Big, fit(Face::Big, title, room, &mut cut), INK, 8);
    }
}

/// Four bars, as strong as the signal, white on a colour. bars: 0 to 4.
fn signal_icon(c: &mut Canvas, x: i32, y: i32, bars: u32, colour: u16) {
    c.shape(x, y, ICON, ICON, 9, colour, 16, 1);
    for i in 0..4 {
        let (h, bx) = (6 + i * 5, x + 7 + i * 6);
        c.shape(bx, y + ICON - 7 - h, 4, h, 1, WHITE, if (i as u32) < bars { 16 } else { 6 }, 1);
    }
}

fn bars(rssi: i8) -> u32 {
    match rssi {
        -55.. => 4,
        -67.. => 3,
        -75.. => 2,
        _ => 1,
    }
}

fn plus_icon(c: &mut Canvas, x: i32, y: i32) {
    c.shape(x, y, ICON, ICON, 9, BLUE, 16, 1);
    c.shape(x + ICON / 2 - 2, y + 9, 4, ICON - 18, 1, WHITE, 16, 1);
    c.shape(x + 9, y + ICON / 2 - 2, ICON - 18, 4, 1, WHITE, 16, 1);
}

fn back_icon(c: &mut Canvas, x: i32, y: i32) {
    c.shape(x, y, ICON, ICON, 9, GREY, 16, 1);
    c.text(x + 11, y + 7, Face::Big, b"<", WHITE, 10);
}

fn pen_icon(c: &mut Canvas, x: i32, y: i32) {
    c.shape(x, y, ICON, ICON, 9, rgb(120, 96, 210), 16, 1);
    c.text(x + (ICON - text_w(Face::Big, b"Aa", 8)) / 2, y + 8, Face::Big, b"Aa", WHITE, 8);
}

/// A small chevron: more this way.
fn more_mark(c: &mut Canvas, y: i32, up: bool) {
    for i in 0..4 {
        let w = if up { 1 + 2 * i } else { 7 - 2 * i };
        c.fill(N / 2 - w / 2, y + i, w, 1, FAINT);
    }
}

fn tile(c: &mut Canvas, row: usize, marked: bool) {
    c.card(TILE_X, TILE_Y0 + row as i32 * TILE_STEP, TILE_W, TILE_H, 12, marked);
}

fn tile_text(c: &mut Canvas, row: usize, name: &[u8], detail: &[u8], detail_colour: u16) {
    let y = TILE_Y0 + row as i32 * TILE_STEP;
    let mut cut = [0u8; 64];
    c.text(TILE_X + 48, y + 5, Face::Big, fit(Face::Big, name, TILE_W - 56, &mut cut), INK, 8);
    let mut cut = [0u8; 64];
    c.text(TILE_X + 48, y + 24, Face::Small, fit(Face::Small, detail, TILE_W - 56, &mut cut), detail_colour, 8);
}

/// The rows shown with the selection in view, one above it when there is one.
fn first_row(selection: usize, items: usize) -> usize {
    let mut top = selection.saturating_sub(1);
    if top + ROWS > items {
        top = items.saturating_sub(ROWS);
    }
    top
}

fn more_marks(c: &mut Canvas, top: usize, items: usize) {
    if top != 0 {
        more_mark(c, TILE_Y0 - 8, true);
    }
    if top + ROWS < items {
        more_mark(c, TILE_Y0 + ROWS as i32 * TILE_STEP - 6, false);
    }
}

// ---- the saved networks ----

/// The saved networks: "Add a network" first, then the networks, then "Back".
pub const ADD: usize = 0;

pub fn list_items(l: &List) -> usize {
    l.count + 2
}

fn network_detail(l: &List, i: usize) -> (&'static [u8], u16) {
    if i == l.in_use {
        (b"In use", GREEN)
    } else if l.net[i].pass[0] != 0 {
        (b"Saved", DIM)
    } else {
        (b"Saved, open", DIM)
    }
}

/// The note, if there is one, or what to show without one, and its colour.
fn top_line<'a>(note: &'a [u8], note_colour: u16, otherwise: &'a [u8], colour: u16) -> (&'a [u8], u16) {
    if note.is_empty() { (otherwise, colour) } else { (note, note_colour) }
}

pub fn list(c: &mut Canvas, l: &List, board: u32, selection: usize, note: &[u8], note_colour: u16) {
    let (items, top) = (list_items(l), first_row(selection, list_items(l)));
    let mut right = Line::<24>::new();
    right.number(l.count as i32).put(if l.count == 1 { b" network" } else { b" networks" });
    header(c, b"WiFi", right.text());
    for r in (0..ROWS).take_while(|r| top + r < items) {
        let item = top + r;
        let y = TILE_Y0 + r as i32 * TILE_STEP;
        tile(c, r, item == selection);
        if item == ADD {
            plus_icon(c, TILE_X + 6, y + 5);
            tile_text(c, r, b"Add a network", b"From those heard here", DIM);
        } else if item == items - 1 {
            back_icon(c, TILE_X + 6, y + 5);
            tile_text(c, r, b"Back", b"To the launcher", DIM);
        } else {
            let i = item - 1;
            signal_icon(c, TILE_X + 6, y + 5, 4, if i == l.in_use { GREEN } else { BLUE });
            let (line, colour) = network_detail(l, i);
            tile_text(c, r, l.net[i].ssid(), line, colour);
        }
    }
    more_marks(c, top, items);
    c.mark(0, 0, N, BAR_Y);
    let (t, colour) = top_line(note, note_colour, back_key(board), DIM);
    c.bar(t, colour, keys_line(board, b"OPEN").text());
}

// ---- a saved network ----

/// A saved network's page: its actions.
pub const ACTION_TEST: u8 = 0;
pub const ACTION_USE: u8 = 1;
pub const ACTION_PASSWORD: u8 = 2;
pub const ACTION_DELETE: u8 = 3;

const LABELS: [&[u8]; 4] = [b"Test the connection", b"Use this one", b"Change the password", b"Delete it"];
const ACTION_Y: i32 = 98;
const ACTION_STEP: i32 = 25;

fn draw_action(c: &mut Canvas, action: u8, i: usize, on: bool, held_percent: u32) {
    let (y, w) = (ACTION_Y + i as i32 * ACTION_STEP, N - 24);
    let fill = if action == ACTION_DELETE { RED } else { BLUE };
    c.clear(10, y - 2, w + 4, ACTION_STEP);
    c.shape(12, y, w, 22, 11, if on { fill } else { WHITE }, 16, 1);
    if !on {
        c.shape(12, y, w, 22, 11, LINE, 8, 1);
        c.shape(13, y + 1, w - 2, 20, 10, WHITE, 16, 1);
    }
    if on && held_percent != 0 {
        let lit = w * held_percent.min(100) as i32 / 100;
        c.shape(12, y, lit, 22, 11, WHITE, 7, 1);
    }
    let label = LABELS[usize::from(action)];
    c.text(12 + (w - text_w(Face::Small, label, 8)) / 2, y + 4, Face::Small, label, if on { WHITE } else { INK }, 8);
    c.mark(10, y - 2, w + 4, ACTION_STEP);
}

/// How far a hold to confirm has gone (0..100), on the action selected.
pub fn network_hold(c: &mut Canvas, actions: &[u8], selected: usize, held_percent: u32) {
    draw_action(c, actions[selected], selected, true, held_percent);
}

#[allow(clippy::too_many_arguments)]
pub fn network(
    c: &mut Canvas,
    l: &List,
    index: usize,
    board: u32,
    actions: &[u8],
    selected: usize,
    held_percent: u32,
    note: &[u8],
    note_colour: u16,
) {
    header(c, b"Network", b"");
    c.card(12, 40, N - 24, 50, 14, false);
    signal_icon(c, 22, 48, 4, if index == l.in_use { GREEN } else { BLUE });
    let mut cut = [0u8; 64];
    c.text(66, 46, Face::Big, fit(Face::Big, l.net[index].ssid(), N - 24 - 66, &mut cut), INK, 8);
    let (line, colour) = network_detail(l, index);
    c.text(66, 66, Face::Small, line, colour, 8);
    for (i, &a) in actions.iter().enumerate() {
        draw_action(c, a, i, i == selected, if i == selected { held_percent } else { 0 });
    }
    c.mark(0, 0, N, BAR_Y);
    let a = actions[selected];
    let (top, colour) = if a == ACTION_DELETE {
        (if board == 2 { &b"Hold SEQ PLAY to delete"[..] } else { b"Hold PLAY to delete" }, RED)
    } else {
        (back_key(board), DIM)
    };
    let (t, colour) = top_line(note, note_colour, top, colour);
    c.bar(t, colour, keys_line(board, if a == ACTION_DELETE { b"HOLD" } else { b"OK" }).text());
}

// ---- listening, and what was heard ----

const WORK_X: i32 = 12;
const WORK_Y: i32 = 64;
const WORK_W: i32 = N - 24;
const PROGRESS_X: i32 = WORK_X + 16;
const PROGRESS_Y: i32 = WORK_Y + 58;
const PROGRESS_W: i32 = WORK_W - 32;

/// Listening for networks: what's done of total.
pub fn scanning(c: &mut Canvas, done: u32, total: u32) {
    if done == 0 {
        header(c, b"Add a network", b"");
        c.card(WORK_X, WORK_Y, WORK_W, 84, 14, false);
        c.text(WORK_X + 16, WORK_Y + 12, Face::Big, b"Listening", INK, 8);
        c.text(WORK_X + 16, WORK_Y + 33, Face::Small, b"For networks on channels 1 to 13", DIM, 8);
        c.mark(0, 0, N, BAR_Y);
        c.bar(b"", DIM, b"");
    }
    let w = if total != 0 { (PROGRESS_W as u32 * done.min(total) / total) as i32 } else { 0 };
    c.fill(PROGRESS_X - 2, PROGRESS_Y - 2, PROGRESS_W + 4, 12, WHITE);
    c.shape(PROGRESS_X, PROGRESS_Y, PROGRESS_W, 8, 4, rgb(222, 230, 240), 16, 1);
    if w >= 8 {
        c.shape(PROGRESS_X, PROGRESS_Y, w, 8, 4, BLUE, 16, 1);
    }
    c.mark(PROGRESS_X - 2, PROGRESS_Y - 2, PROGRESS_W + 4, 12);
}

/// What was heard: the networks, then "Type a name", then "Back".
pub fn heard_items(count: usize) -> usize {
    count + 2
}

pub fn heard(c: &mut Canvas, aps: &[Ap], l: &List, board: u32, selection: usize) {
    let count = aps.len();
    let (items, top) = (heard_items(count), first_row(selection, heard_items(count)));
    let mut right = Line::<24>::new();
    right.number(count as i32).put(b" heard");
    header(c, b"Add a network", right.text());
    for r in (0..ROWS).take_while(|r| top + r < items) {
        let item = top + r;
        let y = TILE_Y0 + r as i32 * TILE_STEP;
        tile(c, r, item == selection);
        if item == count {
            pen_icon(c, TILE_X + 6, y + 5);
            tile_text(c, r, b"Type a name", b"For one not heard, or hidden", DIM);
        } else if item == count + 1 {
            back_icon(c, TILE_X + 6, y + 5);
            tile_text(c, r, b"Back", b"To the saved networks", DIM);
        } else {
            let ap = &aps[item];
            let ok = ap.security != OTHER;
            let mut line = Line::<48>::new();
            let mut colour = DIM;
            signal_icon(c, TILE_X + 6, y + 5, bars(ap.rssi), if ok { BLUE } else { GREY });
            if !ok {
                line.put(b"WPA3 or other: can't join it");
                colour = ORANGE;
            } else {
                if ap.security == OPEN {
                    line.put(b"Open, ");
                }
                line.number(i32::from(ap.rssi)).put(b" dBm, channel ").number(i32::from(ap.channel));
                if l.find(ap.ssid()).is_some() {
                    line.put(b", saved");
                    colour = GREEN;
                }
            }
            tile_text(c, r, ap.ssid(), line.text(), colour);
        }
    }
    more_marks(c, top, items);
    c.mark(0, 0, N, BAR_Y);
    let (t, colour) = if count != 0 { (back_key(board), DIM) } else { (&b"Nothing heard"[..], ORANGE) };
    c.bar(t, colour, keys_line(board, b"CHOOSE").text());
}

// ---- typing ----

/// Typing a name or a password: the title, the network's name (or none), the text so far, what's shown when it's
/// empty, the layer, the piano keys held, the cursor's blink, and a note (in red) or none.
#[allow(clippy::too_many_arguments)]
pub fn typing(
    c: &mut Canvas,
    board: u32,
    title: &[u8],
    network: &[u8],
    text: &[u8],
    placeholder: &[u8],
    layer: u32,
    held: u32,
    cursor: bool,
    note: &[u8],
) {
    header(c, title, network);
    c.card(12, 40, N - 24, 40, 12, true);
    // The end of the text, if it's longer than the field.
    let mut shown = text;
    while !shown.is_empty() && text_w(Face::Big, shown, 8) > N - 24 - 30 {
        shown = &shown[1..];
    }
    let mut end = 26;
    if !text.is_empty() {
        end = c.text(26, 50, Face::Big, shown, INK, 8);
    } else if !placeholder.is_empty() {
        let mut cut = [0u8; 64];
        c.text(26, 50, Face::Big, fit(Face::Big, placeholder, N - 24 - 30, &mut cut), FAINT, 8);
    }
    if cursor {
        c.fill(end + 1, 50, 2, 19, BLUE);
    }
    const LAYERS: [&[u8]; 4] = [b"abc", b"ABC", b"123 &()", b"more #<>"];
    c.text(16, 88, Face::Small, LAYERS[layer as usize], if layer != 0 { BLUE } else { DIM }, 8);
    let hold = b"OCT- ABC, OCT+ 123, both more";
    c.text(N - 16 - text_w(Face::Small, hold, 8), 88, Face::Small, hold, FAINT, 8);
    c.mark(0, 0, N, 110);
    textentry::piano(c, 110, layer, held);
    let mut count = Line::<48>::new();
    count.number(text.len() as i32).put(b" characters. The last key deletes.");
    c.text(16, 160, Face::Small, count.text(), DIM, 8);
    c.mark(0, 156, N, BAR_Y - 156);
    let (t, colour) = top_line(note, RED, if board == 2 { b"SEQ PLAY DONE" } else { b"PLAY DONE" }, DIM);
    c.bar(t, colour, if board == 2 { b"SEQ REC CANCEL" } else { b"REC CANCEL" });
}

// ---- a test ----

const STEP_NAMES: [&[u8]; 5] =
    [b"Starting the radio", b"Finding the network", b"Making the key", b"Joining", b"Getting an address"];

/// A test: the network, the step it's on (STEPS when done), and then why it failed, or None if it joined.
#[allow(clippy::too_many_arguments)]
pub fn test(
    c: &mut Canvas,
    ssid: &[u8],
    step: u8,
    done: bool,
    why: Option<&[u8]>,
    r: &Joined,
    bar_top: &[u8],
    bar_colour: u16,
    bar_keys: &[u8],
) {
    header(c, ssid, b"");
    c.card(12, 40, N - 24, 152, 14, false);
    for i in 0..STEPS {
        let y = 50 + i32::from(i) * 17;
        let (failed, passed) = (done && why.is_some() && i == step, i < step || (done && why.is_none()));
        let dot = if failed {
            RED
        } else if passed {
            GREEN
        } else if i == step {
            BLUE
        } else {
            LINE
        };
        c.shape(26, y + 4, 8, 8, 4, dot, 16, 1);
        c.text(42, y, Face::Small, STEP_NAMES[usize::from(i)], if i <= step || done { INK } else { FAINT }, 8);
    }
    if done {
        let mut cut = [0u8; 64];
        if let Some(why) = why {
            c.text(26, 140, Face::Big, fit(Face::Big, why, N - 24 - 28, &mut cut), RED, 8);
            if r.channel != 0 {
                let mut line = Line::<64>::new();
                line.put(b"Heard at ").number(i32::from(r.rssi)).put(b" dBm, channel ").number(i32::from(r.channel));
                c.text(26, 164, Face::Small, line.text(), DIM, 8);
            }
        } else {
            let mut line = Line::<64>::new();
            line.put(b"Connected: ").ip(r.ip);
            c.text(26, 138, Face::Big, fit(Face::Big, line.text(), N - 24 - 28, &mut cut), GREEN, 8);
            let mut line = Line::<64>::new();
            line.number(i32::from(r.rssi)).put(b" dBm, channel ").number(i32::from(r.channel)).put(b", ");
            line.number((r.ms / 1000) as i32).put(b".").number((r.ms / 100 % 10) as i32).put(b" s");
            c.text(26, 160, Face::Small, line.text(), DIM, 8);
            let mut line = Line::<64>::new();
            line.put(b"Gateway ").ip(r.gateway);
            c.text(26, 175, Face::Small, line.text(), DIM, 8);
        }
    }
    c.mark(0, 0, N, BAR_Y);
    c.bar(bar_top, bar_colour, bar_keys);
}
