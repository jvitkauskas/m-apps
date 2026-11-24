// SPDX-License-Identifier: MIT
//! The card's MENU.BIN (m-bootloader's docs/SPEC.md §11.5): it loads the launcher, /MBOOT/LAUNCH.BIN (apps/picker),
//! and runs it. Stage 2 has 32 KiB at 0x01C28000; the launcher runs at 0x01C0A800, with up to 48 KiB, which an app
//! that returns doesn't touch (§11.8), so this stays out of its way and only says what went wrong.
#![no_std]
use kit::board::{board_init, board_keys, board_restore};
use kit::display::line;
use kit::fat::{Error, Volume};
use kit::platform::hal_delay_us;
use kit::{Global, app, card, load, log};

const LAUNCHER: u32 = 0x01c0_a800;
const WHITE: u16 = 0xffff;
const RED: u16 = 0xf800;
const GREY: u16 = 0x8410;

static VOLUME: Global<Volume> = Global::new(Volume::new());

kit::app!(main);

fn run(volume: &mut Volume, lsb_mhz: u32) -> Result<(), Error> {
    card::open(lsb_mhz);
    let sectors = card::start_card(lsb_mhz);
    let result = if sectors == 0 {
        Err("NO CARD FOUND")
    } else {
        card::fast(lsb_mhz);
        load_from(volume, sectors)
    };
    // The launcher starts the card itself.
    if sectors != 0 {
        card::finish();
    }
    card::close();
    result
}

fn load_from(volume: &mut Volume, sectors: u32) -> Result<(), Error> {
    volume.mount(sectors)?;
    let mut folder = volume.root();
    volume.folder(&mut folder, b"MBOOT      ", "NO /MBOOT FOLDER")?;
    let file = volume.find(folder, b"LAUNCH  BIN")?.ok_or("NO /MBOOT/LAUNCH.BIN")?;
    // Safety: the launcher's RAM is no one's while this runs (above).
    unsafe { load::load_at(volume, &file, LAUNCHER, &load::LAUNCHER, app::board()) }
}

fn main(lsb_mhz: u32) -> i32 {
    // Safety: the only use of it.
    let volume = unsafe { VOLUME.get() };
    match run(volume, lsb_mhz) {
        Ok(()) => {
            // Like an app (§11.8): its own guard, and the watchdog off while it runs.
            app::guard(false);
            kit::hw::sync_cpu();
            // Safety: the launcher was just loaded there and checked, and it starts as an app's entry does.
            let launcher: extern "C" fn(u32) -> i32 =
                unsafe { core::mem::transmute(core::ptr::with_exposed_provenance::<u8>(LAUNCHER as usize)) };
            let result = launcher(lsb_mhz);
            app::guard(true);
            log!("the launcher returned ", result);
            0
        }
        Err(error) => {
            log!("launcher: ", error);
            let board = app::board();
            if board == 0 {
                return 0;
            }
            board_init(board == 2);
            if kit::display::init(board) {
                for row in 0..15 {
                    line(row, WHITE, b"");
                }
                line(1, WHITE, b"M-APPS");
                line(3, RED, error.as_bytes());
                line(5, GREY, b"PUT LAUNCH.BIN AND");
                line(6, GREY, b"MENU.BIN IN /MBOOT");
                line(8, WHITE, b"ANY KEY: BACK");
            }
            while board_keys() != 0 {
                hal_delay_us(20_000);
            }
            while board_keys() == 0 {
                hal_delay_us(20_000);
                app::feed();
            }
            board_restore();
            0
        }
    }
}
