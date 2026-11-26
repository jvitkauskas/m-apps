// SPDX-License-Identifier: MIT
//! Draws the launcher's screen on the computer (apps/picker/scene.rs and qoi.rs), for tests/scene_test.py and to see
//! it: scene_host OUT FRAMES [ICON.QOI...] writes FRAMES frames of 240 x 240 RGB565 big-endian to OUT, the selection
//! moving down a tile every 40 frames, then the panel for nothing to start. The icons go to the first apps in turn.
#[path = "../apps/picker/qoi.rs"]
mod qoi;
#[allow(dead_code)]
#[path = "../apps/picker/scene.rs"]
mod scene;

use scene::{ICON, ICON_BYTES, Layer, SIZE, Scene, TILES, Tile};
use std::io::Write;

const PIXELS: usize = (SIZE * SIZE) as usize;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        std::process::exit(2);
    }
    let Ok(file) = std::fs::File::create(&args[1]) else { std::process::exit(2) };
    let mut out = std::io::BufWriter::new(file);
    let frames: u32 = args[2].parse().unwrap_or(0);
    let mut icons = vec![[0u8; ICON_BYTES]; 4];
    let mut given = 0;
    for (i, path) in args[3..].iter().take(4).enumerate() {
        let Ok(data) = std::fs::read(path) else { std::process::exit(2) };
        // As the C read it: into 16 KiB, so a bigger file is cut there.
        let data = &data[..data.len().min(16384)];
        if qoi::decode(data, &mut icons[i], ICON as u32) != Some((ICON as u32, ICON as u32)) {
            std::process::exit(3);
        }
        given = i + 1;
    }
    let mut apps = vec![
        Tile { name: b"MP3 Player", about: b"Plays the MP3 files on the card", version: b"1.0", icon: None, colour: 0xe9a6, letter: b'M' },
        Tile { name: b"USB Disk", about: b"The card as a USB drive", version: b"1.0", icon: None, colour: 0x2c9f, letter: b'U' },
        Tile { name: b"Slots", about: b"Swap firmware and its settings from the card", version: b"1.0", icon: None, colour: 0x8c1f, letter: b'S' },
        Tile { name: b"Tone", about: b"Two quiet tones, and the real sample rate", version: b"1.0", icon: None, colour: 0xfc60, letter: b'T' },
        Tile { name: b"HELLO", about: b"", version: b"", icon: None, colour: 0x2d2b, letter: b'H' },
        Tile { name: b"Back", about: b"To the bootloader's menu", version: b"", icon: None, colour: 0x8c71, letter: b'<' },
    ];
    for (app, icon) in apps.iter_mut().zip(&icons[..given]) {
        app.icon = Some(icon);
    }
    let count = apps.len() as u32;
    let mut layer = Box::new(Layer::new());
    let mut frame = vec![[0u8; 2]; PIXELS];
    let mut s = Scene::new(&mut layer, 7);
    let (mut selection, mut first) = (0u32, 0u32);
    for f in 0..frames {
        if f % 40 == 0 {
            if f != 0 {
                selection = (selection + 1) % count;
            }
            first = if selection < first {
                selection
            } else if selection >= first + TILES {
                selection - (TILES - 1)
            } else {
                first
            };
            s.clear_tiles();
            let mut slot = 0;
            while slot < TILES && first + slot < count {
                s.tile(slot, &apps[(first + slot) as usize], first + slot == selection);
                slot += 1;
            }
            s.more(first > 0, first + TILES < count);
            let at = format!("{} / {}", selection + 1, count - 1);
            let at = if selection + 1 < count { at.as_bytes() } else { b"" };
            s.bar(at, b"< > MOVE   SEQ PLAY START", b"", 0);
        }
        if f != 0 {
            s.step(|_, _, _, _, _| {});
        }
        s.render(0, 0, SIZE, SIZE, &mut frame);
        if out.write_all(frame.as_flattened()).is_err() {
            std::process::exit(2);
        }
    }
    // and the panel for a card with nothing to start, last
    s.clear_tiles();
    s.message(b"Nothing to start", b"NO APPS IN /MBOOT/APPS");
    s.bar(b"", b"SEQ PLAY: BACK", b"", 0);
    s.render(0, 0, SIZE, SIZE, &mut frame);
    if out.write_all(frame.as_flattened()).is_err() || out.flush().is_err() {
        std::process::exit(2);
    }
}
