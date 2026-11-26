// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! FM-1 time base: TIMER4 free-running from the 24 MHz crystal.
//! Measured on hardware: under mask-ROM UBOOT the crystal runs and TIMER4 counts 24 MHz, matching the host's
//! measurement of the same interval to 0.3 %. TIMER4 is not used by the ROM or the SPL. No interrupts, no ROM calls.
//!
//! JL_TIMER4 = 0x10800: CON [1:0]=1 run, [3:2]=2 OSC source, [7:4]=0 /1, bit14 = clear pending; CNT +4; PRD +8 (32-bit,
//! set to 0xFFFFFFFF). The counter wraps every 178.9 s; use differences (wrapping arithmetic).
use kit::hw::Reg;

const T4_CON: Reg = Reg::at::<0x10800>();
pub const T4_CNT: Reg = Reg::at::<0x10804>();
const T4_PRD: Reg = Reg::at::<0x10808>();
pub const TICKS_PER_US: u32 = 24;

/// Starts TIMER4 unless it already runs, so time stays continuous across RAM-run jumps.
pub fn init() {
    if T4_CON.read() & 0xf == (2 << 2) | 1 {
        return;
    }
    T4_CON.write(0);
    T4_PRD.write(0xffff_ffff);
    T4_CNT.write(0);
    T4_CON.write((2 << 2) | 1 | (1 << 14));
}

#[inline(always)]
pub fn ticks() -> u32 {
    T4_CNT.read()
}

#[inline(always)]
pub fn delay_us(us: u32) {
    let (start, span) = (T4_CNT.read(), us.wrapping_mul(TICKS_PER_US));
    while T4_CNT.read().wrapping_sub(start) < span {}
}
