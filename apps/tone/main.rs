// SPDX-License-Identifier: MIT
//! Plays two quiet tones and measures the real sample rate: the first step towards playing files.
#![no_std]
use kit::audio::{self, HALF_FRAMES, RATE_DEN, RATE_NUM};
use kit::display::line;
use kit::hw::{Reg, bit};
use kit::log::Hex;
use kit::platform::{hal_delay_us, hal_timer_clock};
use kit::reg::{Reg8, Reg16};
use kit::{APP_BOARD, app, log};

const WHITE: u16 = 0xffff;
const GREEN: u16 = 0x07e0;
const TONE_S: u32 = 3;
const TABLE: usize = 1024;
const LEVEL: f32 = 8388607.0 * 0.1;
const HALVES_PER_S: u32 = RATE_NUM / RATE_DEN / HALF_FRAMES as u32;
const TIMER0_CON: Reg = Reg::at::<0x10400>();
const TIMER0_COUNT: Reg = Reg::at::<0x10404>();
const TIMER0_PERIOD: Reg = Reg::at::<0x10408>();
const PLL_CON0: Reg = Reg::at::<0x119a0>();
const PLL_CON1: Reg = Reg::at::<0x119a4>();
const AUDIO_CON0: Reg16 = Reg16::at::<0x12e00>();
const AUDIO_CON3: Reg8 = Reg8::at::<0x12e0c>();

kit::app!(main);

/// One cycle, by a rotation, as there's no maths library.
fn make_wave(wave: &mut [i32; TABLE]) {
    let (mut c, mut s, step) = (1.0f32, 0.0f32, 2.0 * 3.14159265 / TABLE as f32);
    for w in wave.iter_mut() {
        *w = (s * LEVEL) as i32;
        let next_c = c - s * step;
        let next_s = s + c * step;
        let fix = 1.0 / (next_c * next_c + next_s * next_s) * 0.5 + 0.5;
        c = next_c * fix;
        s = next_s * fix;
    }
}

/// Plays hz for TONE_S seconds. Returns the frames per second counted, or 0 if the halves stopped coming, and the
/// longest a half took, in us.
fn play(audio: &mut audio::Audio, wave: &[i32; TABLE], hz: u32, lsb_mhz: u32, late: &mut u32) -> u32 {
    let mut phase = 0u32;
    let step = ((u64::from(hz) * TABLE as u64) << 16) * u64::from(RATE_DEN) / u64::from(RATE_NUM);
    let step = step as u32;
    // Timer 0 running free, as hal_delay_us() isn't used while this plays.
    TIMER0_CON.write(bit(14));
    TIMER0_COUNT.write(0);
    TIMER0_PERIOD.write(u32::MAX);
    TIMER0_CON.write(bit(0));
    let halves = HALVES_PER_S * TONE_S;
    let (mut started, mut last, mut most) = (0u32, 0u32, 0u32);
    let mut h = 0;
    while h < halves {
        let free = audio.free_half();
        let now = TIMER0_COUNT.read();
        let Some(which) = free else {
            if now.wrapping_sub(last) > 400_000_000 {
                return 0;
            }
            continue;
        };
        if h == 0 {
            started = now;
        } else {
            most = most.max(now.wrapping_sub(last));
        }
        last = now;
        for frame in audio.half(which).chunks_exact_mut(2) {
            let v = wave[(phase >> 16) as usize % TABLE];
            phase = phase.wrapping_add(step);
            frame[0] = v;
            frame[1] = v;
        }
        audio.filled();
        h += 1;
        app::feed();
    }
    let ticks = last.wrapping_sub(started);
    TIMER0_CON.write(bit(14));
    *late = most / lsb_mhz;
    // Frames per second, from what was counted.
    (u64::from(halves - 1) * HALF_FRAMES as u64 * u64::from(lsb_mhz) * 1_000_000 / u64::from(ticks)) as u32
}

fn main(lsb_mhz: u32) -> i32 {
    kit::display::init(APP_BOARD);
    for row in 0..15 {
        line(row, WHITE, b"");
    }
    line(1, GREEN, b"TONE TEST");
    log!(
        "LSB ",
        lsb_mhz,
        ", PLL ",
        Hex(PLL_CON0.read(), 8),
        " ",
        Hex(PLL_CON1.read(), 8),
        ", audio clock stock: ",
        u32::from(audio::clock_is_stock())
    );
    let mut wave = [0i32; TABLE];
    make_wave(&mut wave);
    let mut audio = audio::start();
    log!("started, CON0 ", Hex(u32::from(AUDIO_CON0.read()), 0), " CON3 ", Hex(u32::from(AUDIO_CON3.read()), 0));
    let mut gap = 0;
    line(3, WHITE, b"440 HZ");
    let mut rate = play(&mut audio, &wave, 440, lsb_mhz, &mut gap);
    log!("440 Hz: ", rate, " frames/s, longest half ", gap, " us");
    line(4, WHITE, b"880 HZ");
    rate = play(&mut audio, &wave, 880, lsb_mhz, &mut gap);
    log!("880 Hz: ", rate, " frames/s, longest half ", gap, " us");
    audio.stop();
    hal_timer_clock(lsb_mhz);
    line(6, GREEN, b"DONE");
    hal_delay_us(1_000_000);
    if rate != 0 { 0 } else { 1 }
}
