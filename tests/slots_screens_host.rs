// SPDX-License-Identifier: MIT
//! Draws slots' screens on a computer, from the same code, into PPM files: slots_screens_host OUT_DIR.
//!
//! Here the screen isn't there to send to, so what's saved is the canvas once everything has been sent: every part
//! screens.rs draws, it marks, so that's what the screen would show.
// The screens need the slots' list, and nothing else of slotfile.
#[allow(dead_code)]
#[path = "../apps/slots/slotfile.rs"]
mod slotfile;
// The test draws every screen, but not every piece of each.
#[allow(dead_code)]
#[path = "../apps/slots/screens.rs"]
mod screens;

use kit::canvas::{Canvas, GREEN, N, ORANGE, Plain};
use kit::textentry::{LOWER, SYMBOLS, UPPER};
use screens::Action;
use slotfile::{SLOT_NAME_MOST, Slot, Slots};
use std::io::Write;

fn save(canvas: &mut Canvas, dir: &str, name: &str) {
    canvas.send_all(&mut Plain);
    let mut out = Vec::with_capacity(16 + N * N * 3);
    out.extend_from_slice(b"P6 240 240 255\n");
    for row in &canvas.fb {
        for &c in row {
            out.extend_from_slice(&[
                ((c >> 11) << 3 | (c >> 13)) as u8,
                (((c >> 5) & 63) << 2 | ((c >> 9) & 3)) as u8,
                ((c & 31) << 3 | ((c & 31) >> 2)) as u8,
            ]);
        }
    }
    let Ok(mut f) = std::fs::File::create(format!("{dir}/{name}.ppm")) else { std::process::exit(2) };
    f.write_all(&out).unwrap();
}

fn add(s: &mut Slots, name: &str, size: u32, sound: bool) {
    let mut slot = Slot::EMPTY;
    // Cut to fit, as the app's list has them.
    let n = name.len().min(SLOT_NAME_MOST);
    slot.name[..n].copy_from_slice(&name.as_bytes()[..n]);
    slot.file.size = size;
    slot.sound = sound;
    s.list[s.count] = slot;
    s.count += 1;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        std::process::exit(2);
    }
    let dir = &args[1];
    // Safety: the only use of the canvas.
    let c = unsafe { kit::canvas::claim() };
    let mut s = Box::new(Slots::new());
    add(&mut s, "Felucca 1.8.2 beta", 812 * 1024 + 512, true);
    add(&mut s, "Snoop37 with my patches and a name too long to fit", 655 * 1024, true);
    add(&mut s, "SLOOP", 1024 * 1024, true);
    add(&mut s, "Broken one", 4096, false);
    for board in 1..=2 {
        screens::screen_list(c, &s, board, 0, Some(0), None, b"", 0);
        save(c, dir, &format!("{board}-0-list-save"));
        screens::screen_list(c, &s, board, 2, Some(0), Some(2), b"The last load stopped", ORANGE);
        save(c, dir, &format!("{board}-1-list-pending"));
        screens::screen_list(c, &s, board, 5, Some(0), None, b"Saved", GREEN);
        save(c, dir, &format!("{board}-2-list-end"));
        let actions = [Action::Load, Action::Delete, Action::Back];
        screens::screen_slot(c, &s.list[0], board, false, false, &actions, 0, 0);
        save(c, dir, &format!("{board}-3-slot"));
        screens::screen_slot(c, &s.list[1], board, false, false, &actions, 1, 60);
        save(c, dir, &format!("{board}-4-slot-holding-delete"));
        screens::screen_name(c, board, b"", b"Slot 1", LOWER, 0, true, b"");
        save(c, dir, &format!("{board}-5-name-empty"));
        screens::screen_name(c, board, b"Felucca 1.8", b"Slot 1", UPPER, 1 << 5 | 1 << 12, true, b"");
        save(c, dir, &format!("{board}-6-name-caps"));
        screens::screen_name(c, board, b"My very long slot name that scrolls", b"Slot 1", SYMBOLS, 0, false,
            b"A name can't end in a space or a dot");
        save(c, dir, &format!("{board}-7-name-symbols"));
        screens::screen_work(c, b"Loading the slot", b"Writing the firmware and its settings", true);
        screens::screen_work_progress(c, 40, 100);
        save(c, dir, &format!("{board}-8-work"));
    }
    println!("OK");
}
