// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Charles Vestal
//! Typing on the piano keys, as fm1-radio types a WiFi password (app/app.c): the 27 keys F3..G5 type a..z, and G5
//! deletes. Holding OCT- types capitals, and holding OCT+ digits (F3..D4) and symbols (D#4..F#5). The symbols here are
//! those a file name may have; with [`all_symbols`] on, as for a password, holding both types the rest (F3..A4). The
//! keyboard is drawn as a little piano, each key with what it types now.
//!
//! Adapted from fm1-radio's password page (app/app.c: pw_char, pw_layer, pw_key, ui_piano), drawn with the kit's
//! canvas, and with symbols a file name may have.
use crate::canvas::{BLUE, Canvas, Face, INK, LINE, N, WHITE, text_w};
use crate::{Global, keys};

pub const LOWER: u32 = 0;
pub const UPPER: u32 = 1;
pub const SYMBOLS: u32 = 2;
pub const MORE: u32 = 3;
/// What a key types when it types nothing; 0 is delete.
pub const NOTHING: u8 = 1;
/// The piano's height on the canvas.
pub const PIANO_H: i32 = 46;

/// With OCT+ held: digits on F3..D4, then these on D#4..F#5. A file name can't have \ / : * ? " < > |.
const SYMBOL_KEYS: &[u8] = b" -_.,!&()+=#@$%'";
/// With both held, for a password: the printable ASCII left.
const MORE_KEYS: &[u8] = b"\"*/:;<>?[\\]^`{|}~";

static ALL_SYMBOLS: Global<bool> = Global::new(false);

/// Every printable ASCII character, or only those a file name may have (the default).
pub fn all_symbols(on: bool) {
    ALL_SYMBOLS.set(on);
}

/// What piano key k (0 = F3 .. 26 = G5) types in a layer, 0 for delete, or NOTHING.
pub fn char_for(key: u32, layer: u32) -> u8 {
    if key >= 26 {
        return 0;
    }
    let k = key as usize;
    match layer {
        MORE => MORE_KEYS.get(k).copied().unwrap_or(NOTHING),
        SYMBOLS if k < 10 => b'0' + k as u8,
        SYMBOLS => SYMBOL_KEYS[k - 10],
        UPPER => b'A' + k as u8,
        _ => b'a' + k as u8,
    }
}

/// The layer the held buttons (from [`keys`]) choose.
pub fn layer(buttons: u32) -> u32 {
    let both = keys::OCT_UP | keys::OCT_DOWN;
    if ALL_SYMBOLS.read() && buttons & both == both {
        MORE
    } else if buttons & keys::OCT_UP != 0 {
        SYMBOLS
    } else if buttons & keys::OCT_DOWN != 0 {
        UPPER
    } else {
        LOWER
    }
}

/// Applies the piano keys pressed to text, which ends in a zero, and has room for `most` characters and the zero.
/// Returns whether it changed.
pub fn type_keys(text: &mut [u8], most: usize, pressed: u32, layer: u32) -> bool {
    let mut n = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    let mut changed = false;
    for k in 0..keys::PIANO {
        if pressed >> k & 1 == 0 {
            continue;
        }
        match char_for(k, layer) {
            NOTHING => {}
            0 => {
                if n > 0 {
                    n -= 1;
                    text[n] = 0;
                    changed = true;
                }
            }
            c if n < most && n + 1 < text.len() => {
                text[n] = c;
                n += 1;
                text[n] = 0;
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/// F3..G5 as a little piano at y on the canvas, 240 wide and PIANO_H high, with held keys lit: 16 white keys and 11
/// black, each with what it types now.
pub fn piano(canvas: &mut Canvas, y: i32, layer: u32, held: u32) {
    // From F.
    const BLACK: [bool; 12] = [false, true, false, true, false, true, false, false, true, false, true, false];
    let (kw, x0, wh, bh) = (14, 8, PIANO_H - 2, 27);
    canvas.clear(0, y, N as i32, PIANO_H);
    let label = |k: u32, delete: u8| -> ([u8; 1], usize) {
        match char_for(k, layer) {
            0 => ([delete], usize::from(delete != 0)),
            NOTHING => ([0], 0),
            c => ([c], 1),
        }
    };
    // White keys first, then the black ones over them.
    let mut wi = 0;
    for k in 0..keys::PIANO {
        if BLACK[(k % 12) as usize] {
            continue;
        }
        let x = x0 + wi * kw;
        let lit = held >> k & 1 != 0;
        let (t, n) = label(k, b'<');
        let t = &t[..n];
        canvas.shape(x, y, kw - 1, wh, 2, if lit { BLUE } else { WHITE }, 16, 1);
        canvas.shape(x, y, kw - 1, wh, 2, LINE, 6, 1);
        canvas.shape(x + 1, y, kw - 3, wh - 1, 2, if lit { BLUE } else { WHITE }, 16, 1);
        canvas.text(x + (kw - 1 - text_w(Face::Small, t, 8)) / 2, y + wh - 17, Face::Small, t, if lit { WHITE } else { INK }, 8);
        wi += 1;
    }
    wi = 0;
    for k in 0..keys::PIANO {
        if !BLACK[(k % 12) as usize] {
            wi += 1;
            continue;
        }
        let x = x0 + wi * kw - 6;
        let lit = held >> k & 1 != 0;
        let (t, n) = label(k, 0);
        let t = &t[..n];
        canvas.shape(x, y, 12, bh, 2, if lit { BLUE } else { INK }, 16, 1);
        canvas.text(x + (12 - text_w(Face::Small, t, 8)) / 2, y + 5, Face::Small, t, WHITE, 8);
    }
    canvas.mark(0, y, N as i32, PIANO_H);
}
