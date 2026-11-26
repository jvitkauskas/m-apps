// SPDX-License-Identifier: MIT
//! libFuzzer: apps/picker's QOI decoder (the launcher's icons) on any bytes.
#![no_main]
#[macro_use]
#[path = "target.rs"]
mod target;
#[path = "../../apps/picker/qoi.rs"]
mod qoi;

const MAX: u32 = 44;

fuzz!(|data| {
    // Exactly the pixels an icon of MAX x MAX can have, so that a write past them panics.
    let mut out = vec![0xa5u8; (MAX * MAX * 4) as usize];
    if let Some((w, h)) = qoi::decode(data, &mut out, MAX) {
        assert!(w != 0 && h != 0 && w <= MAX && h <= MAX);
        // Nothing written past the picture's own pixels.
        assert!(out[(w * h * 4) as usize..].iter().all(|&b| b == 0xa5));
    }
});
