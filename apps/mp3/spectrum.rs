// SPDX-License-Identifier: MIT
//! Levels for the analyser's bars, from a frame of samples: a 512-point FFT, Hann-windowed, in fixed point.
#[path = "spectrum_tables.rs"]
mod tables;
use tables::{FFT_COS, FFT_SIN, HANN, N};

pub const BANDS: usize = 14;
pub const TOP: u8 = 14;

const LOG2_N: u32 = 9;
/// Each band's first bin; at 44.1 kHz a bin is 86 Hz. About two thirds of an octave each.
const EDGES: [u16; BANDS + 1] = [1, 2, 3, 4, 6, 8, 11, 15, 20, 27, 37, 50, 68, 100, 256];
/// A full-scale sine comes out near 96, in eighths of an octave (0.75 dB). Each step of the bars is 5 of them.
const FULL_SCALE: i32 = 96;
const PER_STEP: i32 = 5;

fn reversed(mut n: usize) -> usize {
    let mut r = 0;
    for _ in 0..LOG2_N {
        r = r << 1 | (n & 1);
        n >>= 1;
    }
    r
}

/// log2 in eighths: the top bit's place, and the three bits after it.
fn eighths(v: u32) -> i32 {
    if v == 0 {
        return 0;
    }
    let top = 31 - v.leading_zeros();
    let frac = if top >= 3 { (v >> (top - 3)) & 7 } else { (v << (3 - top)) & 7 };
    (top * 8 + frac) as i32
}

/// pcm holds frames of 16-bit samples, interleaved if there are two channels. It uses the first 512 frames, and treats
/// missing ones as silence. levels gets BANDS values from 0 to TOP, low notes first.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn run(pcm: &[i16], channels: u32, frames: usize, levels: &mut [u8; BANDS]) {
    let mut re = [0i32; N];
    let mut im = [0i32; N];
    for n in 0..N {
        let mut s = 0;
        if n < frames {
            s = if channels == 2 {
                (i32::from(pcm[2 * n]) + i32::from(pcm[2 * n + 1])) >> 1
            } else {
                i32::from(pcm[n])
            };
        }
        let w = i32::from(HANN[if n < N / 2 { n } else { N - 1 - n }]);
        // Halved, so that sums of products stay inside 32 bits.
        re[reversed(n)] = (s * w) >> 16;
    }
    let mut len = 2;
    while len <= N {
        let (half, step) = (len / 2, N / len);
        for i in (0..N).step_by(len) {
            for j in 0..half {
                let (c, s) = (i32::from(FFT_COS[j * step]), i32::from(FFT_SIN[j * step]));
                let (a, b) = (i + j, i + j + half);
                let tr = (re[b] * c + im[b] * s) >> 15;
                let ti = (im[b] * c - re[b] * s) >> 15;
                let (ur, ui) = (re[a], im[a]);
                re[a] = (ur + tr) >> 1;
                im[a] = (ui + ti) >> 1;
                re[b] = (ur - tr) >> 1;
                im[b] = (ui - ti) >> 1;
            }
        }
        len <<= 1;
    }
    for (b, level) in levels.iter_mut().enumerate() {
        let mut most = 0;
        for k in usize::from(EDGES[b])..usize::from(EDGES[b + 1]) {
            let (x, y) = (re[k].unsigned_abs(), im[k].unsigned_abs());
            let m = if x > y { x + y / 2 } else { y + x / 2 };
            most = most.max(m);
        }
        let l = (eighths(most) - (FULL_SCALE - i32::from(TOP) * PER_STEP)) / PER_STEP;
        *level = l.clamp(0, i32::from(TOP)) as u8;
    }
}
