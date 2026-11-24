// SPDX-License-Identifier: MIT
//! Crashes on purpose, to test the guard: make run APP=crashtest. The log should end with the crash.
#![no_std]
#![cfg_attr(target_arch = "pi32v2", feature(asm_experimental_arch))]
use kit::log;
use kit::log::Hex;

/// A word from an odd address, which the CPU refuses.
const ODD: usize = 0x01c3_0002;

kit::app!(main);

/// One 32-bit load from `address`, as C's `*(volatile uint32_t *)` makes: Rust can't read a misaligned word without
/// it being undefined behaviour, so the instruction is written out.
fn load_word(address: usize) -> u32 {
    let value: u32;
    #[cfg(target_arch = "pi32v2")]
    // Safety: the point is the fault: the CPU takes the exception on this load, and the guard's handler ends the app.
    unsafe {
        core::arch::asm!("{0} = [{1}+0]", out(reg) value, in(reg) address, options(nostack, readonly, preserves_flags))
    };
    #[cfg(not(target_arch = "pi32v2"))]
    {
        // (Only the unit has the fault to test; on a computer this just reads.)
        value = address as u32;
    }
    value
}

fn main(_lsb_mhz: u32) -> i32 {
    log!("about to read from an odd address");
    let odd = load_word(ODD);
    log!("read ", Hex(odd, 8), ", and it should have crashed");
    1
}
