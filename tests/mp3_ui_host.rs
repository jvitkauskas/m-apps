// SPDX-License-Identifier: MIT
//! Draws the MP3 player's screens on a computer, from the same code, into PPM files: mp3_ui_host OUT_DIR. Also checks
//! the analyser's levels on test tones, and that what's sent matches what's drawn.
#[path = "../apps/mp3/spectrum.rs"]
mod spectrum;
#[path = "../apps/mp3/ui.rs"]
mod ui;

use kit::canvas::Plain;
use kit::pixel;
use std::f64::consts::PI;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use ui::{Model, NAME_BYTES, Names, PAUSED, PLAYING, STOPPED, Ui};

static SCREEN: Mutex<[[u16; 240]; 240]> = Mutex::new([[0; 240]; 240]);
static SENT_BYTES: AtomicU32 = AtomicU32::new(0);

/// What display::pixels() would get.
fn sent(x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
    if x + w > 240 || y + h > 240 || w == 0 || h == 0 {
        eprintln!("display_pixels out of the screen: {x} {y} {w} {h}");
        std::process::exit(2);
    }
    let mut screen = SCREEN.lock().unwrap();
    let mut at = 0;
    for r in 0..h as usize {
        for c in 0..w as usize {
            screen[y as usize + r][x as usize + c] = u16::from_be_bytes([data[at], data[at + 1]]);
            at += 2;
        }
    }
    SENT_BYTES.fetch_add(w * h * 2, Ordering::Relaxed);
}

fn save(dir: &str, name: &str) {
    let mut out = b"P6 240 240 255\n".to_vec();
    for row in SCREEN.lock().unwrap().iter() {
        for &c in row {
            out.extend_from_slice(&[
                ((c >> 11) << 3 | (c >> 13)) as u8,
                (((c >> 5) & 63) << 2 | ((c >> 9) & 3)) as u8,
                ((c & 31) << 3 | ((c & 31) >> 2)) as u8,
            ]);
        }
    }
    if std::fs::write(format!("{dir}/{name}.ppm"), out).is_err() {
        std::process::exit(2);
    }
    println!("{name}: {} bytes sent since the last", SENT_BYTES.swap(0, Ordering::Relaxed));
}

const NAMES: [&str; 8] = [
    "Daft Punk - Around the World",
    "Aphex Twin - Windowlicker",
    "Boards of Canada - Roygbiv",
    "The Prodigy - Firestarter (Extended Mix)",
    "Moby - Porcelain",
    "Air - La Femme d'Argent",
    "Fatboy Slim - Right Here, Right Now",
    "Underworld - Born Slippy .NUXX",
];

struct Tracks;

impl Names for Tracks {
    fn name(&self, track: u32, out: &mut [u8; NAME_BYTES]) -> usize {
        let name = NAMES[track as usize % 8].as_bytes();
        out[..name.len()].copy_from_slice(name);
        name.len()
    }
}

fn tone(pcm: &mut [i16; 1152 * 2], hz: f64, amplitude: f64) {
    for i in 0..1152 {
        let v = (amplitude * 32767.0 * (2.0 * PI * hz * i as f64 / 44100.0).sin()) as i16;
        pcm[2 * i] = v;
        pcm[2 * i + 1] = v;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        std::process::exit(2);
    }
    let dir = &args[1];
    let mut levels = [0u8; spectrum::BANDS];
    let mut pcm = [0i16; 1152 * 2];

    // The analyser on tones: a full-scale tone fills its band, a quiet one shows low, silence shows nothing.
    for hz in [100.0, 1000.0, 10000.0] {
        let mut a = 1.0;
        while a > 0.0005 {
            tone(&mut pcm, hz, a);
            spectrum::run(&pcm, 2, 1152, &mut levels);
            let (mut most, mut band) = (0, 0);
            for (b, &l) in levels.iter().enumerate() {
                if l > most {
                    (most, band) = (l, b);
                }
            }
            println!("{hz:5.0} Hz at {a:6.4}: band {band:2} level {most:2}");
            if a == 1.0 && most < spectrum::TOP - 1 {
                eprintln!("a full-scale tone shows only {most}");
                std::process::exit(1);
            }
            a /= 10.0;
        }
    }
    pcm.fill(0);
    spectrum::run(&pcm, 2, 1152, &mut levels);
    for (b, &l) in levels.iter().enumerate() {
        if l != 0 {
            eprintln!("silence shows {l} in band {b}");
            std::process::exit(1);
        }
    }

    pixel::watch(Some(sent));
    // Safety: the only claim.
    let pix = unsafe { pixel::claim() };
    let mut ui = Ui::new();
    let tracks = Tracks;
    let mut m = Model {
        mode: STOPPED,
        elapsed: 0,
        total: 0,
        kbps: 0,
        hz: 0,
        channels: 0,
        count: 8,
        selected: 0,
        playing: -1,
        names: &tracks,
        note: None,
        keys: 0,
        levels: None,
    };
    ui.start(pix, 1, &mut Plain);
    save(dir, "0-start");
    ui.update(pix, &m, 0, &mut Plain);
    save(dir, "1-stopped");
    m.selected = 6;
    ui.update(pix, &m, 20, &mut Plain);
    save(dir, "2-selected-6");

    // Playing, with a mix of tones for the bars.
    m.mode = PLAYING;
    m.playing = 3;
    m.selected = 3;
    m.elapsed = 83;
    m.total = 245;
    m.kbps = 192;
    m.hz = 44100;
    m.channels = 2;
    for i in 0..1152 {
        let t = i as f64 / 44100.0;
        let v = 0.5 * (2.0 * PI * 80.0 * t).sin()
            + 0.25 * (2.0 * PI * 440.0 * t).sin()
            + 0.12 * (2.0 * PI * 2500.0 * t).sin()
            + 0.05 * (2.0 * PI * 9000.0 * t).sin();
        pcm[2 * i] = (v * 32767.0) as i16;
        pcm[2 * i + 1] = pcm[2 * i];
    }
    spectrum::run(&pcm, 2, 1152, &mut levels);
    m.levels = Some(levels);
    for _ in 0..10 {
        ui.update(pix, &m, 30, &mut Plain);
    }
    save(dir, "3-playing");
    m.keys = 2;
    ui.update(pix, &m, 30, &mut Plain);
    save(dir, "4-next-held");
    m.keys = 0;
    m.mode = PAUSED;
    for _ in 0..8 {
        ui.update(pix, &m, 100, &mut Plain);
    }
    save(dir, "5-paused");

    let empty = Model {
        mode: STOPPED,
        elapsed: 0,
        total: 0,
        kbps: 0,
        hz: 0,
        channels: 0,
        count: 0,
        selected: 0,
        playing: -1,
        names: &tracks,
        note: Some(b"NO MP3 FILES"),
        keys: 0,
        levels: None,
    };
    ui.start(pix, 2, &mut Plain);
    ui.update(pix, &empty, 0, &mut Plain);
    save(dir, "6-no-files-smk37");

    // What's on the screen has to be what was drawn: draw it all again from the start, and compare.
    ui.start(pix, 1, &mut Plain);
    m.mode = PLAYING;
    m.levels = Some(levels);
    // Sent a little at a time, as while playing, with the track changing part way.
    pix.budget(4096);
    for k in 0..50 {
        m.elapsed = 83 + k / 10;
        if k == 20 {
            m.playing = 5;
            m.selected = 5;
        }
        ui.update(pix, &m, 37, &mut Plain);
    }
    for _ in 0..40 {
        ui.update(pix, &m, 0, &mut Plain);
    }
    pix.budget(0);
    let before = *SCREEN.lock().unwrap();
    *SCREEN.lock().unwrap() = [[0; 240]; 240];
    ui.start(pix, 1, &mut Plain);
    let same = Model { ..m };
    ui.update(pix, &same, 0, &mut Plain);
    // Animations differ (bars and scrolling), so compare the parts that don't move: the list and below.
    let screen = SCREEN.lock().unwrap();
    for y in 2 * 66..240 {
        if before[y] != screen[y] {
            eprintln!("row {y} differs after updates");
            std::process::exit(1);
        }
    }
    println!("OK");
    std::io::stdout().flush().unwrap();
}
