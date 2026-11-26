// SPDX-License-Identifier: MIT
//! Hangs on purpose, to test the guard: make run APP=hangtest. The watchdog should reset the unit into download mode
//! after 32 s, and the log should end with the line below.
#![no_std]
use kit::log;

kit::app!(main);

fn main(_lsb_mhz: u32) -> i32 {
    log!("about to hang");
    // An empty loop that never feeds the watchdog. (Unlike C++, Rust keeps a loop with no side effects.)
    #[allow(clippy::empty_loop)]
    loop {}
}
