// SPDX-License-Identifier: MIT
//! The Barlow faces the launcher and the apps draw with ([`crate::canvas`]), from kit/font_data.rs.

/// A face: each glyph its lit box, as left, top and rows, then the rows, 4-bit alpha, two pixels to a byte.
pub struct Font {
    /// The height; the bitmap starts pad pixels left of the pen.
    pub h: u8,
    pub pad: u8,
    pub first: u8,
    pub last: u8,
    /// The advance and the bitmap's width, per glyph.
    pub adv: &'static [u8],
    pub bw: &'static [u8],
    /// Each glyph's offset in data.
    pub off: &'static [u16],
    pub data: &'static [u8],
}

impl Font {
    /// A character's glyph, '?' for one the face doesn't have.
    pub fn glyph(&self, ch: u8) -> usize {
        let ch = if ch < self.first || ch > self.last { b'?' } else { ch };
        usize::from(ch - self.first)
    }
}

pub use crate::font_data::{FONT_B, FONT_XS};
