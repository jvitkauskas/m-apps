// SPDX-License-Identifier: MIT
//! The kit: what every app is built on. Its start and its guard ([`app`]), the log ([`log!`]), and the card, the
//! screen, the keys and sound. The chip's drivers are m-bootloader's (the `m-bootloader` submodule), as the crate
//! `mboot`; [`mboot`] is that crate, for what the kit doesn't wrap.
//!
//! An app is a crate of its own, built against this one for one board, with [`app!`] naming its main.
#![no_std]
#![cfg_attr(target_arch = "pi32v2", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "pi32v2", feature(optimize_attribute))]

pub use mboot;
pub use mboot::{Aligned, Global, board, display, hw, platform};

pub mod app;
pub mod audio;
pub mod canvas;
#[cfg(target_arch = "pi32v2")]
pub mod card;
/// On this computer, for the tests: a card in memory.
#[cfg(not(target_arch = "pi32v2"))]
#[path = "card_sim.rs"]
pub mod card;
pub mod fat;
pub mod font;
mod font_data;
pub mod keys;
pub mod load;
pub mod log;
pub mod pixel;
mod pixel_font;
pub mod reg;
pub mod textentry;

/// The board the app was built for: 1 FM-1, 2 SMK-37, as `display::init()` and the file's trailer count them, or 0
/// for a universal app, one file for both, which finds out which it's on when it starts ([`app::board`]).
pub const APP_BOARD: u32 = if cfg!(board = "fm1") {
    1
} else if cfg!(board = "smk37") {
    2
} else {
    0
};

/// Names the app's main: `kit::app!(main);` with `fn main(lsb_mhz: u32) -> i32`. It runs on its own stack, with
/// interrupts off, and the timer set for `lsb_mhz`. Returning goes back to what started the app, or to download mode
/// if that was the ROM. See m-bootloader's docs/SPEC.md §11.8 for what it may touch.
#[macro_export]
macro_rules! app {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub fn app_main(lsb_mhz: u32) -> i32 {
            $main(lsb_mhz)
        }
    };
}
