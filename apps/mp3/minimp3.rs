// SPDX-License-Identifier: CC0-1.0
//! minimp3 (https://github.com/lieff/minimp3), ported to Rust from its minimp3.h for the MP3 player: layer 3 only
//! and without SIMD, as the player built it in C (MINIMP3_ONLY_MP3, MINIMP3_NO_SIMD), with 16-bit samples out. The
//! functions keep minimp3's names, and every floating-point operation keeps its order, so that the samples are the
//! C's bit for bit (tests/mp3_decoder_test.py compares them). Its comments are kept, and what's added says why the
//! Rust differs.
//!
//! The port is dedicated to the public domain as minimp3 is. minimp3's notice:
//!
//! > https://github.com/lieff/minimp3
//! > To the extent possible under law, the author(s) have dedicated all copyright and related and neighboring rights
//! > to this software to the public domain worldwide.
//! > This software is distributed without any warranty.
//! > See <http://creativecommons.org/publicdomain/zero/1.0/>.

pub const MAX_SAMPLES_PER_FRAME: usize = 1152 * 2;

#[derive(Clone, Copy, Default)]
pub struct FrameInfo {
    pub frame_bytes: i32,
    pub frame_offset: i32,
    pub channels: i32,
    pub hz: i32,
    pub layer: i32,
    pub bitrate_kbps: i32,
}

/// mp3dec_t: what a stream keeps from one frame to the next.
pub struct Mp3Dec {
    mdct_overlap: [[f32; 9 * 32]; 2],
    qmf_state: [f32; 15 * 2 * 32],
    reserv: i32,
    free_format_bytes: i32,
    header: [u8; 4],
    reserv_buf: [u8; 511],
}

impl Mp3Dec {
    pub const fn new() -> Mp3Dec {
        Mp3Dec {
            mdct_overlap: [[0.0; 9 * 32]; 2],
            qmf_state: [0.0; 15 * 2 * 32],
            reserv: 0,
            free_format_bytes: 0,
            header: [0; 4],
            reserv_buf: [0; 511],
        }
    }

    /// memset(dec, 0, sizeof(mp3dec_t)), in place: a new one would be made on the stack first.
    pub fn clear(&mut self) {
        self.mdct_overlap.as_flattened_mut().fill(0.0);
        self.qmf_state.fill(0.0);
        self.reserv = 0;
        self.free_format_bytes = 0;
        self.header = [0; 4];
        self.reserv_buf.fill(0);
    }
}

impl Default for Mp3Dec {
    fn default() -> Mp3Dec {
        Mp3Dec::new()
    }
}

const MAX_FREE_FORMAT_FRAME_SIZE: i32 = 2304; /* more than ISO spec's */
const MAX_FRAME_SYNC_MATCHES: i32 = 10;

const MAX_L3_FRAME_PAYLOAD_BYTES: usize = MAX_FREE_FORMAT_FRAME_SIZE as usize; /* MUST be >= 320000/8/32000*1152 = 1440 */

const MAX_BITRESERVOIR_BYTES: i32 = 511;
const SHORT_BLOCK_TYPE: u8 = 2;
const STOP_BLOCK_TYPE: u8 = 3;
const HDR_SIZE: i32 = 4;

type Header = [u8; 4];

fn hdr_is_mono(h: &Header) -> bool {
    h[3] & 0xc0 == 0xc0
}
fn hdr_is_ms_stereo(h: &Header) -> bool {
    h[3] & 0xe0 == 0x60
}
fn hdr_is_free_format(h: &Header) -> bool {
    h[2] & 0xf0 == 0
}
fn hdr_is_crc(h: &Header) -> bool {
    h[1] & 1 == 0
}
fn hdr_test_padding(h: &Header) -> bool {
    h[2] & 0x2 != 0
}
fn hdr_test_mpeg1(h: &Header) -> bool {
    h[1] & 0x8 != 0
}
fn hdr_test_not_mpeg25(h: &Header) -> bool {
    h[1] & 0x10 != 0
}
fn hdr_test_i_stereo(h: &Header) -> bool {
    h[3] & 0x10 != 0
}
fn hdr_test_ms_stereo(h: &Header) -> bool {
    h[3] & 0x20 != 0
}
fn hdr_get_layer(h: &Header) -> usize {
    usize::from(h[1] >> 1 & 3)
}
fn hdr_get_bitrate(h: &Header) -> usize {
    usize::from(h[2] >> 4)
}
fn hdr_get_sample_rate(h: &Header) -> usize {
    usize::from(h[2] >> 2 & 3)
}
fn hdr_get_my_sample_rate(h: &Header) -> usize {
    hdr_get_sample_rate(h) + usize::from((h[1] >> 3 & 1) + (h[1] >> 4 & 1)) * 3
}
fn hdr_is_frame_576(h: &Header) -> bool {
    h[1] & 14 == 2
}
fn hdr_is_layer_1(h: &Header) -> bool {
    h[1] & 6 == 6
}

const BITS_DEQUANTIZER_OUT: i32 = -1;
const MAX_SCF: i32 = 255 + BITS_DEQUANTIZER_OUT * 4 - 210;
const MAX_SCFI: i32 = (MAX_SCF + 3) & !3;

/// The 4 bytes of a header at `i`, which the callers have checked are there, as minimp3's do.
fn at(mp3: &[u8], i: i32) -> &Header {
    mp3[i as usize..].first_chunk().unwrap()
}

/// bs_t: bits from `buf`, which the Rust keeps beside the position, not in a struct of its own buffer.
struct Bs<'a> {
    buf: &'a [u8],
    pos: i32,
    limit: i32,
}

impl<'a> Bs<'a> {
    fn new(data: &'a [u8], bytes: i32) -> Bs<'a> {
        Bs { buf: data, pos: 0, limit: bytes * 8 }
    }
}

#[derive(Clone, Copy)]
struct GrInfo {
    sfbtab: &'static [u8],
    part_23_length: u16,
    big_values: u16,
    scalefac_compress: u16,
    global_gain: u8,
    block_type: u8,
    mixed_block_flag: u8,
    n_long_sfb: u8,
    n_short_sfb: u8,
    table_select: [u8; 3],
    region_count: [u8; 3],
    subblock_gain: [u8; 3],
    preflag: u8,
    scalefac_scale: u8,
    count1_table: u8,
    scfsi: u8,
}

impl GrInfo {
    const EMPTY: GrInfo = GrInfo {
        sfbtab: &[],
        part_23_length: 0,
        big_values: 0,
        scalefac_compress: 0,
        global_gain: 0,
        block_type: 0,
        mixed_block_flag: 0,
        n_long_sfb: 0,
        n_short_sfb: 0,
        table_select: [0; 3],
        region_count: [0; 3],
        subblock_gain: [0; 3],
        preflag: 0,
        scalefac_scale: 0,
        count1_table: 0,
        scfsi: 0,
    };
}

/// mp3dec_scratch_t, but for its bit reader and main data, which the bit reader borrows: decode_frame() keeps those
/// apart, and the granules' side info too, which is read while these are written.
struct Scratch {
    /// grbuf[2][576], then scf[40], as one, as C's struct has them. For a mixed block at 8 kHz (MPEG 2.5), minimp3
    /// reorders the short blocks from sample 72 but its table's long bands end at 48, so L3_reorder() runs 24 samples
    /// past the channel's 576: into the other channel's for the first, and into scf for the second. In C that's
    /// outside the array but inside the struct; here it stays inside this one, with the C's result.
    grbuf_scf: [f32; 2 * 576 + 40],
    syn: [f32; (18 + 15) * 2 * 32],
    ist_pos: [[u8; 39]; 2],
}

/// Scratch's grbuf and scf.
fn grbuf_scf(both: &mut [f32; 2 * 576 + 40]) -> (&mut [f32; 2 * 576], &mut [f32; 40]) {
    let (grbuf, scf) = both.split_first_chunk_mut::<{ 2 * 576 }>().unwrap();
    (grbuf, scf.first_chunk_mut().unwrap())
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn get_bits(bs: &mut Bs, n: i32) -> u32 {
    let s = (bs.pos & 7) as u32;
    let mut shl = n + s as i32;
    let mut p = (bs.pos >> 3) as usize;
    bs.pos += n;
    if bs.pos > bs.limit {
        return 0;
    }
    let mut next = u32::from(bs.buf[p]) & (255 >> s);
    p += 1;
    let mut cache = 0;
    loop {
        shl -= 8;
        if shl <= 0 {
            break;
        }
        cache |= next << shl;
        next = u32::from(bs.buf[p]);
        p += 1;
    }
    cache | (next >> -shl)
}

fn hdr_valid(h: &Header) -> bool {
    h[0] == 0xff
        && ((h[1] & 0xf0) == 0xf0 || (h[1] & 0xfe) == 0xe2)
        && (hdr_get_layer(h) != 0)
        && (hdr_get_bitrate(h) != 15)
        && (hdr_get_sample_rate(h) != 3)
}

fn hdr_compare(h1: &Header, h2: &Header) -> bool {
    hdr_valid(h2)
        && ((h1[1] ^ h2[1]) & 0xfe) == 0
        && ((h1[2] ^ h2[2]) & 0x0c) == 0
        && !(hdr_is_free_format(h1) ^ hdr_is_free_format(h2))
}

fn hdr_bitrate_kbps(h: &Header) -> u32 {
    #[rustfmt::skip]
    static HALFRATE: [[[u8; 15]; 3]; 2] = [
        [
            [0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80],
            [0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80],
            [0, 16, 24, 28, 32, 40, 48, 56, 64, 72, 80, 88, 96, 112, 128],
        ],
        [
            [0, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160],
            [0, 16, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192],
            [0, 16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224],
        ],
    ];
    2 * u32::from(HALFRATE[usize::from(hdr_test_mpeg1(h))][hdr_get_layer(h) - 1][hdr_get_bitrate(h)])
}

fn hdr_sample_rate_hz(h: &Header) -> u32 {
    #[rustfmt::skip]
    static G_HZ: [u32; 3] = [44100, 48000, 32000];
    G_HZ[hdr_get_sample_rate(h)] >> u32::from(!hdr_test_mpeg1(h)) >> u32::from(!hdr_test_not_mpeg25(h))
}

fn hdr_frame_samples(h: &Header) -> u32 {
    if hdr_is_layer_1(h) { 384 } else { 1152 >> u32::from(hdr_is_frame_576(h)) }
}

fn hdr_frame_bytes(h: &Header, free_format_size: i32) -> i32 {
    let mut frame_bytes = (hdr_frame_samples(h) * hdr_bitrate_kbps(h) * 125 / hdr_sample_rate_hz(h)) as i32;
    if hdr_is_layer_1(h) {
        frame_bytes &= !3; /* slot align */
    }
    if frame_bytes != 0 { frame_bytes } else { free_format_size }
}

fn hdr_padding(h: &Header) -> i32 {
    if hdr_test_padding(h) { if hdr_is_layer_1(h) { 4 } else { 1 } } else { 0 }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_read_side_info(bs: &mut Bs, gr_info: &mut [GrInfo; 4], hdr: &Header) -> i32 {
    #[rustfmt::skip]
    static G_SCF_LONG: [[u8; 23]; 8] = [
        [
            6, 6, 6, 6, 6, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 38,
            46, 52, 60, 68, 58, 54, 0,
        ],
        [
            12, 12, 12, 12, 12, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 76,
            90, 2, 2, 2, 2, 2, 0,
        ],
        [
            6, 6, 6, 6, 6, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 38,
            46, 52, 60, 68, 58, 54, 0,
        ],
        [
            6, 6, 6, 6, 6, 6, 8, 10, 12, 14, 16, 18, 22, 26, 32, 38,
            46, 54, 62, 70, 76, 36, 0,
        ],
        [
            6, 6, 6, 6, 6, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 38,
            46, 52, 60, 68, 58, 54, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 6, 6, 8, 8, 10, 12, 16, 20, 24, 28,
            34, 42, 50, 54, 76, 158, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 6, 6, 6, 8, 10, 12, 16, 18, 22, 28,
            34, 40, 46, 54, 54, 192, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 6, 6, 8, 10, 12, 16, 20, 24, 30, 38,
            46, 56, 68, 84, 102, 26, 0,
        ],
    ];
    #[rustfmt::skip]
    static G_SCF_SHORT: [[u8; 40]; 8] = [
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 8, 8, 8, 10,
            10, 10, 12, 12, 12, 14, 14, 14, 18, 18, 18, 24, 24, 24, 30, 30,
            30, 40, 40, 40, 18, 18, 18, 0,
        ],
        [
            8, 8, 8, 8, 8, 8, 8, 8, 8, 12, 12, 12, 16, 16, 16, 20,
            20, 20, 24, 24, 24, 28, 28, 28, 36, 36, 36, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 26, 26, 26, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 6, 6, 6, 8,
            8, 8, 10, 10, 10, 14, 14, 14, 18, 18, 18, 26, 26, 26, 32, 32,
            32, 42, 42, 42, 18, 18, 18, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 8, 8, 8, 10,
            10, 10, 12, 12, 12, 14, 14, 14, 18, 18, 18, 24, 24, 24, 32, 32,
            32, 44, 44, 44, 12, 12, 12, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 8, 8, 8, 10,
            10, 10, 12, 12, 12, 14, 14, 14, 18, 18, 18, 24, 24, 24, 30, 30,
            30, 40, 40, 40, 18, 18, 18, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 8,
            8, 8, 10, 10, 10, 12, 12, 12, 14, 14, 14, 18, 18, 18, 22, 22,
            22, 30, 30, 30, 56, 56, 56, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 6,
            6, 6, 10, 10, 10, 12, 12, 12, 14, 14, 14, 16, 16, 16, 20, 20,
            20, 26, 26, 26, 66, 66, 66, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 6, 8,
            8, 8, 12, 12, 12, 16, 16, 16, 20, 20, 20, 26, 26, 26, 34, 34,
            34, 42, 42, 42, 12, 12, 12, 0,
        ],
    ];
    #[rustfmt::skip]
    static G_SCF_MIXED: [[u8; 40]; 8] = [
        [
            6, 6, 6, 6, 6, 6, 6, 6, 6, 8, 8, 8, 10, 10, 10, 12,
            12, 12, 14, 14, 14, 18, 18, 18, 24, 24, 24, 30, 30, 30, 40, 40,
            40, 18, 18, 18, 0, 0, 0, 0,
        ],
        [
            12, 12, 12, 4, 4, 4, 8, 8, 8, 12, 12, 12, 16, 16, 16, 20,
            20, 20, 24, 24, 24, 28, 28, 28, 36, 36, 36, 2, 2, 2, 2, 2,
            2, 2, 2, 2, 26, 26, 26, 0,
        ],
        [
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 8, 8, 8, 10,
            10, 10, 14, 14, 14, 18, 18, 18, 26, 26, 26, 32, 32, 32, 42, 42,
            42, 18, 18, 18, 0, 0, 0, 0,
        ],
        [
            6, 6, 6, 6, 6, 6, 6, 6, 6, 8, 8, 8, 10, 10, 10, 12,
            12, 12, 14, 14, 14, 18, 18, 18, 24, 24, 24, 32, 32, 32, 44, 44,
            44, 12, 12, 12, 0, 0, 0, 0,
        ],
        [
            6, 6, 6, 6, 6, 6, 6, 6, 6, 8, 8, 8, 10, 10, 10, 12,
            12, 12, 14, 14, 14, 18, 18, 18, 24, 24, 24, 30, 30, 30, 40, 40,
            40, 18, 18, 18, 0, 0, 0, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 6, 6, 4, 4, 4, 6, 6, 6, 8, 8,
            8, 10, 10, 10, 12, 12, 12, 14, 14, 14, 18, 18, 18, 22, 22, 22,
            30, 30, 30, 56, 56, 56, 0, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 6, 6, 4, 4, 4, 6, 6, 6, 6, 6,
            6, 10, 10, 10, 12, 12, 12, 14, 14, 14, 16, 16, 16, 20, 20, 20,
            26, 26, 26, 66, 66, 66, 0, 0,
        ],
        [
            4, 4, 4, 4, 4, 4, 6, 6, 4, 4, 4, 6, 6, 6, 8, 8,
            8, 12, 12, 12, 16, 16, 16, 20, 20, 20, 26, 26, 26, 34, 34, 34,
            42, 42, 42, 12, 12, 12, 0, 0,
        ],
    ];

    let mut tables: u32;
    let mut scfsi: u32 = 0;
    let main_data_begin: i32;
    let mut part_23_sum: i32 = 0;
    let mut sr_idx = hdr_get_my_sample_rate(hdr);
    sr_idx -= usize::from(sr_idx != 0);
    let mut gr_count: i32 = if hdr_is_mono(hdr) { 1 } else { 2 };

    if hdr_test_mpeg1(hdr) {
        gr_count *= 2;
        main_data_begin = get_bits(bs, 9) as i32;
        scfsi = get_bits(bs, 7 + gr_count);
    } else {
        main_data_begin = (get_bits(bs, 8 + gr_count) >> gr_count) as i32;
    }

    let mut g = 0;
    loop {
        let gr = &mut gr_info[g];
        if hdr_is_mono(hdr) {
            scfsi <<= 4;
        }
        gr.part_23_length = get_bits(bs, 12) as u16;
        part_23_sum += i32::from(gr.part_23_length);
        gr.big_values = get_bits(bs, 9) as u16;
        if gr.big_values > 288 {
            return -1;
        }
        gr.global_gain = get_bits(bs, 8) as u8;
        gr.scalefac_compress = get_bits(bs, if hdr_test_mpeg1(hdr) { 4 } else { 9 }) as u16;
        gr.sfbtab = &G_SCF_LONG[sr_idx];
        gr.n_long_sfb = 22;
        gr.n_short_sfb = 0;
        if get_bits(bs, 1) != 0 {
            gr.block_type = get_bits(bs, 2) as u8;
            if gr.block_type == 0 {
                return -1;
            }
            gr.mixed_block_flag = get_bits(bs, 1) as u8;
            gr.region_count[0] = 7;
            gr.region_count[1] = 255;
            if gr.block_type == SHORT_BLOCK_TYPE {
                scfsi &= 0x0f0f;
                if gr.mixed_block_flag == 0 {
                    gr.region_count[0] = 8;
                    gr.sfbtab = &G_SCF_SHORT[sr_idx];
                    gr.n_long_sfb = 0;
                    gr.n_short_sfb = 39;
                } else {
                    gr.sfbtab = &G_SCF_MIXED[sr_idx];
                    gr.n_long_sfb = if hdr_test_mpeg1(hdr) { 8 } else { 6 };
                    gr.n_short_sfb = 30;
                }
            }
            tables = get_bits(bs, 10);
            tables <<= 5;
            gr.subblock_gain[0] = get_bits(bs, 3) as u8;
            gr.subblock_gain[1] = get_bits(bs, 3) as u8;
            gr.subblock_gain[2] = get_bits(bs, 3) as u8;
        } else {
            gr.block_type = 0;
            gr.mixed_block_flag = 0;
            tables = get_bits(bs, 15);
            gr.region_count[0] = get_bits(bs, 4) as u8;
            gr.region_count[1] = get_bits(bs, 3) as u8;
            gr.region_count[2] = 255;
        }
        gr.table_select[0] = (tables >> 10) as u8;
        gr.table_select[1] = ((tables >> 5) & 31) as u8;
        gr.table_select[2] = (tables & 31) as u8;
        gr.preflag = if hdr_test_mpeg1(hdr) { get_bits(bs, 1) as u8 } else { u8::from(gr.scalefac_compress >= 500) };
        gr.scalefac_scale = get_bits(bs, 1) as u8;
        gr.count1_table = get_bits(bs, 1) as u8;
        gr.scfsi = ((scfsi >> 12) & 15) as u8;
        scfsi <<= 4;
        g += 1;
        gr_count -= 1;
        if gr_count == 0 {
            break;
        }
    }

    if part_23_sum + bs.pos > bs.limit + main_data_begin * 8 {
        return -1;
    }

    main_data_begin
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_read_scalefactors(
    scf: &mut [u8; 40],
    ist_pos: &mut [u8; 39],
    scf_size: &[u8; 4],
    scf_count: &[u8],
    bitbuf: &mut Bs,
    mut scfsi: i32,
) {
    let (mut s_at, mut i_at) = (0, 0);
    let mut i = 0;
    while i < 4 && scf_count[i] != 0 {
        let cnt = usize::from(scf_count[i]);
        if scfsi & 8 != 0 {
            scf[s_at..s_at + cnt].copy_from_slice(&ist_pos[i_at..i_at + cnt]);
        } else {
            let bits = i32::from(scf_size[i]);
            if bits == 0 {
                scf[s_at..s_at + cnt].fill(0);
                ist_pos[i_at..i_at + cnt].fill(0);
            } else {
                let max_scf = if scfsi < 0 { (1 << bits) - 1 } else { -1 };
                for k in 0..cnt {
                    let s = get_bits(bitbuf, bits) as i32;
                    ist_pos[i_at + k] = if s == max_scf { 255 } else { s as u8 };
                    scf[s_at + k] = s as u8;
                }
            }
        }
        i_at += cnt;
        s_at += cnt;
        i += 1;
        scfsi *= 2;
    }
    scf[s_at] = 0;
    scf[s_at + 1] = 0;
    scf[s_at + 2] = 0;
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_ldexp_q2(mut y: f32, mut exp_q2: i32) -> f32 {
    #[rustfmt::skip]
    static G_EXPFRAC: [f32; 4] = [9.31322575e-10, 7.83145814e-10, 6.58544508e-10, 5.53767716e-10];
    loop {
        let e = (30 * 4).min(exp_q2);
        y *= G_EXPFRAC[(e & 3) as usize] * (1i32 << 30 >> (e >> 2)) as f32;
        exp_q2 -= e;
        if exp_q2 <= 0 {
            break;
        }
    }
    y
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_decode_scalefactors(
    hdr: &Header,
    ist_pos: &mut [u8; 39],
    bs: &mut Bs,
    gr: &GrInfo,
    scf: &mut [f32; 40],
    ch: usize,
) {
    #[rustfmt::skip]
    static G_SCF_PARTITIONS: [[u8; 28]; 3] = [
        [
            6, 5, 5, 5, 6, 5, 5, 5, 6, 5, 7, 3, 11, 10, 0, 0,
            7, 7, 7, 0, 6, 6, 6, 3, 8, 8, 5, 0,
        ],
        [
            8, 9, 6, 12, 6, 9, 9, 9, 6, 9, 12, 6, 15, 18, 0, 0,
            6, 15, 12, 0, 6, 12, 9, 6, 6, 18, 9, 0,
        ],
        [
            9, 9, 6, 12, 9, 9, 9, 9, 9, 9, 12, 6, 18, 18, 0, 0,
            12, 12, 12, 0, 12, 9, 9, 6, 15, 12, 9, 0,
        ],
    ];
    let scf_partition = &G_SCF_PARTITIONS[usize::from(gr.n_short_sfb != 0) + usize::from(gr.n_long_sfb == 0)];
    let mut partition_at = 0;
    let mut scf_size = [0u8; 4];
    let mut iscf = [0u8; 40];
    let scf_shift = i32::from(gr.scalefac_scale) + 1;
    let mut scfsi = i32::from(gr.scfsi);

    if hdr_test_mpeg1(hdr) {
        #[rustfmt::skip]
        static G_SCFC_DECODE: [u8; 16] = [0, 1, 2, 3, 12, 5, 6, 7, 9, 10, 11, 13, 14, 15, 18, 19];
        let part = G_SCFC_DECODE[usize::from(gr.scalefac_compress)];
        scf_size[0] = part >> 2;
        scf_size[1] = scf_size[0];
        scf_size[2] = part & 3;
        scf_size[3] = scf_size[2];
    } else {
        #[rustfmt::skip]
        static G_MOD: [u8; 24] = [
            5, 5, 4, 4, 5, 5, 4, 1, 4, 3, 1, 1, 5, 6, 6, 1,
            4, 4, 4, 1, 4, 3, 1, 1,
        ];
        let ist = i32::from(hdr_test_i_stereo(hdr) && ch != 0);
        let mut sfc = i32::from(gr.scalefac_compress) >> ist;
        let mut k = (ist * 3 * 4) as usize;
        while sfc >= 0 {
            let mut modprod = 1;
            for i in (0..4).rev() {
                scf_size[i] = (sfc / modprod % i32::from(G_MOD[k + i])) as u8;
                modprod *= i32::from(G_MOD[k + i]);
            }
            sfc -= modprod;
            k += 4;
        }
        partition_at = k;
        scfsi = -16;
    }
    l3_read_scalefactors(&mut iscf, ist_pos, &scf_size, &scf_partition[partition_at..], bs, scfsi);

    let (n_long, n_short) = (usize::from(gr.n_long_sfb), usize::from(gr.n_short_sfb));
    if n_short != 0 {
        let sh = 3 - scf_shift;
        let mut i = 0;
        while i < n_short {
            // As C's += on a uint8_t, which wraps.
            iscf[n_long + i] = iscf[n_long + i].wrapping_add(gr.subblock_gain[0] << sh);
            iscf[n_long + i + 1] = iscf[n_long + i + 1].wrapping_add(gr.subblock_gain[1] << sh);
            iscf[n_long + i + 2] = iscf[n_long + i + 2].wrapping_add(gr.subblock_gain[2] << sh);
            i += 3;
        }
    } else if gr.preflag != 0 {
        #[rustfmt::skip]
        static G_PREAMP: [u8; 10] = [1, 1, 1, 1, 2, 2, 3, 3, 3, 2];
        for i in 0..10 {
            iscf[11 + i] = iscf[11 + i].wrapping_add(G_PREAMP[i]);
        }
    }

    let gain_exp =
        i32::from(gr.global_gain) + BITS_DEQUANTIZER_OUT * 4 - 210 - if hdr_is_ms_stereo(hdr) { 2 } else { 0 };
    let gain = l3_ldexp_q2((1 << (MAX_SCFI / 4)) as f32, MAX_SCFI - gain_exp);
    for i in 0..n_long + n_short {
        scf[i] = l3_ldexp_q2(gain, i32::from(iscf[i]) << scf_shift);
    }
}

#[rustfmt::skip]
static G_POW43: [f32; 145] = [
    0.0, -1.0, -2.519842, -4.326749, -6.349604, -8.549880, -10.902724, -13.390518, -16.000000, -18.720754, -21.544347, -24.463781, -27.473142, -30.567351, -33.741992, -36.993181,
    0.0, 1.0, 2.519842, 4.326749, 6.349604, 8.549880, 10.902724, 13.390518, 16.000000, 18.720754, 21.544347, 24.463781, 27.473142, 30.567351, 33.741992, 36.993181,
    40.317474, 43.711787, 47.173345, 50.699631, 54.288352, 57.937408, 61.644865, 65.408941, 69.227979, 73.100443, 77.024898, 81.000000, 85.024491, 89.097188, 93.216975, 97.382800,
    101.593667, 105.848633, 110.146801, 114.487321, 118.869381, 123.292209, 127.755065, 132.257246, 136.798076, 141.376907, 145.993119, 150.646117, 155.335327, 160.060199, 164.820202, 169.614826,
    174.443577, 179.305980, 184.201575, 189.129918, 194.090580, 199.083145, 204.107210, 209.162385, 214.248292, 219.364564, 224.510845, 229.686789, 234.892058, 240.126328, 245.389280, 250.680604,
    256.000000, 261.347174, 266.721841, 272.123723, 277.552547, 283.008049, 288.489971, 293.998060, 299.532071, 305.091761, 310.676898, 316.287249, 321.922592, 327.582707, 333.267377, 338.976394,
    344.709550, 350.466646, 356.247482, 362.051866, 367.879608, 373.730522, 379.604427, 385.501143, 391.420496, 397.362314, 403.326427, 409.312672, 415.320884, 421.350905, 427.402579, 433.475750,
    439.570269, 445.685987, 451.822757, 457.980436, 464.158883, 470.357960, 476.577530, 482.817459, 489.077615, 495.357868, 501.658090, 507.978156, 514.317941, 520.677324, 527.056184, 533.454404,
    539.871867, 546.308458, 552.764065, 559.238575, 565.731879, 572.243870, 578.774440, 585.323483, 591.890898, 598.476581, 605.080431, 611.702349, 618.342238, 625.000000, 631.675540, 638.368763,
    645.079578,
];

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_pow_43(mut x: i32) -> f32 {
    let mut mult = 256;

    if x < 129 {
        return G_POW43[(16 + x) as usize];
    }

    if x < 1024 {
        mult = 16;
        x <<= 3;
    }

    let sign = 2 * x & 64;
    let frac = ((x & 63) - sign) as f32 / ((x & !63) + sign) as f32;
    G_POW43[(16 + ((x + sign) >> 6)) as usize] * (1.0 + frac * ((4.0 / 3.0) + frac * (2.0 / 9.0))) * mult as f32
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_huffman(dst: &mut [f32], bs: &mut Bs, gr_info: &GrInfo, scf: &[f32; 40], layer3gr_limit: i32) {
    #[rustfmt::skip]
    static TABS: [i16; 2164] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        785, 785, 785, 785, 784, 784, 784, 784, 513, 513, 513, 513, 513, 513, 513, 513,
        256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256,
        -255, 1313, 1298, 1282, 785, 785, 785, 785, 784, 784, 784, 784, 769, 769, 769, 769,
        256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256,
        290, 288, -255, 1313, 1298, 1282, 769, 769, 769, 769, 529, 529, 529, 529, 529, 529,
        529, 529, 528, 528, 528, 528, 528, 528, 528, 528, 512, 512, 512, 512, 512, 512,
        512, 512, 290, 288, -253, -318, -351, -367, 785, 785, 785, 785, 784, 784, 784, 784,
        769, 769, 769, 769, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256,
        256, 256, 256, 256, 819, 818, 547, 547, 275, 275, 275, 275, 561, 560, 515, 546,
        289, 274, 288, 258, -254, -287, 1329, 1299, 1314, 1312, 1057, 1057, 1042, 1042, 1026, 1026,
        784, 784, 784, 784, 529, 529, 529, 529, 529, 529, 529, 529, 769, 769, 769, 769,
        768, 768, 768, 768, 563, 560, 306, 306, 291, 259, -252, -413, -477, -542, 1298, -575,
        1041, 1041, 784, 784, 784, 784, 769, 769, 769, 769, 256, 256, 256, 256, 256, 256,
        256, 256, 256, 256, 256, 256, 256, 256, 256, 256, -383, -399, 1107, 1092, 1106, 1061,
        849, 849, 789, 789, 1104, 1091, 773, 773, 1076, 1075, 341, 340, 325, 309, 834, 804,
        577, 577, 532, 532, 516, 516, 832, 818, 803, 816, 561, 561, 531, 531, 515, 546,
        289, 289, 288, 258, -252, -429, -493, -559, 1057, 1057, 1042, 1042, 529, 529, 529, 529,
        529, 529, 529, 529, 784, 784, 784, 784, 769, 769, 769, 769, 512, 512, 512, 512,
        512, 512, 512, 512, -382, 1077, -415, 1106, 1061, 1104, 849, 849, 789, 789, 1091, 1076,
        1029, 1075, 834, 834, 597, 581, 340, 340, 339, 324, 804, 833, 532, 532, 832, 772,
        818, 803, 817, 787, 816, 771, 290, 290, 290, 290, 288, 258, -253, -349, -414, -447,
        -463, 1329, 1299, -479, 1314, 1312, 1057, 1057, 1042, 1042, 1026, 1026, 785, 785, 785, 785,
        784, 784, 784, 784, 769, 769, 769, 769, 768, 768, 768, 768, -319, 851, 821, -335,
        836, 850, 805, 849, 341, 340, 325, 336, 533, 533, 579, 579, 564, 564, 773, 832,
        578, 548, 563, 516, 321, 276, 306, 291, 304, 259, -251, -572, -733, -830, -863, -879,
        1041, 1041, 784, 784, 784, 784, 769, 769, 769, 769, 256, 256, 256, 256, 256, 256,
        256, 256, 256, 256, 256, 256, 256, 256, 256, 256, -511, -527, -543, 1396, 1351, 1381,
        1366, 1395, 1335, 1380, -559, 1334, 1138, 1138, 1063, 1063, 1350, 1392, 1031, 1031, 1062, 1062,
        1364, 1363, 1120, 1120, 1333, 1348, 881, 881, 881, 881, 375, 374, 359, 373, 343, 358,
        341, 325, 791, 791, 1123, 1122, -703, 1105, 1045, -719, 865, 865, 790, 790, 774, 774,
        1104, 1029, 338, 293, 323, 308, -799, -815, 833, 788, 772, 818, 803, 816, 322, 292,
        307, 320, 561, 531, 515, 546, 289, 274, 288, 258, -251, -525, -605, -685, -765, -831,
        -846, 1298, 1057, 1057, 1312, 1282, 785, 785, 785, 785, 784, 784, 784, 784, 769, 769,
        769, 769, 512, 512, 512, 512, 512, 512, 512, 512, 1399, 1398, 1383, 1367, 1382, 1396,
        1351, -511, 1381, 1366, 1139, 1139, 1079, 1079, 1124, 1124, 1364, 1349, 1363, 1333, 882, 882,
        882, 882, 807, 807, 807, 807, 1094, 1094, 1136, 1136, 373, 341, 535, 535, 881, 775,
        867, 822, 774, -591, 324, 338, -671, 849, 550, 550, 866, 864, 609, 609, 293, 336,
        534, 534, 789, 835, 773, -751, 834, 804, 308, 307, 833, 788, 832, 772, 562, 562,
        547, 547, 305, 275, 560, 515, 290, 290, -252, -397, -477, -557, -622, -653, -719, -735,
        -750, 1329, 1299, 1314, 1057, 1057, 1042, 1042, 1312, 1282, 1024, 1024, 785, 785, 785, 785,
        784, 784, 784, 784, 769, 769, 769, 769, -383, 1127, 1141, 1111, 1126, 1140, 1095, 1110,
        869, 869, 883, 883, 1079, 1109, 882, 882, 375, 374, 807, 868, 838, 881, 791, -463,
        867, 822, 368, 263, 852, 837, 836, -543, 610, 610, 550, 550, 352, 336, 534, 534,
        865, 774, 851, 821, 850, 805, 593, 533, 579, 564, 773, 832, 578, 578, 548, 548,
        577, 577, 307, 276, 306, 291, 516, 560, 259, 259, -250, -2107, -2507, -2764, -2909, -2974,
        -3007, -3023, 1041, 1041, 1040, 1040, 769, 769, 769, 769, 256, 256, 256, 256, 256, 256,
        256, 256, 256, 256, 256, 256, 256, 256, 256, 256, -767, -1052, -1213, -1277, -1358, -1405,
        -1469, -1535, -1550, -1582, -1614, -1647, -1662, -1694, -1726, -1759, -1774, -1807, -1822, -1854, -1886, 1565,
        -1919, -1935, -1951, -1967, 1731, 1730, 1580, 1717, -1983, 1729, 1564, -1999, 1548, -2015, -2031, 1715,
        1595, -2047, 1714, -2063, 1610, -2079, 1609, -2095, 1323, 1323, 1457, 1457, 1307, 1307, 1712, 1547,
        1641, 1700, 1699, 1594, 1685, 1625, 1442, 1442, 1322, 1322, -780, -973, -910, 1279, 1278, 1277,
        1262, 1276, 1261, 1275, 1215, 1260, 1229, -959, 974, 974, 989, 989, -943, 735, 478, 478,
        495, 463, 506, 414, -1039, 1003, 958, 1017, 927, 942, 987, 957, 431, 476, 1272, 1167,
        1228, -1183, 1256, -1199, 895, 895, 941, 941, 1242, 1227, 1212, 1135, 1014, 1014, 490, 489,
        503, 487, 910, 1013, 985, 925, 863, 894, 970, 955, 1012, 847, -1343, 831, 755, 755,
        984, 909, 428, 366, 754, 559, -1391, 752, 486, 457, 924, 997, 698, 698, 983, 893,
        740, 740, 908, 877, 739, 739, 667, 667, 953, 938, 497, 287, 271, 271, 683, 606,
        590, 712, 726, 574, 302, 302, 738, 736, 481, 286, 526, 725, 605, 711, 636, 724,
        696, 651, 589, 681, 666, 710, 364, 467, 573, 695, 466, 466, 301, 465, 379, 379,
        709, 604, 665, 679, 316, 316, 634, 633, 436, 436, 464, 269, 424, 394, 452, 332,
        438, 363, 347, 408, 393, 448, 331, 422, 362, 407, 392, 421, 346, 406, 391, 376,
        375, 359, 1441, 1306, -2367, 1290, -2383, 1337, -2399, -2415, 1426, 1321, -2431, 1411, 1336, -2447,
        -2463, -2479, 1169, 1169, 1049, 1049, 1424, 1289, 1412, 1352, 1319, -2495, 1154, 1154, 1064, 1064,
        1153, 1153, 416, 390, 360, 404, 403, 389, 344, 374, 373, 343, 358, 372, 327, 357,
        342, 311, 356, 326, 1395, 1394, 1137, 1137, 1047, 1047, 1365, 1392, 1287, 1379, 1334, 1364,
        1349, 1378, 1318, 1363, 792, 792, 792, 792, 1152, 1152, 1032, 1032, 1121, 1121, 1046, 1046,
        1120, 1120, 1030, 1030, -2895, 1106, 1061, 1104, 849, 849, 789, 789, 1091, 1076, 1029, 1090,
        1060, 1075, 833, 833, 309, 324, 532, 532, 832, 772, 818, 803, 561, 561, 531, 560,
        515, 546, 289, 274, 288, 258, -250, -1179, -1579, -1836, -1996, -2124, -2253, -2333, -2413, -2477,
        -2542, -2574, -2607, -2622, -2655, 1314, 1313, 1298, 1312, 1282, 785, 785, 785, 785, 1040, 1040,
        1025, 1025, 768, 768, 768, 768, -766, -798, -830, -862, -895, -911, -927, -943, -959, -975,
        -991, -1007, -1023, -1039, -1055, -1070, 1724, 1647, -1103, -1119, 1631, 1767, 1662, 1738, 1708, 1723,
        -1135, 1780, 1615, 1779, 1599, 1677, 1646, 1778, 1583, -1151, 1777, 1567, 1737, 1692, 1765, 1722,
        1707, 1630, 1751, 1661, 1764, 1614, 1736, 1676, 1763, 1750, 1645, 1598, 1721, 1691, 1762, 1706,
        1582, 1761, 1566, -1167, 1749, 1629, 767, 766, 751, 765, 494, 494, 735, 764, 719, 749,
        734, 763, 447, 447, 748, 718, 477, 506, 431, 491, 446, 476, 461, 505, 415, 430,
        475, 445, 504, 399, 460, 489, 414, 503, 383, 474, 429, 459, 502, 502, 746, 752,
        488, 398, 501, 473, 413, 472, 486, 271, 480, 270, -1439, -1455, 1357, -1471, -1487, -1503,
        1341, 1325, -1519, 1489, 1463, 1403, 1309, -1535, 1372, 1448, 1418, 1476, 1356, 1462, 1387, -1551,
        1475, 1340, 1447, 1402, 1386, -1567, 1068, 1068, 1474, 1461, 455, 380, 468, 440, 395, 425,
        410, 454, 364, 467, 466, 464, 453, 269, 409, 448, 268, 432, 1371, 1473, 1432, 1417,
        1308, 1460, 1355, 1446, 1459, 1431, 1083, 1083, 1401, 1416, 1458, 1445, 1067, 1067, 1370, 1457,
        1051, 1051, 1291, 1430, 1385, 1444, 1354, 1415, 1400, 1443, 1082, 1082, 1173, 1113, 1186, 1066,
        1185, 1050, -1967, 1158, 1128, 1172, 1097, 1171, 1081, -1983, 1157, 1112, 416, 266, 375, 400,
        1170, 1142, 1127, 1065, 793, 793, 1169, 1033, 1156, 1096, 1141, 1111, 1155, 1080, 1126, 1140,
        898, 898, 808, 808, 897, 897, 792, 792, 1095, 1152, 1032, 1125, 1110, 1139, 1079, 1124,
        882, 807, 838, 881, 853, 791, -2319, 867, 368, 263, 822, 852, 837, 866, 806, 865,
        -2399, 851, 352, 262, 534, 534, 821, 836, 594, 594, 549, 549, 593, 593, 533, 533,
        848, 773, 579, 579, 564, 578, 548, 563, 276, 276, 577, 576, 306, 291, 516, 560,
        305, 305, 275, 259, -251, -892, -2058, -2620, -2828, -2957, -3023, -3039, 1041, 1041, 1040, 1040,
        769, 769, 769, 769, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256, 256,
        256, 256, 256, 256, -511, -527, -543, -559, 1530, -575, -591, 1528, 1527, 1407, 1526, 1391,
        1023, 1023, 1023, 1023, 1525, 1375, 1268, 1268, 1103, 1103, 1087, 1087, 1039, 1039, 1523, -604,
        815, 815, 815, 815, 510, 495, 509, 479, 508, 463, 507, 447, 431, 505, 415, 399,
        -734, -782, 1262, -815, 1259, 1244, -831, 1258, 1228, -847, -863, 1196, -879, 1253, 987, 987,
        748, -767, 493, 493, 462, 477, 414, 414, 686, 669, 478, 446, 461, 445, 474, 429,
        487, 458, 412, 471, 1266, 1264, 1009, 1009, 799, 799, -1019, -1276, -1452, -1581, -1677, -1757,
        -1821, -1886, -1933, -1997, 1257, 1257, 1483, 1468, 1512, 1422, 1497, 1406, 1467, 1496, 1421, 1510,
        1134, 1134, 1225, 1225, 1466, 1451, 1374, 1405, 1252, 1252, 1358, 1480, 1164, 1164, 1251, 1251,
        1238, 1238, 1389, 1465, -1407, 1054, 1101, -1423, 1207, -1439, 830, 830, 1248, 1038, 1237, 1117,
        1223, 1148, 1236, 1208, 411, 426, 395, 410, 379, 269, 1193, 1222, 1132, 1235, 1221, 1116,
        976, 976, 1192, 1162, 1177, 1220, 1131, 1191, 963, 963, -1647, 961, 780, -1663, 558, 558,
        994, 993, 437, 408, 393, 407, 829, 978, 813, 797, 947, -1743, 721, 721, 377, 392,
        844, 950, 828, 890, 706, 706, 812, 859, 796, 960, 948, 843, 934, 874, 571, 571,
        -1919, 690, 555, 689, 421, 346, 539, 539, 944, 779, 918, 873, 932, 842, 903, 888,
        570, 570, 931, 917, 674, 674, -2575, 1562, -2591, 1609, -2607, 1654, 1322, 1322, 1441, 1441,
        1696, 1546, 1683, 1593, 1669, 1624, 1426, 1426, 1321, 1321, 1639, 1680, 1425, 1425, 1305, 1305,
        1545, 1668, 1608, 1623, 1667, 1592, 1638, 1666, 1320, 1320, 1652, 1607, 1409, 1409, 1304, 1304,
        1288, 1288, 1664, 1637, 1395, 1395, 1335, 1335, 1622, 1636, 1394, 1394, 1319, 1319, 1606, 1621,
        1392, 1392, 1137, 1137, 1137, 1137, 345, 390, 360, 375, 404, 373, 1047, -2751, -2767, -2783,
        1062, 1121, 1046, -2799, 1077, -2815, 1106, 1061, 789, 789, 1105, 1104, 263, 355, 310, 340,
        325, 354, 352, 262, 339, 324, 1091, 1076, 1029, 1090, 1060, 1075, 833, 833, 788, 788,
        1088, 1028, 818, 818, 803, 803, 561, 561, 531, 531, 816, 771, 546, 546, 289, 274,
        288, 258, -253, -317, -381, -446, -478, -509, 1279, 1279, -811, -1179, -1451, -1756, -1900, -2028,
        -2189, -2253, -2333, -2414, -2445, -2511, -2526, 1313, 1298, -2559, 1041, 1041, 1040, 1040, 1025, 1025,
        1024, 1024, 1022, 1007, 1021, 991, 1020, 975, 1019, 959, 687, 687, 1018, 1017, 671, 671,
        655, 655, 1016, 1015, 639, 639, 758, 758, 623, 623, 757, 607, 756, 591, 755, 575,
        754, 559, 543, 543, 1009, 783, -575, -621, -685, -749, 496, -590, 750, 749, 734, 748,
        974, 989, 1003, 958, 988, 973, 1002, 942, 987, 957, 972, 1001, 926, 986, 941, 971,
        956, 1000, 910, 985, 925, 999, 894, 970, -1071, -1087, -1102, 1390, -1135, 1436, 1509, 1451,
        1374, -1151, 1405, 1358, 1480, 1420, -1167, 1507, 1494, 1389, 1342, 1465, 1435, 1450, 1326, 1505,
        1310, 1493, 1373, 1479, 1404, 1492, 1464, 1419, 428, 443, 472, 397, 736, 526, 464, 464,
        486, 457, 442, 471, 484, 482, 1357, 1449, 1434, 1478, 1388, 1491, 1341, 1490, 1325, 1489,
        1463, 1403, 1309, 1477, 1372, 1448, 1418, 1433, 1476, 1356, 1462, 1387, -1439, 1475, 1340, 1447,
        1402, 1474, 1324, 1461, 1371, 1473, 269, 448, 1432, 1417, 1308, 1460, -1711, 1459, -1727, 1441,
        1099, 1099, 1446, 1386, 1431, 1401, -1743, 1289, 1083, 1083, 1160, 1160, 1458, 1445, 1067, 1067,
        1370, 1457, 1307, 1430, 1129, 1129, 1098, 1098, 268, 432, 267, 416, 266, 400, -1887, 1144,
        1187, 1082, 1173, 1113, 1186, 1066, 1050, 1158, 1128, 1143, 1172, 1097, 1171, 1081, 420, 391,
        1157, 1112, 1170, 1142, 1127, 1065, 1169, 1049, 1156, 1096, 1141, 1111, 1155, 1080, 1126, 1154,
        1064, 1153, 1140, 1095, 1048, -2159, 1125, 1110, 1137, -2175, 823, 823, 1139, 1138, 807, 807,
        384, 264, 368, 263, 868, 838, 853, 791, 867, 822, 852, 837, 866, 806, 865, 790,
        -2319, 851, 821, 836, 352, 262, 850, 805, 849, -2399, 533, 533, 835, 820, 336, 261,
        578, 548, 563, 577, 532, 532, 832, 772, 562, 562, 547, 547, 305, 275, 560, 515,
        290, 290, 288, 258,
    ];
    #[rustfmt::skip]
    static TAB32: [u8; 28] = [
        130, 162, 193, 209, 44, 28, 76, 140, 9, 9, 9, 9, 9, 9, 9, 9,
        190, 254, 222, 238, 126, 94, 157, 157, 109, 61, 173, 205,
    ];
    #[rustfmt::skip]
    static TAB33: [u8; 16] = [252, 236, 220, 204, 188, 172, 156, 140, 124, 108, 92, 76, 60, 44, 28, 12];
    #[rustfmt::skip]
    static TABINDEX: [i16; 32] = [
        0, 32, 64, 98, 0, 132, 180, 218, 292, 364, 426, 538, 648, 746, 0, 1126,
        1460, 1460, 1460, 1460, 1460, 1460, 1460, 1460, 1842, 1842, 1842, 1842, 1842, 1842, 1842, 1842,
    ];
    #[rustfmt::skip]
    static G_LINBITS: [u8; 32] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        1, 2, 3, 4, 6, 8, 10, 13, 4, 5, 6, 7, 8, 9, 11, 13,
    ];

    let buf = bs.buf;
    // A byte of the main data. C reads on past its end, into what follows it, when a stream is broken; this reads
    // zeros there instead.
    let byte = |i: usize| buf.get(i).map_or(0, |&b| u32::from(b));
    let mut one = 0.0f32;
    let mut ireg = 0;
    let mut big_val_cnt = i32::from(gr_info.big_values);
    let sfb = gr_info.sfbtab;
    let (mut sfb_at, mut scf_at, mut d) = (0, 0, 0);
    let mut bs_next_ptr = (bs.pos / 8) as usize;
    let mut bs_cache: u32 = (((byte(bs_next_ptr) * 256 + byte(bs_next_ptr + 1)) * 256 + byte(bs_next_ptr + 2)) * 256
        + byte(bs_next_ptr + 3))
        << (bs.pos & 7);
    let mut np: i32;
    let mut bs_sh: i32 = (bs.pos & 7) - 8;
    bs_next_ptr += 4;

    macro_rules! peek_bits {
        ($n:expr) => {
            bs_cache >> (32 - $n)
        };
    }
    macro_rules! flush_bits {
        ($n:expr) => {
            bs_cache <<= $n;
            bs_sh += $n as i32;
        };
    }
    macro_rules! check_bits {
        () => {
            while bs_sh >= 0 {
                bs_cache |= byte(bs_next_ptr) << bs_sh;
                bs_next_ptr += 1;
                bs_sh -= 8;
            }
        };
    }

    while big_val_cnt > 0 {
        let tab_num = usize::from(gr_info.table_select[ireg]);
        let mut sfb_cnt = i32::from(gr_info.region_count[ireg]);
        ireg += 1;
        let codebook = &TABS[TABINDEX[tab_num] as usize..];
        let linbits = u32::from(G_LINBITS[tab_num]);
        if linbits != 0 {
            loop {
                np = i32::from(sfb[sfb_at]) / 2;
                sfb_at += 1;
                let mut pairs_to_decode = big_val_cnt.min(np);
                one = scf[scf_at];
                scf_at += 1;
                loop {
                    let mut w = 5;
                    let mut leaf = i32::from(codebook[peek_bits!(w) as usize]);
                    while leaf < 0 {
                        flush_bits!(w);
                        w = (leaf & 7) as u32;
                        leaf = i32::from(codebook[(peek_bits!(w) as i32 - (leaf >> 3)) as usize]);
                    }
                    flush_bits!(leaf >> 8);

                    for _ in 0..2 {
                        let mut lsb = leaf & 0x0f;
                        if lsb == 15 {
                            lsb += peek_bits!(linbits) as i32;
                            flush_bits!(linbits);
                            check_bits!();
                            dst[d] = one * l3_pow_43(lsb) * if (bs_cache as i32) < 0 { -1.0 } else { 1.0 };
                        } else {
                            dst[d] = G_POW43[(16 + lsb - 16 * (bs_cache >> 31) as i32) as usize] * one;
                        }
                        flush_bits!(if lsb != 0 { 1 } else { 0 });
                        d += 1;
                        leaf >>= 4;
                    }
                    check_bits!();
                    pairs_to_decode -= 1;
                    if pairs_to_decode == 0 {
                        break;
                    }
                }
                big_val_cnt -= np;
                if big_val_cnt <= 0 {
                    break;
                }
                sfb_cnt -= 1;
                if sfb_cnt < 0 {
                    break;
                }
            }
        } else {
            loop {
                np = i32::from(sfb[sfb_at]) / 2;
                sfb_at += 1;
                let mut pairs_to_decode = big_val_cnt.min(np);
                one = scf[scf_at];
                scf_at += 1;
                loop {
                    let mut w = 5;
                    let mut leaf = i32::from(codebook[peek_bits!(w) as usize]);
                    while leaf < 0 {
                        flush_bits!(w);
                        w = (leaf & 7) as u32;
                        leaf = i32::from(codebook[(peek_bits!(w) as i32 - (leaf >> 3)) as usize]);
                    }
                    flush_bits!(leaf >> 8);

                    for _ in 0..2 {
                        let lsb = leaf & 0x0f;
                        dst[d] = G_POW43[(16 + lsb - 16 * (bs_cache >> 31) as i32) as usize] * one;
                        flush_bits!(if lsb != 0 { 1 } else { 0 });
                        d += 1;
                        leaf >>= 4;
                    }
                    check_bits!();
                    pairs_to_decode -= 1;
                    if pairs_to_decode == 0 {
                        break;
                    }
                }
                big_val_cnt -= np;
                if big_val_cnt <= 0 {
                    break;
                }
                sfb_cnt -= 1;
                if sfb_cnt < 0 {
                    break;
                }
            }
        }
    }

    np = 1 - big_val_cnt;
    loop {
        let codebook_count1: &[u8] = if gr_info.count1_table != 0 { &TAB33 } else { &TAB32 };
        let mut leaf = i32::from(codebook_count1[peek_bits!(4) as usize]);
        if leaf & 8 == 0 {
            leaf = i32::from(codebook_count1[((leaf >> 3) as u32 + (bs_cache << 4 >> (32 - (leaf & 3)))) as usize]);
        }
        flush_bits!(leaf & 7);
        if bs_next_ptr as i32 * 8 - 24 + bs_sh > layer3gr_limit {
            break;
        }
        macro_rules! reload_scalefactor {
            () => {
                np -= 1;
                if np == 0 {
                    np = i32::from(sfb[sfb_at]) / 2;
                    sfb_at += 1;
                    if np == 0 {
                        break;
                    }
                    one = scf[scf_at];
                    scf_at += 1;
                }
            };
        }
        macro_rules! deq_count1 {
            ($s:expr) => {
                if leaf & (128 >> $s) != 0 {
                    dst[d + $s] = if (bs_cache as i32) < 0 { -one } else { one };
                    flush_bits!(1);
                }
            };
        }
        reload_scalefactor!();
        deq_count1!(0);
        deq_count1!(1);
        reload_scalefactor!();
        deq_count1!(2);
        deq_count1!(3);
        check_bits!();
        d += 4;
    }

    bs.pos = layer3gr_limit;
}

/// The left channel's samples from `at` on, and the right's at 576 more.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_midside_stereo(grbuf: &mut [f32; 1152], at: usize, n: usize) {
    let (left, right) = grbuf.split_at_mut(576);
    for (l, r) in left[at..at + n].iter_mut().zip(&mut right[at..at + n]) {
        let a = *l;
        let b = *r;
        *l = a + b;
        *r = a - b;
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_intensity_stereo_band(grbuf: &mut [f32; 1152], at: usize, n: usize, kl: f32, kr: f32) {
    let (left, right) = grbuf.split_at_mut(576);
    for (l, r) in left[at..at + n].iter_mut().zip(&mut right[at..at + n]) {
        *r = *l * kr;
        *l *= kl;
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_stereo_top_band(right: &[f32], sfb: &[u8], nbands: usize, max_band: &mut [i32; 3]) {
    *max_band = [-1; 3];

    let mut r = 0;
    for i in 0..nbands {
        let n = usize::from(sfb[i]);
        let mut k = 0;
        while k < n {
            if right[r + k] != 0.0 || right[r + k + 1] != 0.0 {
                max_band[i % 3] = i as i32;
                break;
            }
            k += 2;
        }
        r += n;
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_stereo_process(
    grbuf: &mut [f32; 1152],
    ist_pos: &[u8; 39],
    sfb: &[u8],
    hdr: &Header,
    max_band: &[i32; 3],
    mpeg2_sh: u32,
) {
    #[rustfmt::skip]
    static G_PAN: [f32; 14] = [0.0, 1.0, 0.21132487, 0.78867513, 0.36602540, 0.63397460, 0.5, 0.5, 0.63397460, 0.36602540, 0.78867513, 0.21132487, 1.0, 0.0];
    let max_pos = if hdr_test_mpeg1(hdr) { 7 } else { 64 };

    let mut left = 0;
    let mut i = 0;
    while sfb[i] != 0 {
        let n = usize::from(sfb[i]);
        let ipos = u32::from(ist_pos[i]);
        if i as i32 > max_band[i % 3] && ipos < max_pos {
            let (kl, kr);
            let s = if hdr_test_ms_stereo(hdr) { 1.41421356 } else { 1.0 };
            if hdr_test_mpeg1(hdr) {
                kl = G_PAN[2 * ipos as usize];
                kr = G_PAN[2 * ipos as usize + 1];
            } else {
                let k = l3_ldexp_q2(1.0, ((ipos + 1) >> 1 << mpeg2_sh) as i32);
                (kl, kr) = if ipos & 1 != 0 { (k, 1.0) } else { (1.0, k) };
            }
            l3_intensity_stereo_band(grbuf, left, n, kl * s, kr * s);
        } else if hdr_test_ms_stereo(hdr) {
            l3_midside_stereo(grbuf, left, n);
        }
        left += n;
        i += 1;
    }
}

/// gr is the granule's side info for both channels, the left's first.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_intensity_stereo(grbuf: &mut [f32; 1152], ist_pos: &mut [u8; 39], gr: &[GrInfo], hdr: &Header) {
    let mut max_band = [0i32; 3];
    let n_sfb = usize::from(gr[0].n_long_sfb) + usize::from(gr[0].n_short_sfb);
    let max_blocks = if gr[0].n_short_sfb != 0 { 3 } else { 1 };

    l3_stereo_top_band(&grbuf[576..], gr[0].sfbtab, n_sfb, &mut max_band);
    if gr[0].n_long_sfb != 0 {
        max_band = [max_band[0].max(max_band[1]).max(max_band[2]); 3];
    }
    for i in 0..max_blocks {
        let default_pos = if hdr_test_mpeg1(hdr) { 3 } else { 0 };
        let itop = n_sfb - max_blocks + i;
        let prev = itop - max_blocks;
        ist_pos[itop] = if max_band[i] >= prev as i32 { default_pos } else { ist_pos[prev] };
    }
    l3_stereo_process(grbuf, ist_pos, gr[0].sfbtab, hdr, &max_band, u32::from(gr[1].scalefac_compress & 1));
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_reorder(grbuf: &mut [f32], scratch: &mut [f32], sfb: &[u8]) {
    let (mut src, mut dst, mut s) = (0, 0, 0);

    loop {
        let len = usize::from(sfb[s]);
        if len == 0 {
            break;
        }
        for _ in 0..len {
            scratch[dst] = grbuf[src];
            scratch[dst + 1] = grbuf[src + len];
            scratch[dst + 2] = grbuf[src + 2 * len];
            dst += 3;
            src += 1;
        }
        s += 3;
        src += 2 * len;
    }
    grbuf[..dst].copy_from_slice(&scratch[..dst]);
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_antialias(grbuf: &mut [f32], nbands: i32) {
    #[rustfmt::skip]
    static G_AA: [[f32; 8]; 2] = [
        [0.85749293, 0.88174200, 0.94962865, 0.98331459, 0.99551782, 0.99916056, 0.99989920, 0.99999316],
        [0.51449576, 0.47173197, 0.31337745, 0.18191320, 0.09457419, 0.04096558, 0.01419856, 0.00369997],
    ];

    // Across each boundary between bands, so not a band at a time.
    for b in 0..nbands.max(0) as usize {
        let g = &mut grbuf[18 * b..18 * b + 26];
        for i in 0..8 {
            let u = g[18 + i];
            let d = g[17 - i];
            g[18 + i] = u * G_AA[0][i] - d * G_AA[1][i];
            g[17 - i] = u * G_AA[1][i] + d * G_AA[0][i];
        }
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_dct3_9(y: &mut [f32; 9]) {
    let (mut s0, mut s1, mut s2, mut s3, mut s4, mut s5, mut s6, mut s7, mut s8, mut t0, mut t2, mut t4);

    s0 = y[0];
    s2 = y[2];
    s4 = y[4];
    s6 = y[6];
    s8 = y[8];
    t0 = s0 + s6 * 0.5;
    s0 -= s6;
    t4 = (s4 + s2) * 0.93969262;
    t2 = (s8 + s2) * 0.76604444;
    s6 = (s4 - s8) * 0.17364818;
    s4 += s8 - s2;

    s2 = s0 - s4 * 0.5;
    y[4] = s4 + s0;
    s8 = t0 - t2 + s6;
    s0 = t0 - t4 + t2;
    s4 = t0 + t4 - s6;

    s1 = y[1];
    s3 = y[3];
    s5 = y[5];
    s7 = y[7];

    s3 *= 0.86602540;
    t0 = (s5 + s1) * 0.98480775;
    t4 = (s5 - s7) * 0.34202014;
    t2 = (s1 + s7) * 0.64278761;
    s1 = (s1 - s5 - s7) * 0.86602540;

    s5 = t0 - s3 - t2;
    s7 = t4 - s3 - t0;
    s3 = t4 + s3 - t2;

    y[0] = s4 - s7;
    y[1] = s2 + s1;
    y[2] = s0 - s3;
    y[3] = s8 + s5;
    y[5] = s8 - s5;
    y[6] = s0 + s3;
    y[7] = s2 - s1;
    y[8] = s4 + s7;
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_imdct36(grbuf: &mut [f32], overlap: &mut [f32], window: &[f32; 18], nbands: usize) {
    #[rustfmt::skip]
    static G_TWID9: [f32; 18] = [
        0.73727734, 0.79335334, 0.84339145, 0.88701083, 0.92387953, 0.95371695, 0.97629601, 0.99144486, 0.99904822, 0.67559021, 0.60876143, 0.53729961, 0.46174861, 0.38268343, 0.30070580, 0.21643961,
        0.13052619, 0.04361938,
    ];

    for (grbuf, overlap) in grbuf.chunks_exact_mut(18).zip(overlap.chunks_exact_mut(9)).take(nbands) {
        let mut co = [0.0f32; 9];
        let mut si = [0.0f32; 9];
        co[0] = -grbuf[0];
        si[0] = grbuf[17];
        for i in 0..4 {
            si[8 - 2 * i] = grbuf[4 * i + 1] - grbuf[4 * i + 2];
            co[1 + 2 * i] = grbuf[4 * i + 1] + grbuf[4 * i + 2];
            si[7 - 2 * i] = grbuf[4 * i + 4] - grbuf[4 * i + 3];
            co[2 + 2 * i] = -(grbuf[4 * i + 3] + grbuf[4 * i + 4]);
        }
        l3_dct3_9(&mut co);
        l3_dct3_9(&mut si);

        si[1] = -si[1];
        si[3] = -si[3];
        si[5] = -si[5];
        si[7] = -si[7];

        for i in 0..9 {
            let ovl = overlap[i];
            let sum = co[i] * G_TWID9[9 + i] + si[i] * G_TWID9[i];
            overlap[i] = co[i] * G_TWID9[i] - si[i] * G_TWID9[9 + i];
            grbuf[i] = ovl * window[i] - sum * window[9 + i];
            grbuf[17 - i] = ovl * window[9 + i] + sum * window[i];
        }
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_idct3(x0: f32, x1: f32, x2: f32) -> [f32; 3] {
    let m1 = x1 * 0.86602540;
    let a1 = x0 - x2 * 0.5;
    [a1 + m1, x0 + x2, a1 - m1]
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_imdct12(x: &[f32], dst: &mut [f32], overlap: &mut [f32]) {
    #[rustfmt::skip]
    static G_TWID3: [f32; 6] = [0.79335334, 0.92387953, 0.99144486, 0.60876143, 0.38268343, 0.13052619];

    let co = l3_idct3(-x[0], x[6] + x[3], x[12] + x[9]);
    let mut si = l3_idct3(x[15], x[12] - x[9], x[6] - x[3]);
    si[1] = -si[1];

    for i in 0..3 {
        let ovl = overlap[i];
        let sum = co[i] * G_TWID3[3 + i] + si[i] * G_TWID3[i];
        overlap[i] = co[i] * G_TWID3[i] - si[i] * G_TWID3[3 + i];
        dst[i] = ovl * G_TWID3[2 - i] - sum * G_TWID3[5 - i];
        dst[5 - i] = ovl * G_TWID3[5 - i] + sum * G_TWID3[2 - i];
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_imdct_short(grbuf: &mut [f32], overlap: &mut [f32], nbands: usize) {
    for (grbuf, overlap) in grbuf.chunks_exact_mut(18).zip(overlap.chunks_exact_mut(9)).take(nbands) {
        let mut tmp = [0.0f32; 18];
        tmp.copy_from_slice(grbuf);
        grbuf[..6].copy_from_slice(&overlap[..6]);
        let (first, last) = overlap.split_at_mut(6);
        l3_imdct12(&tmp, &mut grbuf[6..12], last);
        l3_imdct12(&tmp[1..], &mut grbuf[12..18], last);
        l3_imdct12(&tmp[2..], first, last);
    }
}

/// Every other band from the second, 16 of them, the last ending at the granule's end.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_change_sign(grbuf: &mut [f32]) {
    for b in 0..16 {
        let band = &mut grbuf[18 + 36 * b..36 + 36 * b];
        for i in (1..18).step_by(2) {
            band[i] = -band[i];
        }
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_imdct_gr(grbuf: &mut [f32], overlap: &mut [f32; 288], block_type: u8, n_long_bands: usize) {
    #[rustfmt::skip]
    static G_MDCT_WINDOW: [[f32; 18]; 2] = [
        [
            0.99904822, 0.99144486, 0.97629601, 0.95371695, 0.92387953, 0.88701083, 0.84339145, 0.79335334, 0.73727734, 0.04361938, 0.13052619, 0.21643961, 0.30070580, 0.38268343, 0.46174861, 0.53729961,
            0.60876143, 0.67559021,
        ],
        [
            1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.99144486, 0.92387953, 0.79335334, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.13052619,
            0.38268343, 0.60876143,
        ],
    ];
    let (mut g, mut o) = (0, 0);
    if n_long_bands != 0 {
        l3_imdct36(grbuf, overlap, &G_MDCT_WINDOW[0], n_long_bands);
        g = 18 * n_long_bands;
        o = 9 * n_long_bands;
    }
    if block_type == SHORT_BLOCK_TYPE {
        l3_imdct_short(&mut grbuf[g..], &mut overlap[o..], 32 - n_long_bands);
    } else {
        let window = &G_MDCT_WINDOW[usize::from(block_type == STOP_BLOCK_TYPE)];
        l3_imdct36(&mut grbuf[g..], &mut overlap[o..], window, 32 - n_long_bands);
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_save_reservoir(h: &mut Mp3Dec, bs: &Bs) {
    let mut pos = (bs.pos + 7) / 8;
    let mut remains = bs.limit / 8 - pos;
    if remains > MAX_BITRESERVOIR_BYTES {
        pos += remains - MAX_BITRESERVOIR_BYTES;
        remains = MAX_BITRESERVOIR_BYTES;
    }
    if remains > 0 {
        let (pos, n) = (pos as usize, remains as usize);
        h.reserv_buf[..n].copy_from_slice(&bs.buf[pos..pos + n]);
    }
    h.reserv = remains;
}

/// Puts what the frame needs of the reservoir in front of its own main data. Returns whether the reservoir had it
/// all, and how many bytes there are.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_restore_reservoir(h: &Mp3Dec, bs: &Bs, maindata: &mut [u8], main_data_begin: i32) -> (bool, i32) {
    let frame_bytes = (bs.limit - bs.pos) / 8;
    let bytes_have = h.reserv.min(main_data_begin);
    let from = 0.max(h.reserv - main_data_begin) as usize;
    let (have, frame) = (bytes_have as usize, frame_bytes as usize);
    maindata[..have].copy_from_slice(&h.reserv_buf[from..from + have]);
    let start = (bs.pos / 8) as usize;
    maindata[have..have + frame].copy_from_slice(&bs.buf[start..start + frame]);
    (h.reserv >= main_data_begin, bytes_have + frame_bytes)
}

/// gr_info is the granule's side info, a channel's at a time.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn l3_decode(h: &mut Mp3Dec, s: &mut Scratch, bs: &mut Bs, gr_info: &[GrInfo], nch: usize) {
    for ch in 0..nch {
        let layer3gr_limit = bs.pos + i32::from(gr_info[ch].part_23_length);
        let (grbuf, scf) = grbuf_scf(&mut s.grbuf_scf);
        l3_decode_scalefactors(&h.header, &mut s.ist_pos[ch], bs, &gr_info[ch], scf, ch);
        l3_huffman(&mut grbuf[576 * ch..576 * ch + 576], bs, &gr_info[ch], scf, layer3gr_limit);
    }

    if hdr_test_i_stereo(&h.header) {
        l3_intensity_stereo(grbuf_scf(&mut s.grbuf_scf).0, &mut s.ist_pos[1], gr_info, &h.header);
    } else if hdr_is_ms_stereo(&h.header) {
        l3_midside_stereo(grbuf_scf(&mut s.grbuf_scf).0, 0, 576);
    }

    for (ch, gr_info) in gr_info.iter().enumerate().take(nch) {
        let mut aa_bands = 31;
        let n_long_bands =
            (if gr_info.mixed_block_flag != 0 { 2 } else { 0 }) << u32::from(hdr_get_my_sample_rate(&h.header) == 2);
        if gr_info.n_short_sfb != 0 {
            aa_bands = n_long_bands - 1;
            // To the end of scf, not the channel's: see Scratch.
            let short = &mut s.grbuf_scf[576 * ch + n_long_bands as usize * 18..];
            l3_reorder(short, &mut s.syn, &gr_info.sfbtab[usize::from(gr_info.n_long_sfb)..]);
        }
        let grbuf = &mut s.grbuf_scf[576 * ch..576 * ch + 576];

        l3_antialias(grbuf, aa_bands);
        l3_imdct_gr(grbuf, &mut h.mdct_overlap[ch], gr_info.block_type, n_long_bands as usize);
        l3_change_sign(grbuf);
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn mp3d_dct_ii(grbuf: &mut [f32], n: usize) {
    #[rustfmt::skip]
    static G_SEC: [f32; 24] = [
        10.19000816, 0.50060302, 0.50241929, 3.40760851, 0.50547093, 0.52249861, 2.05778098, 0.51544732, 0.56694406, 1.48416460, 0.53104258, 0.64682180, 1.16943991, 0.55310392, 0.78815460, 0.97256821,
        0.58293498, 1.06067765, 0.83934963, 0.62250412, 1.72244716, 0.74453628, 0.67480832, 5.10114861,
    ];

    for k in 0..n {
        let mut t = [[0.0f32; 8]; 4];
        let y = &mut grbuf[k..];

        for i in 0..8 {
            let x0 = y[i * 18];
            let x1 = y[(15 - i) * 18];
            let x2 = y[(16 + i) * 18];
            let x3 = y[(31 - i) * 18];
            let t0 = x0 + x3;
            let t1 = x1 + x2;
            let t2 = (x1 - x2) * G_SEC[3 * i];
            let t3 = (x0 - x3) * G_SEC[3 * i + 1];
            t[0][i] = t0 + t1;
            t[1][i] = (t0 - t1) * G_SEC[3 * i + 2];
            t[2][i] = t3 + t2;
            t[3][i] = (t3 - t2) * G_SEC[3 * i + 2];
        }
        for x in &mut t {
            let [mut x0, mut x1, mut x2, mut x3, mut x4, mut x5, mut x6, mut x7] = *x;
            let mut xt;
            xt = x0 - x7;
            x0 += x7;
            x7 = x1 - x6;
            x1 += x6;
            x6 = x2 - x5;
            x2 += x5;
            x5 = x3 - x4;
            x3 += x4;
            x4 = x0 - x3;
            x0 += x3;
            x3 = x1 - x2;
            x1 += x2;
            x[0] = x0 + x1;
            x[4] = (x0 - x1) * 0.70710677;
            x5 += x6;
            x6 = (x6 + x7) * 0.70710677;
            x7 += xt;
            x3 = (x3 + x4) * 0.70710677;
            x5 -= x7 * 0.198912367; /* rotate by PI/8 */
            x7 += x5 * 0.382683432;
            x5 -= x7 * 0.198912367;
            x0 = xt - x6;
            xt += x6;
            x[1] = (xt + x7) * 0.50979561;
            x[2] = (x4 + x3) * 0.54119611;
            x[3] = (x0 - x5) * 0.60134488;
            x[5] = (x0 + x5) * 0.89997619;
            x[6] = (x4 - x3) * 1.30656302;
            x[7] = (xt - x7) * 2.56291556;
        }
        let mut at = 0;
        for i in 0..7 {
            y[at] = t[0][i];
            y[at + 18] = t[2][i] + t[3][i] + t[3][i + 1];
            y[at + 2 * 18] = t[1][i] + t[1][i + 1];
            y[at + 3 * 18] = t[2][i + 1] + t[3][i] + t[3][i + 1];
            at += 4 * 18;
        }
        y[at] = t[0][7];
        y[at + 18] = t[2][7] + t[3][7];
        y[at + 2 * 18] = t[1][7];
        y[at + 3 * 18] = t[3][7];
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn mp3d_scale_pcm(sample: f32) -> i16 {
    if sample >= 32766.5 {
        return 32767;
    }
    if sample <= -32767.5 {
        return -32768;
    }
    let mut s = (sample + 0.5) as i16;
    s -= i16::from(s < 0); /* away from zero, to be compliant */
    s
}

/// Two samples, from z at lins[at..], to pcm[out] and 16 frames on.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn mp3d_synth_pair(pcm: &mut [i16], out: usize, nch: usize, lins: &[f32], at: usize) {
    let z = &lins[at..];
    let mut a;
    a = (z[14 * 64] - z[0]) * 29.0;
    a += (z[64] + z[13 * 64]) * 213.0;
    a += (z[12 * 64] - z[2 * 64]) * 459.0;
    a += (z[3 * 64] + z[11 * 64]) * 2037.0;
    a += (z[10 * 64] - z[4 * 64]) * 5153.0;
    a += (z[5 * 64] + z[9 * 64]) * 6574.0;
    a += (z[8 * 64] - z[6 * 64]) * 37489.0;
    a += z[7 * 64] * 75038.0;
    pcm[out] = mp3d_scale_pcm(a);

    let z = &z[2..];
    a = z[14 * 64] * 104.0;
    a += z[12 * 64] * 1567.0;
    a += z[10 * 64] * 9727.0;
    a += z[8 * 64] * 64019.0;
    a += z[6 * 64] * -9975.0;
    a += z[4 * 64] * -45.0;
    a += z[2 * 64] * 146.0;
    a += z[0] * -5.0;
    pcm[out + 16 * nch] = mp3d_scale_pcm(a);
}

/// Two bands, from grbuf[xl..] (and the right channel's 576 on), to pcm[dstl..], through lins[l..].
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn mp3d_synth(grbuf: &[f32], xl: usize, pcm: &mut [i16], dstl: usize, nch: usize, lins: &mut [f32], l: usize) {
    #[rustfmt::skip]
    static G_WIN: [f32; 240] = [
        -1.0, 26.0, -31.0, 208.0, 218.0, 401.0, -519.0, 2063.0, 2000.0, 4788.0, -5517.0, 7134.0, 5959.0, 35640.0, -39336.0, 74992.0,
        -1.0, 24.0, -35.0, 202.0, 222.0, 347.0, -581.0, 2080.0, 1952.0, 4425.0, -5879.0, 7640.0, 5288.0, 33791.0, -41176.0, 74856.0,
        -1.0, 21.0, -38.0, 196.0, 225.0, 294.0, -645.0, 2087.0, 1893.0, 4063.0, -6237.0, 8092.0, 4561.0, 31947.0, -43006.0, 74630.0,
        -1.0, 19.0, -41.0, 190.0, 227.0, 244.0, -711.0, 2085.0, 1822.0, 3705.0, -6589.0, 8492.0, 3776.0, 30112.0, -44821.0, 74313.0,
        -1.0, 17.0, -45.0, 183.0, 228.0, 197.0, -779.0, 2075.0, 1739.0, 3351.0, -6935.0, 8840.0, 2935.0, 28289.0, -46617.0, 73908.0,
        -1.0, 16.0, -49.0, 176.0, 228.0, 153.0, -848.0, 2057.0, 1644.0, 3004.0, -7271.0, 9139.0, 2037.0, 26482.0, -48390.0, 73415.0,
        -2.0, 14.0, -53.0, 169.0, 227.0, 111.0, -919.0, 2032.0, 1535.0, 2663.0, -7597.0, 9389.0, 1082.0, 24694.0, -50137.0, 72835.0,
        -2.0, 13.0, -58.0, 161.0, 224.0, 72.0, -991.0, 2001.0, 1414.0, 2330.0, -7910.0, 9592.0, 70.0, 22929.0, -51853.0, 72169.0,
        -2.0, 11.0, -63.0, 154.0, 221.0, 36.0, -1064.0, 1962.0, 1280.0, 2006.0, -8209.0, 9750.0, -998.0, 21189.0, -53534.0, 71420.0,
        -2.0, 10.0, -68.0, 147.0, 215.0, 2.0, -1137.0, 1919.0, 1131.0, 1692.0, -8491.0, 9863.0, -2122.0, 19478.0, -55178.0, 70590.0,
        -3.0, 9.0, -73.0, 139.0, 208.0, -29.0, -1210.0, 1870.0, 970.0, 1388.0, -8755.0, 9935.0, -3300.0, 17799.0, -56778.0, 69679.0,
        -3.0, 8.0, -79.0, 132.0, 200.0, -57.0, -1283.0, 1817.0, 794.0, 1095.0, -8998.0, 9966.0, -4533.0, 16155.0, -58333.0, 68692.0,
        -4.0, 7.0, -85.0, 125.0, 189.0, -83.0, -1356.0, 1759.0, 605.0, 814.0, -9219.0, 9959.0, -5818.0, 14548.0, -59838.0, 67629.0,
        -4.0, 7.0, -91.0, 117.0, 177.0, -106.0, -1428.0, 1698.0, 402.0, 545.0, -9416.0, 9916.0, -7154.0, 12980.0, -61289.0, 66494.0,
        -5.0, 6.0, -97.0, 111.0, 163.0, -127.0, -1498.0, 1634.0, 185.0, 288.0, -9585.0, 9838.0, -8540.0, 11455.0, -62684.0, 65290.0,
    ];
    let xr = xl + 576 * (nch - 1);
    let dstr = dstl + (nch - 1);

    let zlin = l + 15 * 64;
    let mut w = 0;

    lins[zlin + 4 * 15] = grbuf[xl + 18 * 16];
    lins[zlin + 4 * 15 + 1] = grbuf[xr + 18 * 16];
    lins[zlin + 4 * 15 + 2] = grbuf[xl];
    lins[zlin + 4 * 15 + 3] = grbuf[xr];

    lins[zlin + 4 * 31] = grbuf[xl + 1 + 18 * 16];
    lins[zlin + 4 * 31 + 1] = grbuf[xr + 1 + 18 * 16];
    lins[zlin + 4 * 31 + 2] = grbuf[xl + 1];
    lins[zlin + 4 * 31 + 3] = grbuf[xr + 1];

    mp3d_synth_pair(pcm, dstr, nch, lins, l + 4 * 15 + 1);
    mp3d_synth_pair(pcm, dstr + 32 * nch, nch, lins, l + 4 * 15 + 64 + 1);
    mp3d_synth_pair(pcm, dstl, nch, lins, l + 4 * 15);
    mp3d_synth_pair(pcm, dstl + 32 * nch, nch, lins, l + 4 * 15 + 64);

    for i in (0..15).rev() {
        let mut a = [0.0f32; 4];
        let mut b = [0.0f32; 4];

        lins[zlin + 4 * i] = grbuf[xl + 18 * (31 - i)];
        lins[zlin + 4 * i + 1] = grbuf[xr + 18 * (31 - i)];
        lins[zlin + 4 * i + 2] = grbuf[xl + 1 + 18 * (31 - i)];
        lins[zlin + 4 * i + 3] = grbuf[xr + 1 + 18 * (31 - i)];
        lins[zlin + 4 * (i + 16)] = grbuf[xl + 1 + 18 * (1 + i)];
        lins[zlin + 4 * (i + 16) + 1] = grbuf[xr + 1 + 18 * (1 + i)];
        // zlin[4*(i - 16) + 2] in C, before zlin.
        lins[zlin + 4 * i - 64 + 2] = grbuf[xl + 18 * (1 + i)];
        lins[zlin + 4 * i - 64 + 3] = grbuf[xr + 18 * (1 + i)];

        // S0(0) S2(1) S1(2) S2(3) S1(4) S2(5) S1(6) S2(7): S0 starts the sums, S1 adds, and S2 adds the other
        // difference.
        for k in 0..8 {
            let w0 = G_WIN[w];
            let w1 = G_WIN[w + 1];
            w += 2;
            let vz = zlin + 4 * i - k * 64;
            let vy = zlin + 4 * i - (15 - k) * 64;
            for j in 0..4 {
                let (z, y) = (lins[vz + j], lins[vy + j]);
                if k == 0 {
                    b[j] = z * w1 + y * w0;
                    a[j] = z * w0 - y * w1;
                } else if k & 1 == 0 {
                    b[j] += z * w1 + y * w0;
                    a[j] += z * w0 - y * w1;
                } else {
                    b[j] += z * w1 + y * w0;
                    a[j] += y * w1 - z * w0;
                }
            }
        }

        pcm[dstr + (15 - i) * nch] = mp3d_scale_pcm(a[1]);
        pcm[dstr + (17 + i) * nch] = mp3d_scale_pcm(b[1]);
        pcm[dstl + (15 - i) * nch] = mp3d_scale_pcm(a[0]);
        pcm[dstl + (17 + i) * nch] = mp3d_scale_pcm(b[0]);
        pcm[dstr + (47 - i) * nch] = mp3d_scale_pcm(a[3]);
        pcm[dstr + (49 + i) * nch] = mp3d_scale_pcm(b[3]);
        pcm[dstl + (47 - i) * nch] = mp3d_scale_pcm(a[2]);
        pcm[dstl + (49 + i) * nch] = mp3d_scale_pcm(b[2]);
    }
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn mp3d_synth_granule(
    qmf_state: &mut [f32; 15 * 2 * 32],
    grbuf: &mut [f32; 1152],
    nbands: usize,
    nch: usize,
    pcm: &mut [i16],
    lins: &mut [f32; (18 + 15) * 64],
) {
    for i in 0..nch {
        mp3d_dct_ii(&mut grbuf[576 * i..576 * i + 576], nbands);
    }

    lins[..15 * 64].copy_from_slice(qmf_state);

    for i in (0..nbands).step_by(2) {
        mp3d_synth(grbuf, i, pcm, 32 * nch * i, nch, lins, i * 64);
    }
    if nch == 1 {
        for i in (0..15 * 64).step_by(2) {
            qmf_state[i] = lins[nbands * 64 + i];
        }
    } else {
        qmf_state.copy_from_slice(&lins[nbands * 64..nbands * 64 + 15 * 64]);
    }
}

/// hdr is the stream from a frame's header on.
fn mp3d_match_frame(hdr: &[u8], frame_bytes: i32) -> bool {
    let mp3_bytes = hdr.len() as i32;
    let mut i = 0;
    for nmatch in 0..MAX_FRAME_SYNC_MATCHES {
        i += hdr_frame_bytes(at(hdr, i), frame_bytes) + hdr_padding(at(hdr, i));
        if i + HDR_SIZE > mp3_bytes {
            return nmatch > 0;
        }
        if !hdr_compare(at(hdr, 0), at(hdr, i)) {
            return false;
        }
    }
    true
}

fn mp3d_find_frame(mp3: &[u8], free_format_bytes: &mut i32, ptr_frame_bytes: &mut i32) -> i32 {
    let mp3_bytes = mp3.len() as i32;
    let mut i = 0;
    while i < mp3_bytes - HDR_SIZE {
        let h = at(mp3, i);
        if hdr_valid(h) {
            let mut frame_bytes = hdr_frame_bytes(h, *free_format_bytes);
            let mut frame_and_padding = frame_bytes + hdr_padding(h);

            let mut k = HDR_SIZE;
            while frame_bytes == 0 && k < MAX_FREE_FORMAT_FRAME_SIZE && i + 2 * k < mp3_bytes - HDR_SIZE {
                if hdr_compare(h, at(mp3, i + k)) {
                    let fb = k - hdr_padding(h);
                    let nextfb = fb + hdr_padding(at(mp3, i + k));
                    if !(i + k + nextfb + HDR_SIZE > mp3_bytes || !hdr_compare(h, at(mp3, i + k + nextfb))) {
                        frame_and_padding = k;
                        frame_bytes = fb;
                        *free_format_bytes = fb;
                    }
                }
                k += 1;
            }
            if (frame_bytes != 0
                && i + frame_and_padding <= mp3_bytes
                && mp3d_match_frame(&mp3[i as usize..], frame_bytes))
                || (i == 0 && frame_and_padding == mp3_bytes)
            {
                *ptr_frame_bytes = frame_and_padding;
                return i;
            }
            *free_format_bytes = 0;
        }
        i += 1;
    }
    *ptr_frame_bytes = 0;
    mp3_bytes
}

pub fn mp3dec_init(dec: &mut Mp3Dec) {
    dec.header[0] = 0;
}

/// Finds the next frame in mp3 and decodes it into pcm, if pcm is given: returns how many samples it made for each
/// channel, 0 if it made none. With no frame found, it only sets `info.frame_bytes`, to the bytes it skipped.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn mp3dec_decode_frame(
    dec: &mut Mp3Dec,
    mp3: &[u8],
    pcm: Option<&mut [i16; MAX_SAMPLES_PER_FRAME]>,
    info: &mut FrameInfo,
) -> i32 {
    let mp3_bytes = mp3.len() as i32;
    let mut i = 0;
    let mut frame_size = 0;

    if mp3_bytes > 4 && dec.header[0] == 0xff && hdr_compare(&dec.header, at(mp3, 0)) {
        frame_size = hdr_frame_bytes(at(mp3, 0), dec.free_format_bytes) + hdr_padding(at(mp3, 0));
        if frame_size != mp3_bytes
            && (frame_size + HDR_SIZE > mp3_bytes || !hdr_compare(at(mp3, 0), at(mp3, frame_size)))
        {
            frame_size = 0;
        }
    }
    if frame_size == 0 {
        dec.clear();
        i = mp3d_find_frame(mp3, &mut dec.free_format_bytes, &mut frame_size);
        if frame_size == 0 || i + frame_size > mp3_bytes {
            info.frame_bytes = i;
            return 0;
        }
    }

    let hdr = at(mp3, i);
    dec.header = *hdr;
    info.frame_bytes = i + frame_size;
    info.frame_offset = i;
    info.channels = if hdr_is_mono(hdr) { 1 } else { 2 };
    info.hz = hdr_sample_rate_hz(hdr) as i32;
    info.layer = 4 - hdr_get_layer(hdr) as i32;
    info.bitrate_kbps = hdr_bitrate_kbps(hdr) as i32;

    let Some(pcm) = pcm else {
        return hdr_frame_samples(hdr) as i32;
    };

    // A free-format frame can say it's shorter than its header, when its stream is broken; C's bits then run out at
    // once, and so do these.
    let payload = mp3.get((i + HDR_SIZE) as usize..(i + frame_size) as usize).unwrap_or(&[]);
    let mut bs_frame = Bs::new(payload, frame_size - HDR_SIZE);
    if hdr_is_crc(hdr) {
        get_bits(&mut bs_frame, 16);
    }

    if info.layer == 3 {
        let mut gr_info = [GrInfo::EMPTY; 4];
        let main_data_begin = l3_read_side_info(&mut bs_frame, &mut gr_info, hdr);
        if main_data_begin < 0 || bs_frame.pos > bs_frame.limit {
            mp3dec_init(dec);
            return 0;
        }
        let mut maindata = [0u8; MAX_BITRESERVOIR_BYTES as usize + MAX_L3_FRAME_PAYLOAD_BYTES];
        let (success, bytes) = l3_restore_reservoir(dec, &bs_frame, &mut maindata, main_data_begin);
        let mut bs = Bs::new(&maindata, bytes);
        if success {
            let nch = info.channels as usize;
            let mut scratch =
                Scratch { grbuf_scf: [0.0; 2 * 576 + 40], syn: [0.0; (18 + 15) * 2 * 32], ist_pos: [[0; 39]; 2] };
            for igr in 0..if hdr_test_mpeg1(hdr) { 2 } else { 1 } {
                grbuf_scf(&mut scratch.grbuf_scf).0.fill(0.0);
                l3_decode(dec, &mut scratch, &mut bs, &gr_info[igr * nch..], nch);
                let out = &mut pcm[576 * nch * igr..];
                let grbuf = grbuf_scf(&mut scratch.grbuf_scf).0;
                mp3d_synth_granule(&mut dec.qmf_state, grbuf, 18, nch, out, &mut scratch.syn);
            }
        }
        l3_save_reservoir(dec, &bs);
        i32::from(success) * hdr_frame_samples(&dec.header) as i32
    } else {
        // MINIMP3_ONLY_MP3: layers 1 and 2 aren't decoded.
        0
    }
}
