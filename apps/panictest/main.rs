// SPDX-License-Identifier: MIT
//! Panics on purpose, to test that a Rust panic ends in the guard's crash handler: make run APP=panictest. The log
//! should end with "PANIC: cause 00000002".
#![no_std]
use kit::log;

kit::app!(main);

fn main(lsb_mhz: u32) -> i32 {
    let table = [1u8, 2, 3, 4];
    // An index the compiler can't know, so the check stays: the clock is 48 or more, past the table's end.
    let index = core::hint::black_box(lsb_mhz as usize);
    log!("about to read past a table's end, at ", index);
    let value = table[index];
    log!("read ", value, ", and it should have stopped");
    1
}
