// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! The radio's registers, by address: helpers that inline into their caller, so the access sequence (and its timing,
//! in the interrupts-off burst) is the one written at the call site, as fm1_cc.h's FM1_INLINE had it.
//!
//! The radio is reached at addresses kit::hw::Reg doesn't take (the BT block at 0x20000 and 0x2FC00, the MAC at
//! 0x30000), many of them from tables, so these take an address made when the app runs, and check it is one of the
//! chip's registers: a constant one is checked when built, as the check folds away once inlined.

/// The chip's registers the radio uses: the system's (0x10000..0x20000), the BT block's and the MAC's.
const fn is_register(a: u32) -> bool {
    a.is_multiple_of(4) && matches!(a, 0x1_0000..0x4_0000)
}

#[inline(always)]
fn at(a: u32) -> *mut u32 {
    assert!(is_register(a));
    core::ptr::with_exposed_provenance_mut(a as usize)
}

#[inline(always)]
pub fn rd(a: u32) -> u32 {
    // Safety: one of the chip's registers (checked), which nothing in Rust holds a reference to.
    unsafe { at(a).read_volatile() }
}

#[inline(always)]
pub fn wr(a: u32, v: u32) {
    // Safety: as rd's.
    unsafe { at(a).write_volatile(v) }
}

/// Read, then written back with `mask` cleared and `v` (within it) set.
#[inline(always)]
pub fn rmw(a: u32, mask: u32, v: u32) {
    wr(a, (rd(a) & !mask) | (v & mask));
}

#[inline(always)]
pub fn set(a: u32, bits: u32) {
    wr(a, rd(a) | bits);
}

#[inline(always)]
pub fn clear(a: u32, bits: u32) {
    wr(a, rd(a) & !bits);
}
