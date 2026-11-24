// SPDX-License-Identifier: MIT
//! An app's start: the two ways in, from m-bootloader's docs/SPEC.md §11.8, and the guard that keeps the log readable
//! if the app hangs or crashes. kit/start.S moves to the app's stacks and calls `app_start` here.
use crate::hw::sync_cpu;
use crate::platform::{flash_init, flash_jedec, hal_clock_b, hal_delay_us, hal_early, hal_reset, hal_timer_clock};
use crate::platform::{p33_and, p33_or, p33_read, p33_write};
use crate::{APP_BOARD, Global, display, log};

/// From here up, the entry's argument is the ROM's pointer, not a clock.
const ROM_POINTER: u32 = 0x0100_0000;
const RETAINED: usize = 0x01c7_fd80;
const DOWNLOAD_MODE: &[u8; 16] = b"usb_update_mode\0";
/// P33: the watchdog. Bits 3:0 are the time, 0xf is 32 s; bit 4 enables, bit 6 clears. Clearing bit 6 of the other
/// register makes it a plain reset.
const WDT_CON: u32 = 0x80;
const WDT_32S: u8 = 0x0f;
const WDT_ENABLE: u8 = 0x10;
const WDT_CLEAR: u8 = 0x40;
const VLD_KEEP: u32 = 0x17;
const VLD_WDT_EXCEPTION: u8 = 0x40;
/// The CPU exception: vector 1, enabled at priority 7 in bits 7:4 of the first interrupt config word. It's taken
/// with interrupts off.
const VECTOR_1: usize = 0x01c7_fe04;
const ICFG_0: usize = 0x01ee_f100;
const ICFG_EXCEPTION: u32 = 0xf0;
const EMU_MSG: usize = 0x01ee_f0d4;
const CRASH_SHOW_S: u32 = 8;
/// EMU_MSG for an illegal instruction, which a panic's trap is (pi32v2-llvm's PseudoTRAP, seen on an FM-1).
const CAUSE_ILLEGAL: u32 = 2;

/// The header at offset 4 of every app the kit builds, after the entry's jump (kit/start.S): the second way in,
/// which the card's launcher (apps/picker) calls with an [`AppLaunch`] instead of starting at the first byte.
pub const HEADER_MAGIC: &[u8; 4] = b"MAPP";
pub const HEADER_FORMAT: u8 = 1;

/// What the launcher tells an app it starts from its folder, /MBOOT/APPS/<folder>/BOOT.APP: the folder, so it can
/// read its own files there ([`folder`]). The cluster is the card's that was in when it started.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AppLaunch {
    pub magic: [u8; 4], // LNCH
    pub format: u8,
    pub reserved: [u8; 3],
    /// The folder's first cluster.
    pub folder: u32,
    /// Its 8.3 name, padded with spaces.
    pub folder_name: [u8; 8],
}
pub const LAUNCH_FORMAT: u8 = 1;

impl AppLaunch {
    pub const fn empty() -> AppLaunch {
        AppLaunch { magic: [0; 4], format: 0, reserved: [0; 3], folder: 0, folder_name: [0; 8] }
    }
}

/// A word of fixed memory: the chip's, not this program's.
fn at(address: usize) -> *mut u32 {
    core::ptr::with_exposed_provenance_mut(address)
}

fn read(address: usize) -> u32 {
    // Safety: the addresses here are the chip's (above), which nothing in Rust holds a reference to.
    unsafe { at(address).read_volatile() }
}

fn write(address: usize, value: u32) {
    unsafe { at(address).write_volatile(value) }
}

/// The retained area's first 16 bytes, which say where a reset goes.
fn retained_mode(bytes: &[u8; 16]) {
    let p: *mut u8 = core::ptr::with_exposed_provenance_mut(RETAINED);
    for (i, &b) in bytes.iter().enumerate() {
        unsafe { p.add(i).write_volatile(b) };
    }
}

fn retained_read() -> [u8; 16] {
    let p: *const u8 = core::ptr::with_exposed_provenance(RETAINED);
    let mut out = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = unsafe { p.add(i).read_volatile() };
    }
    out
}

struct Guard {
    on: bool,
    watchdog: u8,
    keep: u8,
    retained: [u8; 16],
    vector: u32,
    icfg: u32,
}

static GUARD: Global<Guard> =
    Global::new(Guard { on: false, watchdog: 0, keep: 0, retained: [0; 16], vector: 0, icfg: 0 });
static LAUNCH: Global<AppLaunch> = Global::new(AppLaunch::empty());
static DETECTED: Global<u32> = Global::new(0);

unsafe extern "C" {
    static __payload_end: u8;
    static mut __bss_start: u8;
    static mut __bss_end: u8;
    /// kit/start.S: the launcher's [`AppLaunch`], when it came in that way, and otherwise null.
    static app_launch_given: *const AppLaunch;
    fn app_crash_entry();
}

unsafe extern "Rust" {
    /// The app's, which [`app!`](crate::app!) names.
    safe fn app_main(lsb_mhz: u32) -> i32;
}

/// The first cluster of the app's folder on the card, /MBOOT/APPS/<folder>, when the card's launcher started it, and
/// otherwise 0. Mount the card first, then use it as a directory, such as with `fat::find()`.
pub fn folder() -> u32 {
    // Safety: only app_start writes it, before the app runs.
    unsafe { LAUNCH.get() }.folder
}

/// The guard is on when the app's main starts. If the app hangs or crashes, the unit resets into download mode with
/// the log intact. A hang is 32 s without a log line or a `feed()`, so call one of them in anything slower.
pub fn feed() {
    // Safety: nothing holds the guard across a call (here and in guard()).
    if unsafe { GUARD.get() }.on {
        p33_or(WDT_CON, WDT_CLEAR);
    }
}

/// Off for work that must not be cut short, such as writing flash, and when an app would rather restart normally.
pub fn guard(on: bool) {
    let g = unsafe { GUARD.get() };
    if on == g.on {
        return;
    }
    g.on = on;
    if on {
        g.retained = retained_read();
        g.watchdog = p33_read(WDT_CON);
        g.keep = p33_read(VLD_KEEP);
        g.vector = read(VECTOR_1);
        g.icfg = read(ICFG_0);
        // Any reset from here lands in download mode, which keeps RAM.
        retained_mode(DOWNLOAD_MODE);
        write(EMU_MSG, u32::MAX);
        write(VECTOR_1, app_crash_entry as *const () as usize as u32);
        write(ICFG_0, g.icfg | ICFG_EXCEPTION);
        p33_write(WDT_CON, 0);
        p33_and(VLD_KEEP, !VLD_WDT_EXCEPTION);
        p33_write(WDT_CON, WDT_32S | WDT_CLEAR);
        p33_or(WDT_CON, WDT_ENABLE);
    } else {
        p33_write(WDT_CON, 0);
        p33_write(VLD_KEEP, g.keep);
        p33_write(WDT_CON, g.watchdog & !WDT_ENABLE);
        write(ICFG_0, g.icfg);
        write(VECTOR_1, g.vector);
        retained_mode(&g.retained);
    }
    sync_cpu();
}

/// Which board this is: 1 FM-1, 2 SMK-37, or 0 if a universal app can't tell, when it mustn't touch the screen, the
/// keys or the card, as their pins differ. Found from the flash, never by driving pins (m-bootloader's SPEC §8.1): the
/// SMK-37's factory calibration (trimdat at 0xC1224) or m-bootloader's copy of it (TRIMCOPY at 0xFF000) says SMK-37;
/// neither, with an FM-1's flash chip (0x856014, 0xB36014), says FM-1. It starts the flash.
#[inline]
pub fn board() -> u32 {
    if APP_BOARD != 0 { APP_BOARD } else { detect_board() }
}

pub fn detect_board() -> u32 {
    if DETECTED.read() != 0 {
        return DETECTED.read();
    }
    let mut mark = [0u8; 8];
    if !flash_init(1, 1) {
        return 0;
    }
    let id = flash_jedec() & 0xff_ffff;
    let board = if crate::platform::read(0xc1224, &mut mark) && &mark == b"trimdat\0" {
        2
    } else if crate::platform::read(0xff000, &mut mark) && &mark == b"TRIMCOPY" {
        2
    } else if id == 0x85_6014 || id == 0xb3_6014 {
        1
    } else {
        0
    };
    DETECTED.set(board);
    crate::log!("board ", board, ", from the flash (chip ", log::Hex(id, 6), ")");
    board
}

/// A board already known, without reading the flash: for a crash, and the end.
fn known_board() -> u32 {
    if APP_BOARD != 0 { APP_BOARD } else { DETECTED.read() }
}

fn screen_says(colour: u16, first: &[u8], second: &[u8]) -> bool {
    let board = known_board();
    if board == 0 || !display::init(board) {
        return false;
    }
    for row in 0..15 {
        display::line(row, 0xffff, b"");
    }
    display::line(1, colour, first);
    display::line(3, 0xffff, second);
    true
}

/// The app's stacks may be what broke, but there's nothing better to run on. A second fault or a hang here ends with
/// the watchdog, and the log still says what happened.
#[unsafe(no_mangle)]
pub extern "C" fn app_crash() -> ! {
    let cause = read(EMU_MSG);
    let (near, called_from): (u32, u32);
    #[cfg(target_arch = "pi32v2")]
    unsafe {
        core::arch::asm!("{0} = reti", "{1} = rets", out(reg) near, out(reg) called_from, options(nomem, nostack))
    };
    #[cfg(not(target_arch = "pi32v2"))]
    {
        (near, called_from) = (0, 0);
    }
    // The exception is taken a little late, so "near": seen one call after the bad read. A Rust panic (a failed
    // bounds check, an unwrap of None, an overflow) is the compiler's trap, btbclr, which the CPU takes as an illegal
    // instruction: cause 2, near the trap.
    let what = if cause == CAUSE_ILLEGAL { "PANIC" } else { "CRASH" };
    crate::log!(what, ": cause ", log::Hex(cause, 8), " near ", log::Hex(near, 8), ", last call from ", log::Hex(called_from, 8));
    crash_end(if cause == CAUSE_ILLEGAL { b"APP STOPPED" } else { b"APP CRASHED" })
}

fn crash_end(title: &[u8]) -> ! {
    log::crashed();
    // The reset turns the screen off, so the message has to come first. Only a board already known: a crash is no
    // time to read flash.
    if screen_says(0xf800, title, b"SWITCH OFF AND ON") {
        display::line(4, 0xffff, b"TO START AGAIN");
        for _ in 0..CRASH_SHOW_S {
            hal_delay_us(1_000_000);
            feed();
        }
    }
    retained_mode(DOWNLOAD_MODE);
    sync_cpu();
    hal_reset()
}

/// kit/start.S calls this on the app's own stacks.
#[unsafe(no_mangle)]
pub extern "C" fn app_start(mut argument: u32) -> i32 {
    let from_rom = argument >= ROM_POINTER;
    // The file's trailer is about to be cleared with .bss. It says which file the log is from.
    let mut trailer = [0u8; 8];
    // Safety: the trailer is the 8 bytes after the image, and .bss is this program's, with nothing in it yet in use.
    unsafe {
        let end = &raw const __payload_end;
        for (i, t) in trailer.iter_mut().enumerate() {
            *t = end.add(i).read_volatile();
        }
        let (start, stop) = (&raw mut __bss_start, &raw mut __bss_end);
        let n = (stop as usize).wrapping_sub(start as usize);
        for i in 0..n {
            start.add(i).write_volatile(0);
        }
    }
    log::begin(&trailer);
    // The launcher's, copied: it stays in the launcher's RAM, but the app needn't rely on that.
    // Safety: start.S wrote it before calling here, and a launcher's is a whole AppLaunch.
    let given = unsafe { (&raw const app_launch_given).read_volatile() };
    if !given.is_null() {
        let l = unsafe { given.read_volatile() };
        if &l.magic == b"LNCH" && l.format == LAUNCH_FORMAT {
            unsafe { *LAUNCH.get() = l };
        }
    }
    if from_rom {
        hal_early();
        mboot::usb_midi::usb_midi_stop();
        hal_clock_b();
        argument = 48;
    } else {
        hal_timer_clock(argument);
    }
    guard(true);
    crate::log!("started by the ", if from_rom { "ROM" } else { "caller" }, ", LSB ", argument, " MHz");
    let result = app_main(argument);
    crate::log!("app_main returned ", result);
    log::end(result);
    guard(false);
    if from_rom {
        // The SMK-37's panel keeps its picture through the reset, so the app's last screen would stay up as if it were
        // still running: say what it is now.
        screen_says(0x07e0, b"THE APP HAS ENDED", b"IN DOWNLOAD MODE");
        // Download mode keeps RAM, where firmware would overwrite the log.
        retained_mode(DOWNLOAD_MODE);
        sync_cpu();
        hal_reset();
    }
    result
}
