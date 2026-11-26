// SPDX-License-Identifier: MIT
//! A modded unit's SD card as a USB disk. Eject it on the computer, or press REC (SEQ REC on the SMK-37), to leave.
#![no_std]
#![cfg_attr(target_arch = "pi32v2", feature(asm_experimental_arch))]
mod look;
mod msc;

use kit::board::{KEY_RECOVERY, board_init, board_keys, board_restore};
use kit::canvas::{Canvas, Hooks};
use kit::hw::Reg;
use kit::platform::hal_delay_us;
use kit::{app, card, log};
use look::{Look, Model, State};

const TICK_US: u32 = 100;
const TIMER_CON: Reg = Reg::at::<0x10800>();
const TIMER_COUNT: Reg = Reg::at::<0x10804>();
const TIMER_PERIOD: Reg = Reg::at::<0x10808>();
const TICKS_PER_MS: u32 = 24000;
/// The screen and the keys, between USB commands: often enough to look alive, seldom enough not to slow it.
const LOOK_EVERY_MS: u32 = 50;
const SPEED_EVERY_MS: u32 = 500;
const NO_HOST_MS: u32 = 120_000;
const AFTER_EJECT_US: u32 = 300_000;
const SHOW_END_MS: u32 = 1200;
/// What the screen may send at a time while serving the card, so the host isn't kept waiting long.
const SCREEN_BYTES: u32 = 4096;

kit::app!(main);

fn timer_start() {
    if TIMER_CON.read() & 0xf == (2 << 2 | 1) {
        return;
    }
    TIMER_CON.write(0);
    TIMER_PERIOD.write(0xffff_ffff);
    TIMER_COUNT.write(0);
    TIMER_CON.write(2 << 2 | 1 | 1 << 14);
}

/// Milliseconds, from a timer that wraps every 179 s, so it has to be read more often than that.
struct Clock {
    ms: u32,
    ticks: u32,
}

impl Clock {
    fn now_ms(&mut self) -> u32 {
        let whole = TIMER_COUNT.read().wrapping_sub(self.ticks) / TICKS_PER_MS;
        self.ticks = self.ticks.wrapping_add(whole * TICKS_PER_MS);
        self.ms = self.ms.wrapping_add(whole);
        self.ms
    }
}

/// The card and the screen share SPI1: between commands the card is idle, so it can be let go while the screen is
/// drawn. Only while the card is open: before and after, sending needs nothing done.
struct CardOff {
    lsb_mhz: u32,
    open: bool,
}

impl Hooks for CardOff {
    fn before(&mut self) {
        if self.open {
            card::close();
        }
    }

    fn after(&mut self) {
        if self.open {
            card_on(self.lsb_mhz);
        }
    }
}

fn card_on(lsb_mhz: u32) {
    card::open(lsb_mhz);
    card::fast(lsb_mhz);
}

/// What's on the screen, if there is one, and when it was last brought up to date.
struct Screen<'a> {
    canvas: &'a mut Canvas,
    look: Look,
    model: Model,
    on: bool,
    looked_at: u32,
    /// Steps off the card to send, while it's open.
    card: CardOff,
}

impl Screen<'_> {
    fn look(&mut self, clock: &mut Clock) {
        if !self.on {
            return;
        }
        let now = clock.now_ms();
        let ms = now.wrapping_sub(self.looked_at);
        self.look.update(self.canvas, &self.model, ms, &mut self.card);
        self.looked_at = now;
    }

    fn wait_ms(&mut self, clock: &mut Clock, ms: u32) {
        let until = clock.now_ms().wrapping_add(ms);
        while (clock.now_ms().wrapping_sub(until) as i32) < 0 {
            self.look(clock);
            app::feed();
            hal_delay_us(10000);
        }
    }
}

fn main(lsb_mhz: u32) -> i32 {
    let mut clock = Clock { ms: 0, ticks: 0 };
    timer_start();
    clock.now_ms();
    let board = app::board();
    if board == 0 {
        // Its pins aren't known, so nothing is touched.
        log!("which board this is isn't known");
        return 1;
    }
    board_init(board == 2);
    // Safety: the only use of the canvas.
    let canvas = unsafe { kit::canvas::claim() };
    // Safety: the only use of the device.
    let msc = unsafe { msc::claim() };
    let mut screen = Screen {
        canvas,
        look: Look::new(),
        model: Model::new(State::Starting),
        on: kit::display::init(board),
        looked_at: 0,
        card: CardOff { lsb_mhz, open: false },
    };
    if screen.on {
        screen.look.start(screen.canvas, board, &mut screen.card);
        screen.look(&mut clock);
    }
    card::open(lsb_mhz);
    let sectors = card::start_card(lsb_mhz);
    card::close();
    if sectors == 0 {
        log!("NO CARD FOUND");
        screen.model.state = State::NoCard;
        screen.wait_ms(&mut clock, 5000);
        board_restore();
        return 1;
    }
    log!("card: ", sectors, " sectors, ", sectors / 2048, " MiB");
    screen.model.card_mib = sectors / 2048;
    screen.model.state = State::Waiting;
    screen.look(&mut clock);
    // A press that opened this doesn't count as leaving.
    while board_keys() & KEY_RECOVERY != 0 {
        hal_delay_us(10000);
    }
    card_on(lsb_mhz);
    screen.card.open = true;
    screen.canvas.budget(SCREEN_BYTES);
    msc.start(sectors);
    let started = clock.now_ms();
    let (mut looked, mut measured) = (started, started);
    let (mut read_before, mut written_before) = (0, 0);
    let (mut was_configured, mut pressed, mut no_host) = (false, false, false);
    while !msc.ejected() {
        msc.poll();
        hal_delay_us(TICK_US);
        let now = clock.now_ms();
        if now.wrapping_sub(looked) < LOOK_EVERY_MS {
            continue;
        }
        looked = now;
        app::feed();
        if msc.configured() && !was_configured {
            log!("host connected");
            was_configured = true;
            screen.model.state = State::Connected;
        }
        if now.wrapping_sub(measured) >= SPEED_EVERY_MS {
            let ms = now.wrapping_sub(measured);
            let stats = msc.stats();
            screen.model.read_kbs = (stats.sectors_read - read_before) * 500 / ms;
            screen.model.write_kbs = (stats.sectors_written - written_before) * 500 / ms;
            read_before = stats.sectors_read;
            written_before = stats.sectors_written;
            screen.model.read_kib = stats.sectors_read / 2;
            screen.model.written_kib = stats.sectors_written / 2;
            measured = now;
        }
        if board_keys() & KEY_RECOVERY != 0 {
            pressed = true;
            break;
        }
        if !was_configured && now.wrapping_sub(started) >= NO_HOST_MS {
            no_host = true;
            break;
        }
        screen.look(&mut clock);
    }
    // Let the host hear the end of its last command.
    for _ in 0..AFTER_EJECT_US / TICK_US {
        msc.poll();
        hal_delay_us(TICK_US);
    }
    msc.stop();
    card::finish();
    card::close();
    screen.card.open = false;
    screen.canvas.budget(0);
    let stats = msc.stats();
    log!(
        if pressed {
            "REC pressed"
        } else if no_host {
            "no host"
        } else {
            "ejected"
        },
        ": read ",
        stats.sectors_read / 2,
        " KiB, wrote ",
        stats.sectors_written / 2,
        " KiB, ",
        stats.errors,
        " errors"
    );
    screen.model.read_kbs = 0;
    screen.model.write_kbs = 0;
    screen.model.read_kib = stats.sectors_read / 2;
    screen.model.written_kib = stats.sectors_written / 2;
    screen.model.state = if pressed || no_host { State::Leaving } else { State::Ejected };
    screen.wait_ms(&mut clock, SHOW_END_MS);
    board_restore();
    0
}
