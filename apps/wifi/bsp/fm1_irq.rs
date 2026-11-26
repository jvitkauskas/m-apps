// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
// Copyright (C) 2026 Charles Vestal
//! FM-1 interrupt HAL: of felucca-bsp's file, only the global enable that the radio start-up turns off around a burst
//! (fm1_ble_rf.rs). The vectors, attach, exceptions and crash records are left out (apps/wifi/bsp/README.md).

/// The interrupt config word: bit 9 is the global enable that cli and sti clear and set.
#[inline(always)]
pub fn icfg() -> u32 {
    let v: u32;
    // Safety: reads a CPU register, and nothing else.
    unsafe { core::arch::asm!("{0} = icfg", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[inline(always)]
pub fn irq_off() {
    // Safety: interrupts off; a fence for the compiler too, as the C's "memory".
    unsafe { core::arch::asm!("cli", options(nostack, preserves_flags)) };
}

#[inline(always)]
pub fn irq_on() {
    // Safety: what was written reaches the hardware, then interrupts on again, as they were before irq_off().
    unsafe { core::arch::asm!("csync", "sti", options(nostack, preserves_flags)) };
}
