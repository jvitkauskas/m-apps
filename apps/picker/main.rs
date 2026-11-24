// SPDX-License-Identifier: MIT
//! The card's launcher, /MBOOT/LAUNCH.BIN: the apps in /MBOOT/APPS as tiles over rising bubbles, and starts the chosen
//! one. MENU.BIN (apps/menu) loads it to 0x01C0A800, which an app that returns leaves alone (m-bootloader's docs/SPEC.md
//! §11.8), so an app can return to it. Its lists and pictures are in the apps' slot until an app loads there.
#![no_std]
mod catalog;
mod qoi;
mod scene;

use catalog::{App, Catalog, MOST};
use kit::app::{AppLaunch, LAUNCH_FORMAT};
use kit::board::{KEY_NEXT, KEY_PREV, KEY_RECOVERY, KEY_SELECT, board_init, board_keys, board_restore};
use kit::fat::{Error, Volume, until_zero};
use kit::hw::{Reg, sync_cpu};
use kit::load::{self, APP_ADDRESS};
use kit::{Aligned, Global, app, card, log};
use scene::{BAR_Y, ICON, ICON_BYTES, Layer, SIZE, Scene, TILES, Tile};

const ICON_CACHE: usize = 32768;
const ICON_FILE_MAX: u32 = 8192;
const FRAME_MS: u32 = 25;
const REPEAT_FIRST_MS: u32 = 400;
const REPEAT_MS: u32 = 120;
const T4_CON: Reg = Reg::at::<0x10800>();
const T4_CNT: Reg = Reg::at::<0x10804>();
const T4_PRD: Reg = Reg::at::<0x10808>();
const T4_PER_MS: u32 = 24000;
/// The SMK-37's panel clock divider.
const PANEL_CLOCK: Reg = Reg::at::<0x11d04>();
const STRIP_ROWS: i32 = 48;

/// All in the apps' slot, which is free until an app loads over it; the launcher reads and draws it all again then.
/// Only plain numbers and bytes, as it holds whatever was there before.
#[repr(C)]
struct Work {
    /// the scene's
    layer: Layer,
    /// what's being sent to the panel
    strip: [[u8; 2]; (SIZE * STRIP_ROWS) as usize],
    catalog: Catalog,
    /// in the cache; size 0 without one
    icon_at: [u32; MOST],
    icon_size: [u32; MOST],
    icon_cache: Aligned<ICON_CACHE>,
    icon_rgba: [u8; ICON_BYTES],
}
// It ends below the log, at 0x01C7F000 (kit/log.rs).
const _: () = assert!(size_of::<Work>() <= load::APP.largest as usize);

static VOLUME: Global<Volume> = Global::new(Volume::new());
/// An app with the kit's header gets told its folder (kit/app.rs's AppLaunch). The record stays in the launcher's RAM,
/// which the app leaves alone.
static LAUNCH: Global<AppLaunch> = Global::new(AppLaunch::empty());

kit::app!(main);

// ---- time: TIMER4 free-running from the 24 MHz crystal ----

/// Sets TIMER4 up unless it already runs that way, as after an app that left it alone. True if it started it again.
fn time_init() -> bool {
    if T4_CON.read() & 15 == (2 << 2 | 1) && T4_PRD.read() == u32::MAX {
        return false;
    }
    T4_CON.write(0);
    T4_PRD.write(u32::MAX);
    T4_CNT.write(0);
    T4_CON.write(2 << 2 | 1 | 1 << 14);
    true
}

fn ms_since(t: u32) -> u32 {
    T4_CNT.read().wrapping_sub(t) / T4_PER_MS
}

/// A line of text for the bar, as the C's 48-byte buffers had it.
#[derive(Clone, Copy)]
struct Line {
    bytes: [u8; 48],
    n: usize,
}

impl Line {
    const fn new() -> Line {
        Line { bytes: [0; 48], n: 0 }
    }

    fn get(&self) -> &[u8] {
        &self.bytes[..self.n]
    }

    /// As much as fits, leaving room for the C's terminating zero.
    fn push(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.bytes.len() - 1 {
                self.bytes[self.n] = c;
                self.n += 1;
            }
        }
    }

    fn number(&mut self, mut v: u32) {
        let mut digits = [0u8; 10];
        let mut k = 0;
        loop {
            digits[k] = b'0' + (v % 10) as u8;
            k += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            self.push(&digits[k..k + 1]);
        }
    }
}

// ---- the card ----

/// What read_list() found: the apps listed, whether there were more, and whether there's a card at all.
struct Listing {
    count: usize,
    more: bool,
    card_ok: bool,
}

/// The apps in /MBOOT/APPS, then their icons into the cache while there's room.
fn read_list(volume: &mut Volume, w: &mut Lists<'_>, lsb_mhz: u32) -> (Option<Error>, Listing) {
    let mut listing = Listing { count: 0, more: false, card_ok: false };
    card::open(lsb_mhz);
    let sectors = card::start_card(lsb_mhz);
    listing.card_ok = sectors != 0;
    let mut error = Some("NO CARD FOUND");
    if listing.card_ok {
        card::fast(lsb_mhz);
        let mut more = false;
        error = find_apps(volume, sectors, w.catalog, &mut more).err();
        if error.is_none() {
            listing.count = w.catalog.count;
            listing.more = more;
        }
        // the icons, into the cache while there's room
        let mut used = 0;
        for i in 0..listing.count {
            let icon = w.catalog.apps[i].icon;
            let size = icon.size;
            w.icon_size[i] = 0;
            let at = used as usize;
            if size != 0
                && size <= ICON_FILE_MAX
                && size <= ICON_CACHE as u32 - used
                && volume.read(&icon, 0, &mut w.icon_cache.0[at..at + size as usize])
            {
                w.icon_at[i] = used;
                w.icon_size[i] = size;
                used += (size + 3) & !3;
            }
            app::feed();
        }
    }
    card::close();
    (error, listing)
}

fn find_apps(volume: &mut Volume, sectors: u32, catalog: &mut Catalog, more: &mut bool) -> Result<(), Error> {
    volume.mount(sectors)?;
    let mut folder = volume.root();
    volume.folder(&mut folder, b"MBOOT      ", "NO /MBOOT/APPS")?;
    volume.folder(&mut folder, b"APPS       ", "NO /MBOOT/APPS")?;
    catalog::read(volume, folder, catalog, more)
}

// ---- drawing ----

/// The work area's lists, as read_list() fills them.
struct Lists<'w> {
    catalog: &'w mut Catalog,
    icon_at: &'w mut [u32; MOST],
    icon_size: &'w mut [u32; MOST],
    icon_cache: &'w mut Aligned<ICON_CACHE>,
}

/// What's on the screen: the scene, and what it shows.
struct Screen<'w> {
    scene: Scene<'w>,
    strip: &'w mut [[u8; 2]; (SIZE * STRIP_ROWS) as usize],
    apps: &'w [App],
    icon_at: &'w [u32; MOST],
    icon_size: &'w [u32; MOST],
    icon_cache: &'w [u8; ICON_CACHE],
    icon_rgba: &'w mut [u8; ICON_BYTES],
    board: u32,
}

fn draw(scene: &Scene<'_>, strip: &mut [[u8; 2]], x0: i32, y0: i32, x1: i32, y1: i32) {
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(SIZE), y1.min(SIZE));
    let mut y = y0;
    while y < y1 && x0 < x1 {
        let h = (y1 - y).min(STRIP_ROWS);
        let n = ((x1 - x0) * h) as usize;
        scene.render(x0, y, x1, y + h, &mut strip[..n]);
        kit::display::pixels(x0 as u32, y as u32, (x1 - x0) as u32, h as u32, strip[..n].as_flattened());
        y += STRIP_ROWS;
    }
}

fn colour_for(id: &[u8; 8]) -> u16 {
    const PALETTE: [u16; 8] = [0xe9a6, 0x2c9f, 0x8c1f, 0x2d2b, 0xfc60, 0x4ad9, 0xd2b9, 0x0596];
    let h = id.iter().fold(0u32, |h, &c| h.wrapping_mul(31).wrapping_add(u32::from(c)));
    PALETTE[h as usize % PALETTE.len()]
}

fn letter_for(name: &[u8]) -> u8 {
    for &c in until_zero(name) {
        if c.is_ascii_lowercase() {
            return c - 32;
        }
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return c;
        }
    }
    b'?'
}

impl Screen<'_> {
    fn count(&self) -> u32 {
        self.apps.len() as u32
    }

    fn keys(&self) -> &'static [u8] {
        if self.board == 2 { b"< > MOVE   SEQ PLAY START" } else { b"OCT-/+ MOVE   PLAY START" }
    }

    fn draw(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        draw(&self.scene, &mut self.strip[..], x0, y0, x1, y1);
    }

    /// The bubbles a step on, drawing what they changed.
    fn step(&mut self) {
        let strip = &mut self.strip[..];
        self.scene.step(|scene, x0, y0, x1, y1| draw(scene, strip, x0, y0, x1, y1));
    }

    /// app i, or the way back after the last, into slot
    fn tile(&mut self, slot: u32, i: u32, selected: bool) {
        let i = i as usize;
        if i >= self.apps.len() {
            let back = Tile {
                name: b"Back",
                about: b"To the bootloader's menu",
                version: b"",
                icon: None,
                colour: 0x8c71,
                letter: b'<',
            };
            self.scene.tile(slot, &back, selected);
            return;
        }
        let a = &self.apps[i];
        let (at, size) = (self.icon_at[i] as usize, self.icon_size[i] as usize);
        let icon = size != 0
            && qoi::decode(&self.icon_cache[at..at + size], &mut self.icon_rgba[..], ICON as u32)
                == Some((ICON as u32, ICON as u32));
        let t = Tile {
            name: until_zero(&a.text.name),
            about: until_zero(&a.text.about),
            version: until_zero(&a.text.version),
            icon: if icon { Some(&*self.icon_rgba) } else { None },
            colour: colour_for(&a.id),
            letter: letter_for(&a.text.name),
        };
        self.scene.tile(slot, &t, selected);
    }

    fn bar(&mut self, selection: u32, note: &[u8], colour: u16) {
        let mut at = Line::new();
        if selection < self.count() {
            at.number(selection + 1);
            at.push(b" / ");
            at.number(self.count());
        }
        let keys = self.keys();
        self.scene.bar(at.get(), keys, note, colour);
    }

    fn layout(&mut self, first: u32, selection: u32) {
        self.scene.clear_tiles();
        let mut s = 0;
        while s < TILES && first + s <= self.count() {
            self.tile(s, first + s, first + s == selection);
            s += 1;
        }
        self.scene.more(first > 0, first + TILES <= self.count());
    }
}

// ---- keys ----

/// A key that's just gone down, or repeats while held. Two equal reads a frame apart make a change count.
struct Keys {
    held: u32,
    last_read: u32,
    repeat_ms: u32,
    held_since: u32,
    script_at: usize,
    script_last: u32,
}

/// A test build's keys (make PICKER_KEYS=...), one every 1.5 s, on through the apps it starts: < > and s for start,
/// r for REC.
#[cfg(picker_keys)]
const SCRIPT: &[u8] = env!("PICKER_KEYS").as_bytes();
#[cfg(not(picker_keys))]
const SCRIPT: &[u8] = b"";

impl Keys {
    fn press(&mut self) -> u32 {
        if self.script_at < SCRIPT.len() && ms_since(self.script_last) >= 1500 {
            let k = SCRIPT[self.script_at];
            self.script_at += 1;
            self.script_last = T4_CNT.read();
            log!("script: ", log::Char(k));
            return match k {
                b'<' => KEY_PREV,
                b'>' => KEY_NEXT,
                b's' => KEY_SELECT,
                b'r' => KEY_RECOVERY,
                _ => 0,
            };
        }
        let now = board_keys();
        let mut pressed = 0;
        if now == self.last_read {
            pressed = now & !self.held;
            if now != self.held {
                self.held = now;
                self.held_since = T4_CNT.read();
                self.repeat_ms = REPEAT_FIRST_MS;
            } else if now & (KEY_PREV | KEY_NEXT) != 0 && ms_since(self.held_since) >= self.repeat_ms {
                pressed = now & (KEY_PREV | KEY_NEXT);
                self.repeat_ms += REPEAT_MS;
            }
        }
        self.last_read = now;
        pressed
    }
}

// ---- running an app ----

fn run_app(a: &App, lsb_mhz: u32) -> i32 {
    // Whatever starts an app has the watchdog off (§11.8), and the app sets up a guard of its own.
    app::guard(false);
    sync_cpu();
    let result = if a.launched_entry != 0 {
        // Safety: the only use of it, and the app only reads it.
        let launch = unsafe { LAUNCH.get() };
        *launch =
            AppLaunch { magic: *b"LNCH", format: LAUNCH_FORMAT, reserved: [0; 3], folder: a.folder, folder_name: a.id };
        // Safety: the app was just loaded and checked, and load::launched_entry() found this entry inside it, which
        // kit/start.S's app_entry_launched is: an app's entry with the record as its second argument.
        let entry: extern "C" fn(u32, *const AppLaunch) -> i32 =
            unsafe { core::mem::transmute(core::ptr::with_exposed_provenance::<u8>(a.launched_entry as usize)) };
        entry(lsb_mhz, launch)
    } else {
        // Safety: the app was just loaded there and checked, and its first byte is its entry (§11.8).
        let entry: extern "C" fn(u32) -> i32 =
            unsafe { core::mem::transmute(core::ptr::with_exposed_provenance::<u8>(APP_ADDRESS as usize)) };
        entry(lsb_mhz)
    };
    app::guard(true);
    result
}

fn describe_result(name: &[u8], result: i32) -> Line {
    let mut line = Line::new();
    line.push(&name[..name.len().min(24)]);
    line.push(b" returned ");
    if result < 0 {
        line.push(b"-");
    }
    line.number(result.unsigned_abs());
    line
}

/// What the person chose: an app, or to go back.
enum Choice {
    App(App),
    Back { card_ok: bool },
}

/// Where the launcher is between apps.
struct State {
    board: u32,
    selection: u32,
    first: u32,
    note: Option<Line>,
    last_run: [u8; 8],
    keys: Keys,
}

/// The work area, in the apps' slot.
///
/// # Safety
/// Nothing else uses the apps' slot while the reference lives: no app is loaded there.
unsafe fn work() -> &'static mut Work {
    // Safety: the slot is RAM this program doesn't otherwise use (kit/app.ld.S puts the launcher below it), Work fits it
    // (above), and any bytes are a Work, as it holds only numbers and bytes.
    unsafe { &mut *core::ptr::with_exposed_provenance_mut::<Work>(APP_ADDRESS as usize) }
}

/// Reads the card, shows its apps, and waits for a choice. When it's an app, the bar says it's starting.
fn choose(volume: &mut Volume, w: &mut Work, st: &mut State, lsb_mhz: u32) -> Choice {
    let Work { layer, strip, catalog, icon_at, icon_size, icon_cache, icon_rgba } = w;
    let scene = Scene::new(layer, T4_CNT.read());
    let mut lists = Lists { catalog, icon_at, icon_size, icon_cache };
    let (error, listing) = read_list(volume, &mut lists, lsb_mhz);
    if let Some(e) = error {
        log!("list: ", e);
    }
    log!(listing.count as u32, " apps", if listing.more { ", and more" } else { "" });
    let mut count = listing.count as u32;
    // The folder can have changed, as the app may have been what changed it.
    st.selection = st.selection.min(count);
    for (i, a) in lists.catalog.apps[..listing.count].iter().enumerate() {
        if a.id == st.last_run {
            st.selection = i as u32;
        }
    }
    if error.is_some() {
        count = 0;
    }
    let mut screen = Screen {
        scene,
        strip,
        apps: &lists.catalog.apps[..count as usize],
        icon_at: lists.icon_at,
        icon_size: lists.icon_size,
        icon_cache: &lists.icon_cache.0,
        icon_rgba,
        board: st.board,
    };
    if let Some(e) = error {
        screen.scene.clear_tiles();
        screen.scene.message(b"Nothing to start", e.as_bytes());
        let back: &[u8] = if st.board == 2 { b"SEQ PLAY: BACK" } else { b"PLAY: BACK" };
        screen.scene.bar(b"", back, b"", 0);
        st.selection = 0;
    } else {
        if st.selection < st.first || st.selection >= st.first + TILES {
            st.first = if st.selection >= TILES - 1 { st.selection - (TILES - 1) } else { 0 };
        }
        screen.layout(st.first, st.selection);
        let note = st.note.take();
        screen.bar(st.selection, note.as_ref().map_or(b"", Line::get), scene::RED);
    }
    screen.draw(0, 0, SIZE, SIZE);
    let k = &mut st.keys;
    k.held = board_keys();
    k.last_read = k.held;
    loop {
        let frame = T4_CNT.read();
        let keys = k.press();
        if keys & KEY_RECOVERY != 0 {
            // REC (SEQ REC) leaves, as "Back" does.
            st.selection = count;
            break;
        } else if keys & KEY_SELECT != 0 {
            break;
        } else if error.is_none() && keys & (KEY_PREV | KEY_NEXT) != 0 {
            let (was, old_first) = (st.selection, st.first);
            st.selection = if keys & KEY_PREV != 0 {
                if st.selection != 0 { st.selection - 1 } else { count }
            } else if st.selection < count {
                st.selection + 1
            } else {
                0
            };
            if st.selection < st.first {
                st.first = st.selection;
            } else if st.selection >= st.first + TILES {
                st.first = st.selection - (TILES - 1);
            }
            screen.layout(st.first, st.selection);
            screen.bar(st.selection, b"", 0);
            if st.first == old_first {
                // only the two tiles that changed
                let (a0, a1) = scene::tile_rows(was - st.first);
                let (b0, b1) = scene::tile_rows(st.selection - st.first);
                screen.draw(0, a0, SIZE, a1);
                screen.draw(0, b0, SIZE, b1);
            } else {
                screen.draw(0, 0, SIZE, BAR_Y);
            }
            screen.draw(0, BAR_Y, SIZE, SIZE);
            if !SCRIPT.is_empty() {
                log!("moved to ", st.selection, " in ", ms_since(frame), " ms");
            }
        }
        screen.step();
        app::feed();
        while ms_since(frame) < FRAME_MS {}
    }
    if st.selection >= count {
        return Choice::Back { card_ok: listing.card_ok };
    }
    // The app loads over the list, so what's needed afterwards is kept apart from it.
    let chosen = screen.apps[st.selection as usize];
    let mut starting = Line::new();
    starting.push(b"Starting ");
    starting.push(until_zero(&chosen.text.name));
    screen.scene.bar(b"", b"", starting.get(), 0x3a6c);
    screen.draw(0, BAR_Y, SIZE, SIZE);
    Choice::App(chosen)
}

fn main(lsb_mhz: u32) -> i32 {
    time_init();
    let mut st = State {
        board: 0,
        selection: 0,
        first: 0,
        note: None,
        last_run: [0; 8],
        keys: Keys { held: 0, last_read: 0, repeat_ms: 0, held_since: 0, script_at: 0, script_last: T4_CNT.read() },
    };
    st.board = app::board();
    if st.board == 0 {
        // Its pins aren't known, so nothing is touched: back to the bootloader's menu.
        log!("which board this is isn't known");
        return 1;
    }
    // Safety: the only use of it.
    let volume = unsafe { VOLUME.get() };
    loop {
        board_init(st.board == 2);
        kit::display::init(st.board);
        // The SMK-37's panel takes a faster clock than the bootloader gives it: from LSB/5 to LSB/2.
        if st.board == 2 {
            PANEL_CLOCK.write(1);
        }
        // Safety: no app is in the slot until the load below, after the work area's last use.
        let w = unsafe { work() };
        let a = match choose(volume, w, &mut st, lsb_mhz) {
            Choice::App(a) => a,
            Choice::Back { card_ok } => {
                if card_ok {
                    card::open(lsb_mhz);
                    card::finish();
                    card::close();
                }
                PANEL_CLOCK.write(4);
                board_restore();
                return 0;
            }
        };
        let name = until_zero(&a.text.name);
        card::open(lsb_mhz);
        card::fast(lsb_mhz);
        // Safety: the slot is no one's now: the work area isn't used again until the next turn of the loop.
        if let Err(e) = unsafe { load::load_at(volume, &a.file, APP_ADDRESS, &load::APP, st.board) } {
            log!(name, ": ", e);
            card::close();
            let mut note = Line::new();
            note.push(e.as_bytes());
            st.note = Some(note);
            // the load wrote over the layer
            continue;
        }
        // The app starts the card itself, and may not expect the CRCs on.
        card::finish();
        card::close();
        PANEL_CLOCK.write(4);
        board_restore();
        st.last_run = a.id;
        let result = run_app(&a, lsb_mhz);
        log!(name, " returned ", result);
        // An app may have stopped TIMER4 or used it for itself; without it the frame wait would never end.
        if time_init() {
            st.keys.held_since = T4_CNT.read();
            st.keys.script_last = T4_CNT.read();
        }
        if result != 0 {
            st.note = Some(describe_result(name, result));
        }
    }
}
