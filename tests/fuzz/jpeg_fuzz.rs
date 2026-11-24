// SPDX-License-Identifier: MIT
//! libFuzzer: apps/jpeg's header parser on any bytes. It must not read outside them (a slice's bounds check panics),
//! and what it accepts must point inside them.
#![no_main]
#[macro_use]
#[path = "target.rs"]
mod target;
// The decoder's half is for the unit, so this leaves it unused.
#[allow(dead_code)]
#[path = "../../apps/jpeg/jpeg_hw.rs"]
mod jpeg_hw;

fuzz!(|data| {
    if let Ok(im) = jpeg_hw::parse(data) {
        let (start, at) = (data.as_ptr() as usize, im.entropy.as_ptr() as usize);
        assert!(at >= start && im.entropy.len() <= data.len() && at - start <= data.len() - im.entropy.len());
        assert!(im.width != 0 && im.height != 0 && im.lines != 0);
    }
});
