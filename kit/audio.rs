// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
// Copyright (C) 2026 Kerem Kilic (Ellic Studio)
//! Audio out: I2S to the external codec, double-buffered DMA, polled. Adapted from felucca-bsp's
//! boards/fm1/fm1_audio.h (MPL-2.0): polled rather than by interrupt, on the kit's helpers. The pins are the same on
//! the FM-1 and the SMK-37: PC0 MCLK, PC1 BCLK, PC2 LRCK, PC6 data.
//!
//! Stereo frames, L then R, signed 24-bit in the low bits of an i32. A half buffer is HALF_FRAMES frames.
use crate::hw::{Reg, bit, gpio, sync_cpu};
use crate::platform::{gpio_output, hal_delay_us};
use crate::reg::{Reg8, Reg16};
use crate::Global;

pub const HALF_FRAMES: usize = 1152;
pub const HALF_WORDS: usize = 2 * HALF_FRAMES;
/// 24 MHz / 544, close to 44100, but only with the stock audio PLL.
pub const RATE_NUM: u32 = 24_000_000;
pub const RATE_DEN: u32 = 544;

const CON0: Reg16 = Reg16::at::<0x12e00>();
const CON1: Reg16 = Reg16::at::<0x12e04>();
const CON2: Reg8 = Reg8::at::<0x12e08>();
const CON3: Reg8 = Reg8::at::<0x12e0c>();
const ADR3: Reg = Reg::at::<0x12e1c>();
const LEN: Reg16 = Reg16::at::<0x12e20>();
const CLK_CON2: Reg = Reg::at::<0x10014>();
const IOMAP_CON5: Reg = Reg::at::<0x51030>();
const PLL_CON0: Reg = Reg::at::<0x119a0>();
const PLL_CON1: Reg = Reg::at::<0x119a4>();
const PLL_STOCK0: u32 = 0x4540_0203;
const PLL_STOCK1: u32 = 0x3f50_3026;
const PLL_CON1_MASK: u32 = 0x07ff_ffff;
const PENDING_HALF: u8 = 0x80;

/// The DMA's: both halves, which it reads in turn.
static BUFFER: Global<[i32; 2 * HALF_WORDS]> = Global::new([0; 2 * HALF_WORDS]);

/// Whether the audio PLL is as firmware and m-bootloader leave it.
pub fn clock_is_stock() -> bool {
    PLL_CON0.read() == PLL_STOCK0 && PLL_CON1.read() & PLL_CON1_MASK == PLL_STOCK1 & PLL_CON1_MASK
}

fn pin_out(pin: u32, high: bool) {
    gpio(2, 0xc).set(bit(pin));
    gpio_output(2, pin, high);
}

/// Sound going out: the halves to fill come from it, one at a time. There's one at most; dropping it stops the sound.
pub struct Audio(());

/// Whether an Audio exists, which holds the buffer.
static RUNNING: Global<bool> = Global::new(false);

/// Starts sound, with both halves silent, or None while it's going already: two would both fill the same halves.
pub fn start() -> Option<Audio> {
    if RUNNING.read() {
        return None;
    }
    RUNNING.set(true);
    // Safety: the DMA isn't running yet, and nothing else holds the buffer: no Audio exists.
    unsafe { BUFFER.get() }.fill(0);
    pin_out(6, false);
    hal_delay_us(5000);
    CON0.write(0);
    CON1.write(0);
    CON2.write(0);
    CON3.write(0);
    IOMAP_CON5.clear(0xc0);
    CON3.set(3);
    pin_out(0, true);
    CON0.set(0x100);
    pin_out(2, true);
    pin_out(1, true);
    CON0.set(0x80);
    CON0.clear(0x40);
    LEN.write(HALF_WORDS as u16);
    CON0.clear(0x400);
    CON0.clear(0x200);
    CLK_CON2.write(CLK_CON2.read() & 0xffff_f0ff);
    CON3.clear(0x1c);
    CON3.write((CON3.read() & 0x1f) | 0x80);
    CON1.set(1 << 12);
    CON1.set(1 << 14);
    // Safety: only for its address.
    ADR3.write(crate::hw::dma_address(unsafe { BUFFER.get() }.as_mut_ptr().cast()));
    pin_out(6, true);
    CON1.clear(1 << 15);
    CON2.write(0x0f);
    sync_cpu();
    CON0.set(0x800);
    hal_delay_us(5000);
    pin_out(6, true);
    Some(Audio(()))
}

impl Audio {
    /// Which half has come free to fill, if one has: 0 or 1. Fill it with [`Audio::half`], then call
    /// [`Audio::filled`].
    pub fn free_half(&mut self) -> Option<usize> {
        let pending = CON2.read();
        if pending & 0x10 != 0 {
            CON2.set(1);
        }
        if pending & 0x20 != 0 {
            CON2.set(2);
        }
        if pending & 0x40 != 0 {
            CON2.set(4);
        }
        if pending & PENDING_HALF == 0 {
            return None;
        }
        Some(usize::from(CON0.read() >> 15 & 1 ^ 1))
    }

    /// A half, 0 or 1: the one free_half() gave, while the DMA plays the other.
    pub fn half(&mut self, which: usize) -> &mut [i32; HALF_WORDS] {
        // Safety: borrowed from self, so one at a time. The DMA reads the other half meanwhile.
        let all = unsafe { BUFFER.get() };
        let (first, second) = all.split_at_mut(HALF_WORDS);
        let half = if which == 0 { first } else { second };
        half.try_into().unwrap_or_else(|_| unreachable!())
    }

    /// The half is full.
    pub fn filled(&mut self) {
        sync_cpu();
        CON2.set(0x08);
    }

    pub fn stop(self) {}
}

impl Drop for Audio {
    fn drop(&mut self) {
        CON0.clear(0x800);
        sync_cpu();
        CON2.write(0x0f);
        pin_out(6, false);
        RUNNING.set(false);
    }
}
