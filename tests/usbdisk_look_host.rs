// SPDX-License-Identifier: MIT
//! Draws usbdisk's screens on a computer, from the same code, into PPM files: usbdisk_look_host OUT_DIR.
//!
//! Here the screen isn't there to send to, so what's saved is the canvas after each update has sent everything
//! (there's no budget): every part look.rs draws, it marks, so that's what the screen would show.
// The test draws all but the app's last screen.
#[allow(dead_code)]
#[path = "../apps/usbdisk/look.rs"]
mod look;

use kit::canvas::{Canvas, N, Plain};
use look::{Look, Model, State};
use std::io::Write;

fn save(canvas: &Canvas, dir: &str, name: &str) {
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
    let path = format!("{dir}/{name}.ppm");
    let Ok(mut f) = std::fs::File::create(&path) else { std::process::exit(2) };
    f.write_all(&out).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        std::process::exit(2);
    }
    let dir = &args[1];
    // Safety: the only use of the canvas.
    let c = unsafe { kit::canvas::claim() };
    let mut look = Look::new();
    for board in 1..=2 {
        let mut m = Model::new(State::Starting);
        look.start(c, board, &mut Plain);
        look.update(c, &m, 0, &mut Plain);
        save(c, dir, &format!("{board}-0-starting"));
        m.state = State::Waiting;
        m.card_mib = 7452;
        look.update(c, &m, 600, &mut Plain);
        save(c, dir, &format!("{board}-1-waiting"));
        m.state = State::Connected;
        look.update(c, &m, 600, &mut Plain);
        save(c, dir, &format!("{board}-2-connected"));
        m.read_kbs = 870;
        m.read_kib = 52000;
        for _ in 0..7 {
            look.update(c, &m, 50, &mut Plain);
        }
        save(c, dir, &format!("{board}-3-reading"));
        m.read_kbs = 0;
        m.write_kbs = 440;
        m.written_kib = 3100;
        for _ in 0..7 {
            look.update(c, &m, 50, &mut Plain);
        }
        save(c, dir, &format!("{board}-4-writing"));
        m.write_kbs = 0;
        m.state = State::Ejected;
        look.update(c, &m, 50, &mut Plain);
        save(c, dir, &format!("{board}-5-ejected"));
        let none = Model::new(State::NoCard);
        look.start(c, board, &mut Plain);
        look.update(c, &none, 0, &mut Plain);
        save(c, dir, &format!("{board}-6-no-card"));
    }
    println!("OK");
}
