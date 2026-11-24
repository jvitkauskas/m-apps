// SPDX-License-Identifier: MIT
//! Tries the AC79's JPEG decoder: decodes the pictures in images.rs, shows them, and logs how long the decoder took.
//! One picture's planes are left in `dump` (picture 0, or the one built with jpeg_CFG='--cfg jpeg_dump="n"', or none
//! with "none"), for checking on a computer.
#![no_std]
#![cfg_attr(target_arch = "pi32v2", feature(asm_experimental_arch))]
use kit::display;
use kit::hw::Reg;
use kit::log::hex;
use kit::platform::hal_delay_us;
use kit::{APP_BOARD, Aligned, Global, app, log};

mod images;
mod jpeg_hw;

use images::IMAGES;
use jpeg_hw::{Format, Image, Timing};

const WHITE: u16 = 0xffff;
const GREEN: u16 = 0x07e0;
const RED: u16 = 0xf800;
const BLACK: u16 = 0x0000;
const TIMER_CON: Reg = Reg::at::<0x10800>();
const TIMER_COUNT: Reg = Reg::at::<0x10804>();
const TIMER_PERIOD: Reg = Reg::at::<0x10808>();
const TICKS_PER_US: u32 = 24;
const SHOW_MS: u32 = 2500;
const REPEATS: u32 = 10;
const DUMP: usize = if cfg!(jpeg_dump = "none") {
    usize::MAX
} else if cfg!(jpeg_dump = "1") {
    1
} else if cfg!(jpeg_dump = "2") {
    2
} else if cfg!(jpeg_dump = "3") {
    3
} else if cfg!(jpeg_dump = "4") {
    4
} else {
    0
};
const DUMP_SIZE: usize = 240 * 240 * 3;

static WORK: Global<Aligned<{ 480 * 16 * 2 }>> = Global::new(Aligned([0; 480 * 16 * 2]));
/// Named as C named it, to find in the map.
#[unsafe(export_name = "dump")]
static DUMP_PLANES: Global<Aligned<DUMP_SIZE>> = Global::new(Aligned([0; DUMP_SIZE]));

struct View {
    index: usize,
    scale: u32,
    out_width: u32,
    x0: u32,
    y0: u32,
    show: bool,
}

kit::app!(main);

fn timer_start() {
    if TIMER_CON.read() & 0xf == (2 << 2) | 1 {
        return;
    }
    TIMER_CON.write(0);
    TIMER_PERIOD.write(0xffff_ffff);
    TIMER_COUNT.write(0);
    TIMER_CON.write((2 << 2) | 1 | (1 << 14));
}

fn ticks() -> u32 {
    TIMER_COUNT.read()
}

fn us(t: u32) -> u32 {
    t / TICKS_PER_US
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// JFIF's YCbCr to RGB565, big-endian for display::pixels().
fn rgb(out: &mut [u8], y: i32, cb: i32, cr: i32) {
    let (d, e) = (cb - 128, cr - 128);
    let r = clamp(y + ((91881 * e) >> 16));
    let g = clamp(y - ((22554 * d + 46802 * e) >> 16));
    let b = clamp(y + ((116130 * d) >> 16));
    let c = u16::from(r >> 3) << 11 | u16::from(g >> 2) << 5 | u16::from(b >> 3);
    out[..2].copy_from_slice(&c.to_be_bytes());
}

/// A decoded row: into `dump` if it's the picture kept there, and onto the screen.
fn row(v: &View, dump: &mut [u8; DUMP_SIZE], im: &Image, first: u32, y: &[u8], cb: &[u8], cr: &[u8]) -> bool {
    let (width, height) = (im.width as usize, im.height as usize);
    let (lines, chroma_width, chroma_lines) = (im.lines as usize, im.chroma_width as usize, im.chroma_lines as usize);
    let first = first as usize;
    if v.index == DUMP && width * height * 3 <= DUMP_SIZE {
        for l in (0..lines).take_while(|l| first + l < height) {
            let at = (first + l) * width;
            dump[at..at + width].copy_from_slice(&y[l * width..(l + 1) * width]);
        }
        let chroma_height = if im.format == Format::Yuv420 { height.div_ceil(2) } else { height };
        let chroma_first = first / lines * chroma_lines;
        let planes = &mut dump[width * height..];
        for l in (0..chroma_lines).take_while(|l| chroma_first + l < chroma_height) {
            let at = (chroma_first + l) * chroma_width;
            let from = l * chroma_width..(l + 1) * chroma_width;
            planes[at..at + chroma_width].copy_from_slice(&cb[from.clone()]);
            let at = at + chroma_height * chroma_width;
            planes[at..at + chroma_width].copy_from_slice(&cr[from]);
        }
    }
    if !v.show {
        return true;
    }
    let mut line = [0u8; 240 * 2];
    let scale = v.scale as usize;
    for l in (0..lines).take_while(|l| first + l < height) {
        let sy = first + l;
        if sy % scale != 0 {
            continue;
        }
        let yl = &y[l * width..];
        let cl = if im.format == Format::Yuv420 { l / 2 } else { l };
        let (cbl, crl) = (&cb[cl * chroma_width..], &cr[cl * chroma_width..]);
        for ox in 0..v.out_width as usize {
            let sx = ox * scale;
            let cx = if im.format == Format::Yuv444 { sx } else { sx / 2 };
            rgb(&mut line[2 * ox..], i32::from(yl[sx]), i32::from(cbl[cx]), i32::from(crl[cx]));
        }
        display::pixels(v.x0, v.y0 + (sy / scale) as u32, v.out_width, 1, &line[..2 * v.out_width as usize]);
    }
    true
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Yuv420 => "4:2:0",
        Format::Yuv422 => "4:2:2",
        Format::Yuv444 => "4:4:4",
    }
}

fn wait_ms(ms: u32) {
    for i in 0..ms {
        hal_delay_us(1000);
        if i % 500 == 0 {
            app::feed();
        }
    }
}

/// "NNNN.NN" milliseconds from microseconds, for the screen. Returns the bytes written.
fn put_ms(out: &mut [u8], micro: u32) -> usize {
    let (mut ms, frac) = (micro / 1000, micro % 1000 / 10);
    let mut tmp = [0u8; 8];
    let mut n = 0;
    loop {
        tmp[n] = b'0' + (ms % 10) as u8;
        n += 1;
        ms /= 10;
        if ms == 0 {
            break;
        }
    }
    let mut at = 0;
    while n > 0 {
        n -= 1;
        out[at] = tmp[n];
        at += 1;
    }
    out[at..at + 3].copy_from_slice(&[b'.', b'0' + (frac / 10) as u8, b'0' + (frac % 10) as u8]);
    at + 3
}

fn main(lsb_mhz: u32) -> i32 {
    timer_start();
    let screen = display::init(APP_BOARD);
    log!("jpeg test, LSB ", lsb_mhz, " MHz, board ", APP_BOARD, ", screen ", if screen { "yes" } else { "no" });
    log!("registers before: con0 ", hex(jpeg_hw::register(0)), " con1 ", hex(jpeg_hw::register(1)), " con2 ",
        hex(jpeg_hw::register(2)), " mcucnt ", hex(jpeg_hw::register(12)), " ptr_num ", hex(jpeg_hw::register(16)));
    // Do the registers hold what's written?
    Reg::at::<{ 0x41900 + 4 * 3 }>().write(0x1f0);
    Reg::at::<{ 0x41900 + 4 * 16 }>().write(0x10);
    log!("yuvline wrote 1f0 read ", hex(jpeg_hw::register(3)), ", ptr_num wrote 10 read ", hex(jpeg_hw::register(16)));

    // Safety: the only uses of them, here.
    let (work, dump) = unsafe { (&mut WORK.get().0, &mut DUMP_PLANES.get().0) };
    let mut hw_us = [0u32; IMAGES.len()];
    let mut result: [Option<&str>; IMAGES.len()] = [None; IMAGES.len()];
    let mut failures = 0;
    for (i, picture) in IMAGES.iter().enumerate() {
        let im = match jpeg_hw::parse(picture.data) {
            Ok(im) => im,
            Err(e) => {
                result[i] = Some(e);
                log!(picture.name, ": parse: ", e);
                failures += 1;
                continue;
            }
        };
        log!(picture.name, ": ", im.width, "x", im.height, " ", format_name(im.format), ", ", im.entropy.len(),
            " bytes coded, work ", im.work_size());
        let mut v = View { index: i, scale: 1, out_width: 0, x0: 0, y0: 0, show: screen };
        while im.width / v.scale > 240 || im.height / v.scale > 240 {
            v.scale *= 2;
        }
        v.out_width = im.width / v.scale;
        v.x0 = (240 - v.out_width) / 2;
        v.y0 = (240 - im.height / v.scale) / 2;
        if screen {
            display::rect(0, 0, 240, 240, BLACK);
        }
        let mut t = Timing::default();
        let started = ticks();
        let mut draw = |im: &Image, first, y: &[u8], cb: &[u8], cr: &[u8]| row(&v, dump, im, first, y, cb, cr);
        let shown = jpeg_hw::decode(&im, work, Some(&mut draw), Some(ticks), &mut t);
        let total = ticks().wrapping_sub(started);
        result[i] = shown.err();
        log!(picture.name, ": shown: ", result[i].unwrap_or("ok"), ", ", t.rows, " rows, setup ", us(t.setup_ticks),
            " us, decoder ", us(t.hw_ticks), " us, drawing ", us(t.callback_ticks), " us, all ", us(total), " us");
        if result[i].is_some() {
            log!("  con0 ", hex(jpeg_hw::register(0)), " con2 ", hex(jpeg_hw::register(2)), " bcnt ",
                hex(jpeg_hw::register(11)), " mcucnt ", hex(jpeg_hw::register(12)), " baddr ", hex(jpeg_hw::register(10)));
            failures += 1;
            continue;
        }
        if screen {
            wait_ms(SHOW_MS);
        }
        // The decoder alone, without drawing.
        let (mut best, mut sum) = (u32::MAX, 0u32);
        for k in 0..REPEATS {
            let s = ticks();
            let e = jpeg_hw::decode(&im, work, None, None, &mut Timing::default());
            let d = ticks().wrapping_sub(s);
            if let Err(e) = e {
                log!(picture.name, ": repeat ", k, ": ", e);
                result[i] = Some(e);
                failures += 1;
                break;
            }
            sum = sum.wrapping_add(d);
            best = best.min(d);
        }
        hw_us[i] = us(best);
        log!(picture.name, ": decode only: best ", us(best), " us, mean ", us(sum / REPEATS), " us of ", REPEATS);
        app::feed();
    }
    if let Some(picture) = IMAGES.get(DUMP)
        && let Ok(im) = jpeg_hw::parse(picture.data)
    {
        log!("dump: ", picture.name, " at ", hex(dump.as_ptr() as usize as u32), ", ", im.width, "x", im.height, " ",
            format_name(im.format));
    }
    if screen {
        for r in 0..15 {
            display::line(r, WHITE, b"");
        }
        display::line(0, GREEN, b"JPEG DECODER");
        display::line(1, WHITE, b"DECODE ONLY, MS");
        for (i, picture) in IMAGES.iter().enumerate() {
            let mut text = [0u8; 21];
            let name = &picture.name.as_bytes()[..picture.name.len().min(13)];
            let mut n = name.len();
            text[..n].copy_from_slice(name);
            text[n] = b' ';
            n += 1;
            if result[i].is_some() {
                text[n..n + 4].copy_from_slice(b"FAIL");
                n += 4;
            } else {
                n += put_ms(&mut text[n..], hw_us[i]);
            }
            display::line(3 + i as u32, if result[i].is_some() { RED } else { WHITE }, &text[..n]);
        }
        display::line(13, if failures != 0 { RED } else { GREEN }, if failures != 0 { b"SOME FAILED" } else { b"ALL DECODED" });
        wait_ms(8000);
    }
    if failures != 0 { 1 } else { 0 }
}
