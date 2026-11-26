// SPDX-License-Identifier: MIT
//! A QOI image (qoiformat.org), written from the QOI specification, version 1.0.

/// A QOI image of at most max x max pixels, decoded to RGBA, 4 bytes a pixel, row by row, into `out`, which holds
/// max x max of them: its width and height. None if it isn't one, is bigger, or ends early.
pub fn decode(data: &[u8], out: &mut [u8], max: u32) -> Option<(u32, u32)> {
    if data.len() < 14 + 8 || &data[..4] != b"qoif" {
        return None;
    }
    let w = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let h = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    if w == 0 || h == 0 || w > max || h > max {
        return None;
    }
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let (mut at, n, end) = (14, (w * h) as usize, data.len() - 8);
    let mut i = 0;
    while i < n {
        if at >= end {
            return None;
        }
        let b = data[at];
        at += 1;
        let mut run = 1;
        if b == 0xfe || b == 0xff {
            let k = if b == 0xfe { 3 } else { 4 };
            if at + k > end {
                return None;
            }
            px[..k].copy_from_slice(&data[at..at + k]);
            at += k;
        } else if b >> 6 == 0 {
            px = index[usize::from(b)];
        } else if b >> 6 == 1 {
            px[0] = px[0].wrapping_add((b >> 4) & 3).wrapping_sub(2);
            px[1] = px[1].wrapping_add((b >> 2) & 3).wrapping_sub(2);
            px[2] = px[2].wrapping_add(b & 3).wrapping_sub(2);
        } else if b >> 6 == 2 {
            if at >= end {
                return None;
            }
            let dg = (b & 63).wrapping_sub(32);
            let d = data[at];
            at += 1;
            px[0] = px[0].wrapping_add(dg).wrapping_add(d >> 4).wrapping_sub(8);
            px[1] = px[1].wrapping_add(dg);
            px[2] = px[2].wrapping_add(dg).wrapping_add(d & 15).wrapping_sub(8);
        } else {
            run = usize::from(b & 63) + 1;
        }
        if b >> 6 != 3 || b >= 0xfe {
            let hash = (u32::from(px[0]) * 3 + u32::from(px[1]) * 5 + u32::from(px[2]) * 7 + u32::from(px[3]) * 11) % 64;
            index[hash as usize] = px;
        }
        while run > 0 && i < n {
            out[4 * i..4 * i + 4].copy_from_slice(&px);
            run -= 1;
            i += 1;
        }
    }
    Some((w, h))
}
