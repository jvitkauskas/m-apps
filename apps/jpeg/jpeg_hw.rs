// SPDX-License-Identifier: MIT
//! The AC79's JPEG decoder (JL_JPG at 0x41900, IRQ 55), polled: baseline JPEG with three components, 4:2:0, 4:2:2 or
//! 4:4:4. It decodes a row of MCUs at a time into planar YCbCr, and hands each row to a callback before starting the
//! next. The register sequence was worked out from how JieLi's AC79 SDK drives it; this is written fresh, and handles
//! the tables its own way.
//!
//! The decoder takes:
//! - the Huffman tables as 258 16-bit words at table address 0: DC0, AC0, DC1, AC1, each as the smallest code of each
//!   length (12 lengths for DC, 16 for AC), then the index of its first symbol per length, then the symbols. Luma
//!   uses table 0, chroma table 1.
//! - the quantisation tables at table address 0x180: 64 words for luma, then 64 for chroma, in natural order, each
//!   multiplied by the AAN scale factor and shifted right 6.
//! - YPTR, UPTR and VPTR for a row of MCUs, BADDR and BCNT for the coded data, MCUCNT for the MCUs in a row, PTR_NUM
//!   for the lines in that row and YUVLINE for the width.
//!
//! CON0: bits 0-1 on, bits 4-5 the format (0 4:4:4, 1 4:2:2, 3 4:2:0), bit 7 starts a row, bit 15 says a row is done
//! and bit 14 clears it, bit 13 says the coded data ran out and bit 12 clears it. Bits 2-3 would turn on its
//! interrupts, which we leave off.
//!
//! The headers' parser ([`parse`]) touches no hardware, so it builds on a computer too (tests/jpeg_parse_host.rs).
use kit::hw::{Reg, bit, dma_address, sync_cpu};

/// What's wrong, to log and show.
pub type Error = &'static str;

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Yuv444,
    Yuv422,
    Yuv420,
}

/// What the headers held. Baseline allows tables 0 and 1 of each kind; 2 and 3 are refused.
#[derive(Clone, Copy)]
struct Tables {
    /// Zigzag order, as in the file.
    q: [[u8; 64]; 2],
    q_seen: [bool; 2],
    /// [ac][id]
    counts: [[[u8; 16]; 2]; 2],
    symbols: [[[u8; 162]; 2]; 2],
    h_seen: [[bool; 2]; 2],
    component_q: [u8; 3],
    component_dc: [u8; 3],
    component_ac: [u8; 3],
}

impl Tables {
    const EMPTY: Tables = Tables {
        q: [[0; 64]; 2],
        q_seen: [false; 2],
        counts: [[[0; 16]; 2]; 2],
        symbols: [[[0; 162]; 2]; 2],
        h_seen: [[false; 2]; 2],
        component_q: [0; 3],
        component_dc: [0; 3],
        component_ac: [0; 3],
    };
}

/// A parsed picture, with its tables, ready to decode.
pub struct Image<'a> {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    /// Lines in a row of MCUs: 16 for 4:2:0, otherwise 8.
    pub lines: u32,
    /// Width of the Cb and Cr planes.
    pub chroma_width: u32,
    /// Lines of Cb and Cr in a row of MCUs.
    pub chroma_lines: u32,
    /// The scan's coded data.
    pub entropy: &'a [u8],
    tables: Tables,
}

/// How long a decode took, in the caller's ticks.
#[derive(Clone, Copy, Default)]
pub struct Timing {
    pub setup_ticks: u32,
    pub hw_ticks: u32,
    pub callback_ticks: u32,
    pub rows: u32,
}

/// A decoded row of MCUs: the image, its first line, then lines of y (stride width), and chroma_lines of cb and cr
/// (stride chroma_width). The last row may hold fewer real lines than the buffer: first_line + lines can pass the
/// height. Return false to stop.
pub type Row<'r> = &'r mut dyn FnMut(&Image, u32, &[u8], &[u8], &[u8]) -> bool;

const JPG: Reg = Reg::at::<0x41900>();
const CON0: usize = 0;
const CON1: usize = 1;
const CON2: usize = 2;
const YUVLINE: usize = 3;
const YDCVAL: usize = 4;
const UDCVAL: usize = 5;
const VDCVAL: usize = 6;
const YPTR: usize = 7;
const UPTR: usize = 8;
const VPTR: usize = 9;
const BADDR: usize = 10;
const BCNT: usize = 11;
const MCUCNT: usize = 12;
const HQTBLADR: usize = 14;
const HQTBLDAT: usize = 15;
const PTR_NUM: usize = 16;
const REGISTERS: u32 = 19;

const CON0_ON: u32 = 3;
const CON0_START: u32 = bit(7);
const CON0_CLEAR_DATA: u32 = bit(12);
const CON0_DATA_OUT: u32 = bit(13);
const CON0_CLEAR_ROW: u32 = bit(14);
const CON0_ROW_DONE: u32 = bit(15);
/// Set by JieLi's driver with the clears before each row; their meaning isn't known.
const CON0_ROW_SETUP: u32 = bit(6) | bit(10);
const CON2_ROW: u32 = bit(3);
/// About 2 s at 96 MHz, longer at 360.
const ROW_TIMEOUT_POLLS: u32 = 20_000_000;

fn jpg(index: usize) -> Reg {
    JPG.add(4 * index as u32)
}

/// The decoder's registers, for the log.
pub fn register(index: u32) -> u32 {
    if index < REGISTERS { jpg(index as usize).read() } else { 0 }
}

const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61,
    54, 47, 55, 62, 63,
];

/// round(8192 * a(r) * a(c)), with a(0) = 1 and a(k) = sqrt(2) cos(k pi / 16): the AAN IDCT's scale factors.
const AAN: [u16; 64] = [
    8192, 11363, 10703, 9633, 8192, 6436, 4433, 2260, 11363, 15760, 14846, 13361, 11363, 8928, 6149, 3135, 10703,
    14846, 13985, 12586, 10703, 8410, 5793, 2953, 9633, 13361, 12586, 11327, 9633, 7568, 5213, 2658, 8192, 11363, 10703,
    9633, 8192, 6436, 4433, 2260, 6436, 8928, 8410, 7568, 6436, 5057, 3483, 1776, 4433, 6149, 5793, 5213, 4433, 3483,
    2399, 1223, 2260, 3135, 2953, 2658, 2260, 1776, 1223, 624,
];

fn be16(p: &[u8]) -> u32 {
    u32::from(p[0]) << 8 | u32::from(p[1])
}

/// Reads the headers, or says what's wrong.
pub fn parse(data: &[u8]) -> Result<Image<'_>, Error> {
    let mut t = Tables::EMPTY;
    let end = data.len();
    if end < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return Err("NOT A JPEG");
    }
    let mut p = 2;
    let mut frame = false;
    let (mut samp, mut width, mut height, mut restart) = (0u8, 0u32, 0u32, 0u32);
    let entropy;
    loop {
        // A marker: FF (more than one is fill), then its code.
        if p < end && data[p] != 0xff {
            return Err("NO MARKER");
        }
        while p < end && data[p] == 0xff {
            p += 1;
        }
        if p == end {
            return Err("CUT SHORT");
        }
        let marker = data[p];
        p += 1;
        // The ones with no length: the end (before any scan), and those that belong only in the coded data.
        if marker == 0xd9 {
            return Err("NO SCAN");
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            return Err("BAD MARKER");
        }
        if end - p < 2 {
            return Err("CUT SHORT");
        }
        let length = be16(&data[p..]) as usize;
        if length < 2 || length > end - p {
            return Err("BAD SEGMENT");
        }
        let s = &data[p + 2..p + length];
        p += length;
        match marker {
            0xc0 | 0xc1 => {
                if length < 17 || s[0] != 8 {
                    return Err("NOT 8 BIT");
                }
                if s[5] != 3 {
                    return Err(if s[5] == 1 { "GREYSCALE" } else { "NOT 3 COMPONENTS" });
                }
                height = be16(&s[1..]);
                width = be16(&s[3..]);
                for c in 0..3 {
                    let k = &s[6 + 3 * c..];
                    if usize::from(k[0]) != c + 1 {
                        return Err("COMPONENT IDS");
                    }
                    if k[2] > 1 {
                        return Err("QUANT TABLE ID");
                    }
                    t.component_q[c] = k[2];
                    if c != 0 && k[1] != 0x11 {
                        return Err("CHROMA SAMPLING");
                    }
                }
                samp = s[7];
                frame = true;
            }
            0xc2..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return Err(if marker == 0xc2 { "PROGRESSIVE" } else { "NOT BASELINE" });
            }
            0xdb => {
                let mut s = s;
                while !s.is_empty() {
                    let id = usize::from(s[0] & 15);
                    if s[0] >> 4 != 0 {
                        return Err("16 BIT QUANT");
                    }
                    if id > 1 || s.len() < 65 {
                        return Err("BAD QUANT TABLE");
                    }
                    t.q[id].copy_from_slice(&s[1..65]);
                    t.q_seen[id] = true;
                    s = &s[65..];
                }
            }
            0xc4 => {
                let mut s = s;
                while !s.is_empty() {
                    let (ac, id) = (usize::from(s[0] >> 4), usize::from(s[0] & 15));
                    if ac > 1 || id > 1 || s.len() < 17 {
                        return Err("BAD HUFFMAN TABLE");
                    }
                    let total: usize = s[1..17].iter().map(|&n| usize::from(n)).sum();
                    if total > if ac != 0 { 162 } else { 12 } || s.len() < 17 + total {
                        return Err("BAD HUFFMAN TABLE");
                    }
                    if ac == 0 && s[13..17].iter().any(|&n| n != 0) {
                        return Err("LONG DC CODES");
                    }
                    t.counts[ac][id].copy_from_slice(&s[1..17]);
                    t.symbols[ac][id][..total].copy_from_slice(&s[17..17 + total]);
                    t.h_seen[ac][id] = true;
                    s = &s[17 + total..];
                }
            }
            0xdd => {
                if length < 4 {
                    return Err("BAD SEGMENT");
                }
                restart = be16(s);
            }
            0xda => {
                if !frame {
                    return Err("NO FRAME");
                }
                // The component count, three components of two bytes, and three bytes of spectral selection.
                if length < 2 + 1 + 6 + 3 {
                    return Err("BAD SEGMENT");
                }
                if s[0] != 3 {
                    return Err("NOT ONE SCAN");
                }
                for c in 0..3 {
                    if usize::from(s[1 + 2 * c]) != c + 1 {
                        return Err("SCAN COMPONENTS");
                    }
                    t.component_dc[c] = s[2 + 2 * c] >> 4;
                    t.component_ac[c] = s[2 + 2 * c] & 15;
                    if t.component_dc[c] > 1 || t.component_ac[c] > 1 {
                        return Err("HUFFMAN TABLE ID");
                    }
                }
                // Baseline's spectral selection, all 64 coefficients, and no successive approximation.
                if s[7] != 0 || s[8] != 63 || s[9] != 0 {
                    return Err("NOT BASELINE");
                }
                if p == end {
                    return Err("NO DATA");
                }
                entropy = &data[p..];
                break;
            }
            _ => {}
        }
    }
    if t.component_q[1] != t.component_q[2]
        || t.component_dc[1] != t.component_dc[2]
        || t.component_ac[1] != t.component_ac[2]
    {
        return Err("CB AND CR DIFFER");
    }
    for c in 0..3 {
        let (q, dc, ac) = (usize::from(t.component_q[c]), usize::from(t.component_dc[c]), usize::from(t.component_ac[c]));
        if !t.q_seen[q] || !t.h_seen[0][dc] || !t.h_seen[1][ac] {
            return Err("MISSING TABLE");
        }
    }
    if width == 0 || height == 0 {
        return Err("NO SIZE");
    }
    let (format, lines, chroma_width, chroma_lines) = match samp {
        0x11 => (Format::Yuv444, 8, width, 8),
        0x21 => (Format::Yuv422, 8, width / 2, 8),
        0x22 => (Format::Yuv420, 16, width / 2, 8),
        _ => return Err("LUMA SAMPLING"),
    };
    let mcu_width = if format == Format::Yuv444 { 8 } else { 16 };
    if width % mcu_width != 0 {
        return Err(if format == Format::Yuv444 { "WIDTH NOT x8" } else { "WIDTH NOT x16" });
    }
    if restart != 0 && restart % (width / mcu_width) != 0 {
        return Err("RESTART NOT PER ROW");
    }
    // Not handled yet, so an image has none: restart is its interval, in MCUs, or 0.
    if restart != 0 {
        return Err("RESTART MARKERS");
    }
    Ok(Image { width, height, format, lines, chroma_width, chroma_lines, entropy, tables: t })
}

impl Image<'_> {
    /// Bytes the row buffers need.
    pub fn work_size(&self) -> u32 {
        self.width * self.lines + 2 * self.chroma_width * self.chroma_lines
    }
}

/// One table in the decoder's layout: the smallest code of each length, the index of its first symbol, then the
/// symbols. The code is shifted on only while it fits, which is how JieLi's driver builds it.
fn huffman(out: &mut [u8], counts: &[u8; 16], symbols: &[u8; 162], ac: bool) {
    let n = if ac { 16 } else { 12 };
    out.fill(0);
    let (mut code, mut index) = (0u32, 0usize);
    for i in 1..=n {
        out[2 * (i - 1)] = code as u8;
        out[2 * (i - 1) + 1] = (code >> 8) as u8;
        out[2 * n + i - 1] = index as u8;
        let count = counts[i - 1];
        if count == 0 {
            code <<= 1;
        } else {
            code += u32::from(count);
            if code & 1 << (i + 1) == 0 {
                code <<= 1;
            }
            index += usize::from(count);
        }
    }
    out[3 * n..3 * n + index].copy_from_slice(&symbols[..index]);
}

fn load_tables(t: &Tables) {
    let mut h = [0u8; 516];
    let (dc, ac, q) = (&t.component_dc, &t.component_ac, &t.component_q);
    huffman(&mut h[..48], &t.counts[0][usize::from(dc[0])], &t.symbols[0][usize::from(dc[0])], false);
    huffman(&mut h[48..258], &t.counts[1][usize::from(ac[0])], &t.symbols[1][usize::from(ac[0])], true);
    huffman(&mut h[258..306], &t.counts[0][usize::from(dc[1])], &t.symbols[0][usize::from(dc[1])], false);
    huffman(&mut h[306..], &t.counts[1][usize::from(ac[1])], &t.symbols[1][usize::from(ac[1])], true);
    jpg(HQTBLADR).write(0);
    sync_cpu();
    for pair in h.chunks_exact(2) {
        jpg(HQTBLDAT).write(u32::from(pair[0]) | u32::from(pair[1]) << 8);
    }
    jpg(HQTBLADR).write(0x180);
    sync_cpu();
    for table in 0..2 {
        let q = &t.q[usize::from(q[table])];
        let mut natural = [0i16; 64];
        for i in 0..64 {
            let z = usize::from(ZIGZAG[i]);
            natural[z] = ((u32::from(q[i]) * u32::from(AAN[z])) >> 6) as i16;
        }
        for &v in &natural {
            jpg(HQTBLDAT).write(i32::from(v) as u32);
        }
        sync_cpu();
    }
}

/// Where the decoder writes the next row of MCUs.
///
/// # Safety
/// The three planes are the decoder's to write, with nothing reading them, until it says the row is done.
unsafe fn point(y: &mut [u8], cb: &mut [u8], cr: &mut [u8], mcus: u32) {
    jpg(YPTR).write(dma_address(y.as_mut_ptr()));
    jpg(UPTR).write(dma_address(cb.as_mut_ptr()));
    jpg(VPTR).write(dma_address(cr.as_mut_ptr()));
    jpg(MCUCNT).write(mcus);
}

/// The decoder has written a row into memory the compiler didn't see written: what's read from it now has to be read
/// again, as C's volatile loop got without asking. No instruction.
fn decoded() {
    #[cfg(target_arch = "pi32v2")]
    // Safety: an empty instruction, there only as a fence for the compiler.
    unsafe {
        core::arch::asm!("", options(nostack, preserves_flags))
    };
}

/// Decodes a parsed image. `work` must be 4-byte aligned and [`Image::work_size`] long. `ticks_now` is for timing,
/// and may be None, as may `row`. Says what went wrong, if anything.
pub fn decode(
    image: &Image,
    work: &mut [u8],
    mut row: Option<Row>,
    ticks_now: Option<fn() -> u32>,
    timing: &mut Timing,
) -> Result<(), Error> {
    *timing = Timing::default();
    let now = || ticks_now.map_or(0, |f| f());
    let t0 = now();
    let (y, rest) = work.split_at_mut((image.width * image.lines) as usize);
    let (cb, rest) = rest.split_at_mut((image.chroma_width * image.chroma_lines) as usize);
    let cr = &mut rest[..(image.chroma_width * image.chroma_lines) as usize];
    let mcus = image.width / if image.format == Format::Yuv444 { 8 } else { 16 };
    let rows = image.height.div_ceil(image.lines);
    let format = match image.format {
        Format::Yuv444 => 0,
        Format::Yuv422 => 1,
        Format::Yuv420 => 3,
    };
    let con0 = jpg(CON0);

    con0.write(0);
    jpg(CON1).write(0);
    jpg(CON2).write(0);
    sync_cpu();
    // Safety: the planes are borrowed here until the decode ends, and read only between rows, once one is done.
    unsafe { point(y, cb, cr, mcus) };
    load_tables(&image.tables);
    con0.set(CON0_ON);
    jpg(YDCVAL).write(0);
    jpg(UDCVAL).write(0);
    jpg(VDCVAL).write(0);
    sync_cpu();
    jpg(BADDR).write(dma_address(image.entropy.as_ptr().cast_mut()));
    jpg(BCNT).write(image.entropy.len() as u32);
    jpg(PTR_NUM).write(image.lines);
    jpg(MCUCNT).write(mcus);
    con0.set(CON0_ROW_SETUP | CON0_CLEAR_DATA | CON0_CLEAR_ROW);
    con0.set(format << 4);
    sync_cpu();
    jpg(YUVLINE).write(image.width);
    jpg(CON2).set(CON2_ROW);
    sync_cpu();
    if ticks_now.is_some() {
        timing.setup_ticks = now().wrapping_sub(t0);
    }
    con0.set(CON0_START);

    let mut error = None;
    for r in 0..rows {
        let started = now();
        let mut waited = 0u32;
        loop {
            let con = con0.read();
            if con & CON0_DATA_OUT != 0 {
                con0.set(CON0_CLEAR_DATA);
            }
            if con & CON0_ROW_DONE != 0 {
                con0.set(CON0_CLEAR_ROW);
                break;
            }
            waited += 1;
            if waited > ROW_TIMEOUT_POLLS {
                error = Some("DECODER TIMED OUT");
                break;
            }
        }
        if error.is_some() {
            break;
        }
        decoded();
        let done = now();
        timing.hw_ticks = timing.hw_ticks.wrapping_add(done.wrapping_sub(started));
        timing.rows += 1;
        if let Some(row) = row.as_mut()
            && !row(image, r * image.lines, y, cb, cr)
        {
            error = Some("STOPPED");
        }
        if ticks_now.is_some() {
            timing.callback_ticks = timing.callback_ticks.wrapping_add(now().wrapping_sub(done));
        }
        if error.is_some() || r + 1 == rows {
            break;
        }
        // Safety: as above; the callback has finished with the planes.
        unsafe { point(y, cb, cr, mcus) };
        con0.set(CON0_CLEAR_DATA);
        con0.set(bit(10));
        con0.set(CON0_CLEAR_ROW);
        jpg(CON2).set(CON2_ROW);
        sync_cpu();
        con0.set(CON0_START);
    }
    con0.write(0);
    jpg(BCNT).write(0);
    error.map_or(Ok(()), Err)
}
