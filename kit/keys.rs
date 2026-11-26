// SPDX-License-Identifier: MPL-2.0
//! All the keys, polled: the piano keys and every button, on the FM-1 and the SMK-37. `board::board_keys()` reads
//! only four; this reads the whole matrix. The pins, the words and the tables are felucca-bsp's (boards/fm1/fm1_input.h,
//! Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments, and Charles Vestal; boards/smk37/fm1_input.h,
//! Copyright (C) 2026 Kerem Kilic (Ellic Studio)); the shifting is m-bootloader's board's. Both MPL-2.0 here.
//!
//! FM-1: 11 columns x 6 rows behind two 74HC595s (PA4 data, PA3 clock, PA1 latch), the high byte first; a column is
//! one bit low. The rows are PA0, PA5..PA8 and PB7 with pull-ups, low = closed; the keys and buttons are on PA5..PA8.
//! SMK-37: 16 columns x 8 rows behind four 74HC595s on the same pins, the low byte first; column c is bit 24+c
//! (c < 8) or 8+c low, and the low 16 bits are the button LEDs, off when high. The rows are PA2, PA5, PA6, PA7, PA9,
//! PH6, PH8 and PH9. The cells of column p are read with column p-1's word latched, as the stock firmware scans.
use crate::Global;
use crate::hw::sync_cpu;
use crate::platform::{gpio_input, gpio_level, gpio_read, hal_delay_us};

/// The 27 piano keys both boards have, F3 to G5: all of the FM-1's, and the SMK-37's keys 5 to 31 of C3..C6.
pub const PIANO: u32 = 27;

/// Buttons, as bits of [`Keys::buttons`]. PREV and NEXT move through a list: OCT- and OCT+ on the FM-1, which has no
/// others, and < and > on the SMK-37. PLAY is SEQ PLAY and REC is SEQ REC on the SMK-37.
pub const PREV: u32 = 0x1;
pub const NEXT: u32 = 0x2;
pub const PLAY: u32 = 0x4;
pub const REC: u32 = 0x8;
pub const OCT_DOWN: u32 = 0x10;
pub const OCT_UP: u32 = 0x20;

#[derive(Clone, Copy, Default)]
pub struct Keys {
    /// Bit k: piano key k held, 0 = F3 .. 26 = G5.
    pub piano: u32,
    /// Went down since the last scan().
    pub piano_pressed: u32,
    /// The buttons held.
    pub buttons: u32,
    pub buttons_pressed: u32,
}

const PA: u32 = 0;
const PH: u32 = 7;
const SETTLE_US: u32 = 10;
const FM_COLUMNS: usize = 11;
const SMK_COLUMNS: usize = 16;
const NONE: u8 = 0xff;

/// FM-1: the id at each column of rows PA5..PA8; 0..13 buttons, 14..40 the piano keys.
const FM_MAP: [[u8; FM_COLUMNS]; 4] = [
    [5, 11, 4, 10, 3, 9, 2, 8, NONE, NONE, NONE],
    [34, 35, 36, 37, 38, 40, 39, 13, 7, 6, 12],
    [23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33],
    [0, 1, 15, 14, 17, 16, 19, 18, 20, 21, 22],
];
const FM_ROWS: [u32; 4] = [5, 6, 7, 8];
/// SMK-37: each key's second contact (it closes once the key is down), column << 4 | row, for keys 5..31, F3..G5.
const SMK_KEYS: [u8; PIANO as usize] = [
    0xf2, 0xe2, 0xc2, 0xa2, 0x82, 0x62, 0x42, 0x22, 0x12, 0x32, 0x33, 0x43, 0x53, 0x63, 0x73, 0x83, 0x93, 0xa3, 0xb3,
    0xc3, 0xd3, 0xe3, 0x23, 0x13, 0x14, 0x24, 0x34,
];
/// SMK-37 buttons: OCTAVE-, OCTAVE+, SEQ PLAY, SEQ REC, <, >.
const SMK_BUTTONS: [(u8, u32); 6] =
    [(0x66, OCT_DOWN), (0x36, OCT_UP), (0x16, PLAY), (0x46, REC), (0x96, PREV), (0x56, NEXT)];
const SMK_ROWS: [(u32, u32); 8] = [(PA, 2), (PA, 5), (PA, 6), (PA, 7), (PA, 9), (PH, 6), (PH, 8), (PH, 9)];

struct State {
    smk: bool,
    last_piano: u32,
    last_buttons: u32,
}

static STATE: Global<State> = Global::new(State { smk: false, last_piano: 0, last_buttons: 0 });

fn smk() -> bool {
    // Safety: read only for a moment.
    unsafe { STATE.get() }.smk
}

fn shift(word: u32) {
    let smk = smk();
    for b in 0..if smk { 4 } else { 2 } {
        let shift_by = if smk { b * 8 } else { (1 - b) * 8 };
        let byte = (word >> shift_by) as u8;
        for bit in (0..8).rev() {
            gpio_level(PA, 4, byte & 1 << bit != 0);
            gpio_level(PA, 3, true);
            sync_cpu();
            gpio_level(PA, 3, false);
        }
    }
    gpio_level(PA, 1, true);
    sync_cpu();
    gpio_level(PA, 1, false);
}

/// After `board_init()`: the matrix's rows as inputs. `board_restore()` puts them back.
pub fn init(smk: bool) {
    unsafe { STATE.get() }.smk = smk;
    if smk {
        for (port, pin) in SMK_ROWS {
            gpio_input(port, pin, true, false);
        }
    } else {
        for pin in FM_ROWS {
            gpio_input(PA, pin, true, false);
        }
    }
}

/// What the matrix says now: piano keys and buttons closed.
fn read_matrix() -> (u32, u32) {
    let (mut piano, mut buttons) = (0, 0);
    if !smk() {
        for p in 0..FM_COLUMNS as u32 {
            shift(0xffff ^ 1 << p ^ if p < 2 { 1 << (11 + p) } else { 0 });
            hal_delay_us(SETTLE_US);
            for (r, &pin) in FM_ROWS.iter().enumerate() {
                let id = FM_MAP[r][p as usize];
                if id == NONE || gpio_read(PA, pin) {
                    continue;
                }
                match id {
                    14.. => piano |= 1 << (id - 14),
                    0 => buttons |= PREV | OCT_DOWN,
                    1 => buttons |= NEXT | OCT_UP,
                    12 => buttons |= PLAY,
                    13 => buttons |= REC,
                    _ => {}
                }
            }
        }
        shift(0xffff);
        return (piano, buttons);
    }
    let mut cells = [0u8; SMK_COLUMNS];
    for (p, cell) in cells.iter_mut().enumerate() {
        let c = ((p + SMK_COLUMNS - 1) % SMK_COLUMNS) as u32;
        shift((0xffff_0000 ^ if c < 8 { 0x0100_0000 << c } else { 0x0001_0000 << (c - 8) }) | 0xffff);
        hal_delay_us(SETTLE_US);
        for (r, &(port, pin)) in SMK_ROWS.iter().enumerate() {
            if !gpio_read(port, pin) {
                *cell |= 1 << r;
            }
        }
    }
    shift(u32::MAX);
    let closed = |key: u8| cells[usize::from(key >> 4)] >> (key & 15) & 1 != 0;
    for (k, &key) in SMK_KEYS.iter().enumerate() {
        if closed(key) {
            piano |= 1 << k;
        }
    }
    for (key, bit) in SMK_BUTTONS {
        if closed(key) {
            buttons |= bit;
        }
    }
    (piano, buttons)
}

/// One scan of the matrix, about 0.3 ms. Call it every few ms: a press counts after two scans closed, a release after
/// two open.
pub fn scan(k: &mut Keys) {
    let (piano, buttons) = read_matrix();
    let s = unsafe { STATE.get() };
    // Two scans the same, either way.
    let piano_now = (k.piano & (piano | s.last_piano)) | (piano & s.last_piano);
    let buttons_now = (k.buttons & (buttons | s.last_buttons)) | (buttons & s.last_buttons);
    s.last_piano = piano;
    s.last_buttons = buttons;
    k.piano_pressed = piano_now & !k.piano;
    k.buttons_pressed = buttons_now & !k.buttons;
    k.piano = piano_now;
    k.buttons = buttons_now;
}
