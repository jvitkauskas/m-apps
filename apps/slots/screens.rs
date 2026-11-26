// SPDX-License-Identifier: MIT
//! What slots shows, in the launcher's look, drawn on the canvas only, so they also draw on a computer
//! (tests/slots_screens_host.rs).
use crate::slotfile::{Slot, Slots};
use kit::canvas::{self, BAR_Y, BLUE, Canvas, DIM, FAINT, Face, GREEN, INK, LINE, N, ORANGE, RED, WHITE, rgb, text_w};
use kit::textentry;

const N_I: i32 = N as i32;
const TILE_X: i32 = 8;
const TILE_W: i32 = 224;
const TILE_H: i32 = 44;
const TILE_Y0: i32 = 46;
const TILE_STEP: i32 = 50;
const ROWS: usize = 3;
const ICON: i32 = 34;
const GREY: u16 = rgb(170, 180, 194);

/// The list: "save" first, then the slots, then "back".
pub const SCREEN_SAVE: usize = 0;

/// What can be done with a slot, on its page.
#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    Load,
    Delete,
    Back,
}

/// A line of text made here, and its length.
struct Line {
    bytes: [u8; 48],
    n: usize,
}

impl Line {
    const fn new() -> Line {
        Line { bytes: [0; 48], n: 0 }
    }

    fn push(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.bytes.len() - 1 {
                self.bytes[self.n] = c;
                self.n += 1;
            }
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.n]
    }
}

fn keys_line(board: u32, play: &[u8]) -> Line {
    let mut line = Line::new();
    line.push(if board == 2 { b"<> MOVE  SEQ PLAY " } else { b"-/+ MOVE  PLAY " });
    line.push(play);
    line
}

fn back_key(board: u32) -> &'static [u8] {
    if board == 2 { b"SEQ REC BACK" } else { b"REC BACK" }
}

fn header(canvas: &mut Canvas, title: &[u8], right: &[u8]) {
    canvas.clear(0, 0, N_I, BAR_Y);
    canvas.text(16, 12, Face::Big, title, INK, 11);
    if !right.is_empty() {
        canvas.text(N_I - 16 - text_w(Face::Small, right, 8), 17, Face::Small, right, DIM, 8);
    }
}

/// A letter on a colour, the slot's own from its name, as the launcher shows an app with no icon.
fn letter_icon(canvas: &mut Canvas, x: i32, y: i32, name: &[u8], colour: u16) {
    let letter = name.first().map(|&c| [if c.is_ascii_lowercase() { c - 32 } else { c }]);
    let letter = letter.as_ref().map_or(&[][..], |l| &l[..]);
    canvas.shape(x, y, ICON, ICON, 9, colour, 16, 1);
    canvas.text(x + (ICON - text_w(Face::Big, letter, 10)) / 2, y + 7, Face::Big, letter, WHITE, 10);
}

fn name_colour(name: &[u8]) -> u16 {
    const COLOURS: [u16; 6] =
        [rgb(84, 132, 220), rgb(232, 120, 72), rgb(120, 96, 210), rgb(46, 160, 140), rgb(214, 84, 120), rgb(90, 150, 70)];
    let h = name.iter().fold(5381u32, |h, &c| h.wrapping_mul(33).wrapping_add(u32::from(c | 32)));
    COLOURS[(h % COLOURS.len() as u32) as usize]
}

fn plus_icon(canvas: &mut Canvas, x: i32, y: i32) {
    canvas.shape(x, y, ICON, ICON, 9, BLUE, 16, 1);
    canvas.shape(x + ICON / 2 - 2, y + 9, 4, ICON - 18, 1, WHITE, 16, 1);
    canvas.shape(x + 9, y + ICON / 2 - 2, ICON - 18, 4, 1, WHITE, 16, 1);
}

fn back_icon(canvas: &mut Canvas, x: i32, y: i32) {
    canvas.shape(x, y, ICON, ICON, 9, GREY, 16, 1);
    canvas.text(x + 11, y + 7, Face::Big, b"<", WHITE, 10);
}

/// KB, or MB with a tenth.
fn put_size(out: &mut Line, bytes: u32) {
    let mut tmp = [0u8; 12];
    let mut n = 0;
    let kb = bytes / 1024;
    let mb = kb >= 1024;
    let mut v = if mb { kb * 10 / 1024 } else { kb };
    loop {
        tmp[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if mb && n == 1 {
            tmp[n] = b'.';
            n += 1;
        }
        if v == 0 && !(mb && n < 3) {
            break;
        }
    }
    out.n = 0;
    while n > 0 {
        n -= 1;
        out.push(&tmp[n..n + 1]);
    }
    out.push(if mb { b" MB" } else { b" KB" });
}

fn slot_detail(slot: &Slot, here: bool, pending: bool, line: &mut Line) -> u16 {
    line.n = 0;
    if pending {
        line.push(b"Load stopped: load it again");
        ORANGE
    } else if !slot.sound {
        line.push(b"Damaged");
        RED
    } else if here {
        line.push(b"On the unit now");
        GREEN
    } else {
        put_size(line, slot.file.size);
        DIM
    }
}

pub fn screen_items(s: &Slots) -> usize {
    s.count + 2
}

/// A small chevron: more this way.
fn more_mark(canvas: &mut Canvas, y: i32, up: bool) {
    for i in 0..4 {
        let w = if up { 1 + 2 * i } else { 7 - 2 * i };
        canvas.fill(N_I / 2 - w / 2, y + i, w, 1, FAINT);
    }
}

/// The list, with `selection` marked. `note`, if not empty, goes in the bar instead of the key to go back.
#[allow(clippy::too_many_arguments)]
pub fn screen_list(
    canvas: &mut Canvas,
    s: &Slots,
    board: u32,
    selection: usize,
    here: Option<usize>,
    pending: Option<usize>,
    note: &[u8],
    note_colour: u16,
) {
    let items = screen_items(s);
    let mut top = selection.saturating_sub(1);
    if top + ROWS > items {
        top = items.saturating_sub(ROWS);
    }
    let mut right = Line::new();
    let count = s.count;
    if count >= 10 {
        right.push(&[b'0' + (count / 10 % 10) as u8]);
    }
    right.push(&[b'0' + (count % 10) as u8]);
    right.push(if count == 1 { b" slot" } else { b" slots" });
    header(canvas, b"Slots", right.as_bytes());
    let mut line = Line::new();
    let mut room = [0u8; 64];
    for (r, item) in (top..items).take(ROWS).enumerate() {
        let y = TILE_Y0 + r as i32 * TILE_STEP;
        canvas.card(TILE_X, y, TILE_W, TILE_H, 12, item == selection);
        let (name, detail, detail_colour): (&[u8], &[u8], u16);
        if item == SCREEN_SAVE {
            plus_icon(canvas, TILE_X + 6, y + 5);
            (name, detail, detail_colour) = (b"Save what's on the unit", b"As a new slot, named on the keys", DIM);
        } else if item == items - 1 {
            back_icon(canvas, TILE_X + 6, y + 5);
            (name, detail, detail_colour) = (b"Back", b"To the launcher", DIM);
        } else {
            let slot = &s.list[item - 1];
            letter_icon(canvas, TILE_X + 6, y + 5, slot.name(), if slot.sound { name_colour(slot.name()) } else { GREY });
            let colour = slot_detail(slot, Some(item - 1) == here, Some(item - 1) == pending, &mut line);
            (name, detail, detail_colour) = (slot.name(), line.as_bytes(), colour);
        }
        let fitted = canvas::fit(Face::Big, name, TILE_W - 56, &mut room);
        canvas.text(TILE_X + 48, y + 5, Face::Big, fitted, INK, 8);
        let fitted = canvas::fit(Face::Small, detail, TILE_W - 56, &mut room);
        canvas.text(TILE_X + 48, y + 24, Face::Small, fitted, detail_colour, 8);
    }
    if top != 0 {
        more_mark(canvas, TILE_Y0 - 8, true);
    }
    if top + ROWS < items {
        more_mark(canvas, TILE_Y0 + ROWS as i32 * TILE_STEP - 6, false);
    }
    canvas.mark(0, 0, N_I, BAR_Y);
    let (top_line, top_colour) = if note.is_empty() { (back_key(board), DIM) } else { (note, note_colour) };
    canvas.bar(top_line, top_colour, keys_line(board, b"OPEN").as_bytes());
}

fn label(action: Action) -> &'static [u8] {
    match action {
        Action::Load => b"Load it onto the unit",
        Action::Delete => b"Delete it",
        Action::Back => b"Back",
    }
}

/// An action's button: white, or filled when selected, with a hold filling it with white from the left.
fn draw_action(canvas: &mut Canvas, action: Action, i: usize, on: bool, held_percent: u32) {
    let (y, w) = (114 + i as i32 * 28, N_I - 24);
    let fill = if action == Action::Delete { RED } else { BLUE };
    canvas.clear(10, y - 2, w + 4, 28);
    canvas.shape(12, y, w, 24, 12, if on { fill } else { WHITE }, 16, 1);
    if !on {
        canvas.shape(12, y, w, 24, 12, LINE, 8, 1);
        canvas.shape(13, y + 1, w - 2, 22, 11, WHITE, 16, 1);
    }
    if on && held_percent != 0 {
        let lit = w * held_percent.min(100) as i32 / 100;
        canvas.shape(12, y, lit, 24, 12, WHITE, 7, 1);
    }
    let text = label(action);
    let x = 12 + (w - text_w(Face::Small, text, 8)) / 2;
    canvas.text(x, y + 5, Face::Small, text, if on { WHITE } else { INK }, 8);
    canvas.mark(10, y - 2, w + 4, 28);
}

/// Only the selected action's button, as a hold goes on.
pub fn screen_slot_hold(canvas: &mut Canvas, actions: &[Action], selected: usize, held_percent: u32) {
    draw_action(canvas, actions[selected], selected, true, held_percent);
}

/// A slot's page: its actions, the one selected, and how far a hold to confirm has gone (0..100).
#[allow(clippy::too_many_arguments)]
pub fn screen_slot(
    canvas: &mut Canvas,
    slot: &Slot,
    board: u32,
    here: bool,
    pending: bool,
    actions: &[Action],
    selected: usize,
    held_percent: u32,
) {
    header(canvas, b"Slot", b"");
    canvas.card(12, 40, N_I - 24, 64, 14, false);
    letter_icon(canvas, 24, 52, slot.name(), if slot.sound { name_colour(slot.name()) } else { GREY });
    let mut room = [0u8; 64];
    let fitted = canvas::fit(Face::Big, slot.name(), N_I - 24 - 68, &mut room);
    canvas.text(68, 50, Face::Big, fitted, INK, 8);
    let mut line = Line::new();
    let colour = slot_detail(slot, here, pending, &mut line);
    if colour == DIM {
        // With the size, how much of the flash it fills.
        put_size(&mut line, slot.file.size);
    }
    let fitted = canvas::fit(Face::Small, line.as_bytes(), N_I - 24 - 68, &mut room);
    canvas.text(68, 70, Face::Small, fitted, colour, 8);
    for (i, &action) in actions.iter().enumerate() {
        draw_action(canvas, action, i, i == selected, if i == selected { held_percent } else { 0 });
    }
    canvas.mark(0, 0, N_I, BAR_Y);
    let a = actions[selected];
    let smk = board == 2;
    let top: &[u8] = match a {
        Action::Load if smk => b"Hold SEQ PLAY to load",
        Action::Load => b"Hold PLAY to load",
        Action::Delete if smk => b"Hold SEQ PLAY to delete",
        Action::Delete => b"Hold PLAY to delete",
        Action::Back => back_key(board),
    };
    let bottom = keys_line(board, if a == Action::Back { b"BACK" } else { b"HOLD" });
    canvas.bar(top, if a == Action::Delete { RED } else { DIM }, bottom.as_bytes());
}

/// Naming a slot: the text so far, what it'll be if left empty, the layer, the piano keys held, the cursor's blink.
#[allow(clippy::too_many_arguments)]
pub fn screen_name(
    canvas: &mut Canvas,
    board: u32,
    text: &[u8],
    placeholder: &[u8],
    layer: u32,
    held: u32,
    cursor: bool,
    note: &[u8],
) {
    header(canvas, b"Name the slot", b"");
    canvas.card(12, 40, N_I - 24, 40, 12, true);
    // The end of the text, if it's longer than the field.
    let mut shown = text;
    while !shown.is_empty() && text_w(Face::Big, shown, 8) > N_I - 24 - 30 {
        shown = &shown[1..];
    }
    let mut end = 26;
    if !text.is_empty() {
        end = canvas.text(26, 50, Face::Big, shown, INK, 8);
    } else {
        canvas.text(26, 50, Face::Big, placeholder, FAINT, 8);
    }
    if cursor {
        canvas.fill(end + 1, 50, 2, 19, BLUE);
    }
    const LAYERS: [&[u8]; 3] = [b"abc", b"ABC", b"123 &()"];
    canvas.text(16, 88, Face::Small, LAYERS[layer as usize], if layer != 0 { BLUE } else { DIM }, 8);
    let hold = b"Hold OCT- for ABC, OCT+ for 123";
    canvas.text(N_I - 16 - text_w(Face::Small, hold, 8), 88, Face::Small, hold, FAINT, 8);
    canvas.mark(0, 0, N_I, 110);
    textentry::piano(canvas, 110, layer, held);
    canvas.text(16, 160, Face::Small, b"The keys type. The last one deletes.", DIM, 8);
    canvas.mark(0, 156, N_I, BAR_Y - 156);
    let smk = board == 2;
    let (top, top_colour): (&[u8], u16) = if !note.is_empty() {
        (note, RED)
    } else {
        (if smk { b"SEQ PLAY SAVE" } else { b"PLAY SAVE" }, DIM)
    };
    canvas.bar(top, top_colour, if smk { b"SEQ REC CANCEL" } else { b"REC CANCEL" });
}

const WORK_X: i32 = 12;
const WORK_Y: i32 = 64;
const WORK_W: i32 = N_I - 24;
const BAR_X: i32 = WORK_X + 16;
const BAR_TOP: i32 = WORK_Y + 58;
const BAR_W: i32 = WORK_W - 32;

/// Work that takes a while: the whole screen, then only the bar as it moves ([`screen_work_progress`]).
pub fn screen_work(canvas: &mut Canvas, title: &[u8], line: &[u8], warn: bool) {
    header(canvas, b"Slots", b"");
    canvas.card(WORK_X, WORK_Y, WORK_W, 84, 14, false);
    canvas.text(WORK_X + 16, WORK_Y + 12, Face::Big, title, INK, 8);
    canvas.text(WORK_X + 16, WORK_Y + 33, Face::Small, line, if warn { ORANGE } else { DIM }, 8);
    canvas.shape(BAR_X, BAR_TOP, BAR_W, 8, 4, rgb(222, 230, 240), 16, 1);
    canvas.mark(0, 0, N_I, BAR_Y);
    canvas.bar(if warn { b"Don't switch off" } else { b"" }, if warn { ORANGE } else { DIM }, b"");
}

pub fn screen_work_progress(canvas: &mut Canvas, done: u32, total: u32) {
    let w = if total != 0 { (BAR_W as u32 * done.min(total) / total) as i32 } else { 0 };
    canvas.fill(BAR_X - 2, BAR_TOP - 2, BAR_W + 4, 12, WHITE);
    canvas.shape(BAR_X, BAR_TOP, BAR_W, 8, 4, rgb(222, 230, 240), 16, 1);
    if w >= 8 {
        canvas.shape(BAR_X, BAR_TOP, w, 8, 4, BLUE, 16, 1);
    }
    canvas.mark(BAR_X - 2, BAR_TOP - 2, BAR_W + 4, 12);
}
