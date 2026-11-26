// SPDX-License-Identifier: MIT
//! Keeps the WiFi networks to join, in /WIFI.TXT at the card's root (wifilist.rs), and tests them: adding one from
//! those heard, or by its name, with its password typed on the piano keys (kit::textentry), choosing the one to use,
//! and joining it to see it works (net.rs).
//!
//! Built with `--cfg wifi_autotest` (make WIFI_TEST=autotest), it needs no keys, for a test from a computer: what's
//! heard, then each saved network joined, all in the log.
#![no_std]
#![cfg_attr(target_arch = "pi32v2", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "pi32v2", feature(optimize_attribute))]

mod bsp {
    pub mod ble_rf_tables;
    pub mod fm1_ble;
    pub mod fm1_ble_rf;
    pub mod fm1_cc;
    pub mod fm1_irq;
    pub mod fm1_rf_cal;
    pub mod fm1_time;
    pub mod fm1_wifi;
    pub mod fm1_wifi_wlc;
}
mod frames;
mod net;
mod network;
mod screens;
mod sha1hw;
mod wifi_crypto;
mod wifi_ip;
mod wifi_sta;
mod wifilist;

use bsp::fm1_time;
use kit::board::{board_init, board_restore};
use kit::canvas::{self, Canvas, DIM, GREEN, Plain, RED};
use kit::fat::{ATTR_ARCHIVE, Volume};
use kit::hw::{Reg, bit, sync_cpu};
use kit::keys::{self, Keys, NEXT, OCT_DOWN, OCT_UP, PLAY, PREV, REC};
use kit::log::Hex;
use kit::platform::{hal_delay_us, hal_timer_clock};
use kit::{Global, app, card, display, log, textentry};
use net::Radio;
use network::{Ap, Joined, OPEN, OTHER};
use wifilist::{List, PASS_LEAST, PASS_MOST, SSID_MOST, TEXT_MOST, until_zero};

const POLL_US: u32 = 10000;
const DELETE_HOLD_MS: u32 = 1500;
const BLINK_MS: u32 = 500;
const MOST_HEARD: usize = 24;
const TICKS_PER_MS: u32 = 24000;

const FILE_NAME: &[u8; 11] = b"WIFI    TXT";
const NO_CARD: &str = "No card found";

kit::app!(main);

/// What's to say, if anything.
type Note = Option<&'static str>;

fn bytes(note: Note) -> &'static [u8] {
    note.unwrap_or("").as_bytes()
}

struct State {
    list: List,
    heard: [Ap; MOST_HEARD],
    heard_count: usize,
    /// The file as read, for a save to change in place (wifilist.rs), and what's written.
    text: [u8; TEXT_MOST],
    out: [u8; TEXT_MOST],
    text_n: usize,
    /// Why the file can't be changed here.
    locked: Note,
    keys: Keys,
    board: u32,
    lsb: u32,
    /// The card answered (and needs card::finish() before leaving), and it's mounted.
    card_started: bool,
    card_ok: bool,
    clock_byte: u32,
    given_lsb: u32,
    /// What's being typed, ending in a zero.
    typed: [u8; PASS_MOST + 1],
    /// The layer keys last drawn while typing.
    type_shown: u32,
}

static STATE: Global<State> = Global::new(State {
    list: List::new(),
    heard: [Ap::new(); MOST_HEARD],
    heard_count: 0,
    text: [0; TEXT_MOST],
    out: [0; TEXT_MOST],
    text_n: 0,
    locked: None,
    keys: Keys { piano: 0, piano_pressed: 0, buttons: 0, buttons_pressed: 0 },
    board: 0,
    lsb: 0,
    card_started: false,
    card_ok: false,
    clock_byte: 0,
    given_lsb: 0,
    typed: [0; PASS_MOST + 1],
    type_shown: 0,
});
static VOLUME: Global<Volume> = Global::new(Volume::new());

struct App {
    s: &'static mut State,
    canvas: &'static mut Canvas,
    volume: &'static mut Volume,
    radio: Radio,
}

/// Typing: its page.
struct Typing {
    title: &'static [u8],
    network: [u8; SSID_MOST + 1],
    placeholder: &'static [u8],
    note: Note,
    most: usize,
    blink_ms: u32,
}

// ---- the clock ----

// The datasheet rates the AC7911B for 320 MHz. m-bootloader, as M-VAVE's FM-1 firmware up to V15, starts an app from
// the card at 360 (PLL1 2/3 of 540), and there, with the radio on, the key's hashing went wrong in the store
// experiment. V16 went back to 320. So from 360, the system clock moves to 2/3 of PLL0's 480 MHz, 320, with LSB 53, and
// back before returning.
const CLK_CON: Reg = Reg::at::<0x1000c>();
const CLK_SEL: Reg = Reg::at::<0x10014>();

fn clock_set(byte: u32) {
    CLK_CON.clear(bit(8));
    sync_cpu();
    CLK_SEL.write((CLK_SEL.read() & !255) | byte);
    sync_cpu();
    CLK_CON.set(bit(8));
    sync_cpu();
}

fn clock_320(s: &mut State, lsb_mhz: u32) -> u32 {
    s.given_lsb = lsb_mhz;
    s.clock_byte = CLK_SEL.read() & 255;
    if s.clock_byte != 6 {
        return lsb_mhz;
    }
    clock_set(2);
    hal_timer_clock(53);
    53
}

fn clock_back(s: &State) {
    if s.clock_byte != 6 {
        return;
    }
    clock_set(6);
    hal_timer_clock(s.given_lsb);
}

// ---- typing ----

fn name_ok(s: &[u8]) -> Note {
    if wifilist::ssid_ok(s) { None } else { Some("Type its name") }
}

/// Empty for an open network.
fn pass_ok(s: &[u8]) -> Note {
    let n = s.len();
    if n != 0 && n < PASS_LEAST {
        Some("At least 8 characters")
    } else if n > PASS_MOST {
        Some("At most 63 characters")
    } else {
        None
    }
}

fn ok_keys(board: u32) -> &'static [u8] {
    if board == 2 { b"SEQ PLAY OK" } else { b"PLAY OK" }
}

/// Joins, showing each step: why it failed, or None. r.step is where it got to.
fn test(radio: &mut Radio, canvas: &mut Canvas, ssid: &[u8], pass: &[u8], r: &mut Joined) -> Note {
    let why = radio.join(
        ssid,
        pass,
        |step| {
            screens::test(canvas, ssid, step, false, None, &Joined::default(), b"Testing", DIM, b"");
            canvas.send_all(&mut Plain);
        },
        r,
    );
    radio.leave();
    log!("test ", ssid, ": ", why.unwrap_or("joined"));
    why
}

/// A copy of a name, ending in a zero, that the list changing doesn't change.
fn name_copy(ssid: &[u8]) -> [u8; SSID_MOST + 1] {
    let mut out = [0u8; SSID_MOST + 1];
    let n = ssid.len().min(SSID_MOST);
    out[..n].copy_from_slice(&ssid[..n]);
    out
}

impl App {
    // ---- the card ----

    /// Reads WIFI.TXT, if there is one: what to say about it, or None.
    fn read_list(&mut self) -> Note {
        let s = &mut *self.s;
        s.list.clear();
        s.text_n = 0;
        s.locked = None;
        card::open(s.lsb);
        let sectors = card::start_card(s.lsb);
        s.card_started |= sectors != 0;
        let mut note = Some(NO_CARD);
        // Whether what the card holds is known: read, or surely not there. Only then can it be saved over.
        let mut known = false;
        if sectors != 0 {
            note = self.volume.mount(sectors).err();
            if note.is_none() {
                s.card_ok = true;
                let root = self.volume.root();
                let mut folder = root;
                // Read it, or know it isn't there: a failed look is neither, and saving then could lose the file.
                if self.volume.folder(&mut folder, FILE_NAME, "").is_ok() {
                    note = Some("WIFI.TXT is a folder");
                    s.locked = note;
                } else {
                    match self.volume.find(root, FILE_NAME) {
                        Err(_) => {
                            note = Some("WIFI.TXT couldn't be read");
                            s.locked = note;
                        }
                        Ok(None) => {
                            note = Some("No WIFI.TXT yet");
                            known = true;
                        }
                        Ok(Some(file)) => {
                            let n = (file.size as usize).min(TEXT_MOST);
                            if !self.volume.read(&file, 0, &mut s.text[..n]) {
                                note = Some("WIFI.TXT couldn't be read");
                                s.locked = note;
                            } else {
                                s.text_n = n;
                                s.list.read(&s.text[..n]);
                                known = true;
                                // Changing only the start of a file would lose the rest.
                                if file.size as usize > TEXT_MOST {
                                    note = Some("WIFI.TXT is too big to change here");
                                    s.locked = note;
                                }
                            }
                        }
                    }
                }
            }
        }
        card::close();
        if !known && s.locked.is_none() {
            s.locked = note;
        }
        if note.is_none() && s.list.skipped != 0 {
            note = Some("Some of WIFI.TXT was left out");
        }
        log!("WIFI.TXT: ", s.list.count, " networks, use ", s.list.in_use, ", ", s.list.skipped, " left out: ",
            note.unwrap_or("ok"));
        note
    }

    /// Writes the list to WIFI.TXT, the old one replaced only once the new one is whole: None, or what went wrong.
    /// Afterwards the list is what the card holds: the new one, or, if it failed, the old one read again.
    fn save_list(&mut self) -> Note {
        if self.s.locked.is_some() {
            return self.s.locked;
        }
        let s = &mut *self.s;
        let Some(n) = s.list.write(&s.text[..s.text_n], &mut s.out) else { return Some("Too much to save") };
        let whole = (n + 511) & !511;
        s.out[n..whole].fill(0);
        card::open(s.lsb);
        card::fast(s.lsb);
        // The card and the panel share SPI1: drawing steps off the card, and back on.
        let v = &mut *self.volume;
        let folder = v.root();
        let mut named = false;
        let mut error: Note = None;
        let mut found = false;
        let mut start = 0;
        match v.find(folder, FILE_NAME) {
            Ok(old) => found = old.is_some(),
            Err(e) => error = Some(e),
        }
        if error.is_none() {
            match v.reserve(n as u32) {
                Ok(at) => start = at,
                Err(e) => error = Some(e),
            }
        }
        if error.is_none() && whole != 0 && !v.write(start, 0, &s.out[..whole]) {
            // Nothing names the new chain yet.
            let _ = v.release(start);
            error = Some("The card didn't take it");
        } else if error.is_none() {
            // From here the directory may name the new chain, so it's never freed here: at worst, a failure leaves
            // clusters no file uses, which fsck can reclaim.
            error = if found {
                v.swap(folder, FILE_NAME, start, n as u32)
            } else {
                v.add(folder, FILE_NAME, ATTR_ARCHIVE, start, n as u32)
            }
            .err();
            named = error.is_none();
            if error.is_none() {
                error = v.commit().err();
            }
        }
        card::close();
        log!("saved WIFI.TXT, ", n, " bytes: ", error.unwrap_or("ok"));
        if named {
            // What's on the card now, and the list as read from it (use= with it).
            s.text[..n].copy_from_slice(&s.out[..n]);
            s.text_n = n;
            s.list.read(&s.text[..n]);
        } else if error.is_some() {
            self.read_list();
        }
        error
    }

    fn leave_card(&self) {
        card::open(self.s.lsb);
        card::finish();
        card::close();
    }

    // ---- keys ----

    /// Waits for a key, scanning the matrix. tick runs each poll with the ms since the last.
    fn next_press(&mut self, mut tick: impl FnMut(&mut App, u32)) {
        loop {
            hal_delay_us(POLL_US);
            app::feed();
            keys::scan(&mut self.s.keys);
            tick(self, POLL_US / 1000);
            if self.s.keys.piano_pressed != 0 || self.s.keys.buttons_pressed != 0 {
                return;
            }
        }
    }

    fn wait_press(&mut self) {
        self.next_press(|_, _| {});
    }

    fn pressed(&self, key: u32) -> bool {
        self.s.keys.buttons_pressed & key != 0
    }

    fn send_all(&mut self) {
        self.canvas.send_all(&mut Plain);
    }

    // ---- typing ----

    fn draw_type(&mut self, t: &Typing) {
        let k = self.s.keys;
        screens::typing(
            self.canvas,
            self.s.board,
            t.title,
            until_zero(&t.network),
            until_zero(&self.s.typed),
            t.placeholder,
            textentry::layer(k.buttons),
            k.piano,
            t.blink_ms < BLINK_MS,
            bytes(t.note),
        );
        self.send_all();
    }

    /// Redraws while the layer keys change, which are held rather than pressed, and blinks the cursor.
    fn type_tick(&mut self, t: &mut Typing, ms: u32) {
        let layers = self.s.keys.buttons & (OCT_DOWN | OCT_UP);
        let was = t.blink_ms < BLINK_MS;
        t.blink_ms = (t.blink_ms + ms) % (2 * BLINK_MS);
        if layers != self.s.type_shown || (t.blink_ms < BLINK_MS) != was {
            self.s.type_shown = layers;
            self.draw_type(t);
        }
    }

    /// Typing into typed, which holds the text to start from: false if cancelled. ok() says what's wrong, or None.
    fn type_text(
        &mut self,
        title: &'static [u8],
        network: &[u8],
        placeholder: &'static [u8],
        most: usize,
        ok: fn(&[u8]) -> Note,
    ) -> bool {
        let mut t = Typing { title, network: name_copy(network), placeholder, note: None, most, blink_ms: 0 };
        self.draw_type(&t);
        loop {
            self.next_press(|app, ms| app.type_tick(&mut t, ms));
            if self.pressed(REC) {
                return false;
            }
            t.note = None;
            if self.pressed(PLAY) {
                t.note = ok(until_zero(&self.s.typed));
                if t.note.is_none() {
                    return true;
                }
            }
            let k = self.s.keys;
            textentry::type_keys(&mut self.s.typed, t.most, k.piano_pressed, textentry::layer(k.buttons));
            t.blink_ms = 0;
            self.draw_type(&t);
        }
    }

    // ---- testing ----

    fn test(&mut self, ssid: &[u8], pass: &[u8], r: &mut Joined) -> Note {
        test(&mut self.radio, self.canvas, ssid, pass, r)
    }

    /// A test's result, shown until PLAY or REC.
    fn show_result(&mut self, ssid: &[u8], why: Note, r: &Joined, bar_top: &[u8], bar_colour: u16, bar_keys: &[u8]) {
        screens::test(self.canvas, ssid, r.step, true, why.map(str::as_bytes), r, bar_top, bar_colour, bar_keys);
        self.send_all();
        loop {
            self.wait_press();
            if self.pressed(PLAY | REC) {
                break;
            }
        }
    }

    // ---- adding, and changing a password ----

    fn put_and_save(&mut self, ssid: &[u8]) -> Note {
        let typed = self.s.typed;
        match self.s.list.put(ssid, until_zero(&typed)) {
            None => Some("The list is full"),
            Some(_) => self.save_list(),
        }
    }

    /// The password typed for ssid, tested, and saved: true once saved. In typed: the password to start from. An open
    /// network heard (ask false) has none to type.
    fn password_and_save(&mut self, ssid: &[u8], ask: bool, note: &mut Note) -> bool {
        let board = self.s.board;
        loop {
            textentry::all_symbols(true);
            if ask && !self.type_text(b"Password", ssid, b"None for an open network", PASS_MOST, pass_ok) {
                return false;
            }
            let mut r = Joined::default();
            let typed = self.s.typed;
            let why = self.test(ssid, until_zero(&typed), &mut r);
            if why.is_none() {
                let error = self.put_and_save(ssid);
                let said = error.unwrap_or("Saved, and in use");
                self.show_result(ssid, None, &r, said.as_bytes(), if error.is_some() { RED } else { GREEN }, ok_keys(board));
                *note = Some(said);
                return error.is_none();
            }
            let keys: &[u8] = match (ask, board == 2) {
                (true, true) => b"SEQ REC EDIT",
                (true, false) => b"REC EDIT",
                (false, true) => b"SEQ REC BACK",
                (false, false) => b"REC BACK",
            };
            let top: &[u8] = if board == 2 { b"SEQ PLAY: save it anyway" } else { b"PLAY: save it anyway" };
            self.show_result(ssid, why, &r, top, RED, keys);
            if self.pressed(PLAY) {
                let error = self.put_and_save(ssid);
                *note = Some(error.unwrap_or("Saved, not tested"));
                return error.is_none();
            }
            if !ask {
                return false;
            }
        }
    }

    /// Listens for networks, into heard.
    fn scan(&mut self) {
        self.canvas.forget();
        screens::scanning(self.canvas, 0, 13);
        self.send_all();
        let canvas = &mut *self.canvas;
        self.s.heard_count = self.radio.scan(&mut self.s.heard, |done, total| {
            screens::scanning(canvas, done, total);
            canvas.send(&mut Plain);
        });
    }

    /// Scans, and adds a network chosen from those heard or typed: what to say on the list, or None.
    fn add_network(&mut self) -> Note {
        if self.s.list.count == wifilist::MOST {
            return Some("Full: delete one first");
        }
        self.scan();
        let mut selection = 0;
        loop {
            let (count, items) = (self.s.heard_count, screens::heard_items(self.s.heard_count));
            if selection >= items {
                selection = items - 1;
            }
            screens::heard(self.canvas, &self.s.heard[..count], &self.s.list, self.s.board, selection);
            self.send_all();
            self.wait_press();
            if self.pressed(REC) {
                return None;
            }
            if self.pressed(PREV) {
                selection = if selection != 0 { selection - 1 } else { items - 1 };
            } else if self.pressed(NEXT) {
                selection = if selection + 1 < items { selection + 1 } else { 0 };
            } else if self.pressed(PLAY) {
                if selection == items - 1 {
                    return None;
                }
                let ssid;
                if selection == count {
                    self.s.typed[0] = 0;
                    textentry::all_symbols(true);
                    if !self.type_text(b"Network name", b"", b"As it's spelt", SSID_MOST, name_ok) {
                        continue;
                    }
                    ssid = name_copy(until_zero(&self.s.typed));
                } else {
                    if self.s.heard[selection].security == OTHER {
                        continue;
                    }
                    ssid = self.s.heard[selection].ssid;
                }
                let open = selection < count && self.s.heard[selection].security == OPEN;
                // Saved already: start from its password.
                let known = self.s.list.find(until_zero(&ssid));
                self.s.typed[0] = 0;
                if let Some(k) = known
                    && !open
                {
                    self.s.typed = self.s.list.net[k].pass;
                }
                let mut note = None;
                if self.password_and_save(until_zero(&ssid), !open, &mut note) {
                    return note;
                }
            }
        }
    }

    // ---- a saved network ----

    /// Held long enough, it's confirmed; let go sooner, it isn't.
    fn hold(&mut self, actions: &[u8], at: usize) -> bool {
        let mut shown = 0;
        let started = fm1_time::ticks();
        loop {
            let ms = fm1_time::ticks().wrapping_sub(started) / TICKS_PER_MS;
            if ms >= DELETE_HOLD_MS {
                return true;
            }
            app::feed();
            keys::scan(&mut self.s.keys);
            if self.s.keys.buttons & PLAY == 0 {
                return false;
            }
            let percent = ms * 100 / DELETE_HOLD_MS + 1;
            if percent >= shown + 4 {
                shown = percent;
                screens::network_hold(self.canvas, actions, at, percent);
                self.canvas.send(&mut Plain);
            }
            hal_delay_us(POLL_US / 2);
        }
    }

    /// A network's page: what to say on the list, or None.
    fn network_page(&mut self, index: usize) -> Note {
        let mut at = 0;
        let mut note: Note = None;
        let mut note_colour = DIM;
        loop {
            // A failed save reads the card again, which can change the list.
            if index >= self.s.list.count {
                return note;
            }
            let mut actions = [0u8; 4];
            let mut count = 0;
            let mut add = |a: u8| {
                actions[count] = a;
                count += 1;
            };
            add(screens::ACTION_TEST);
            if index != self.s.list.in_use {
                add(screens::ACTION_USE);
            }
            add(screens::ACTION_PASSWORD);
            add(screens::ACTION_DELETE);
            let actions = &actions[..count];
            if at >= count {
                at = count - 1;
            }
            screens::network(self.canvas, &self.s.list, index, self.s.board, actions, at, 0, bytes(note), note_colour);
            self.send_all();
            note = None;
            self.wait_press();
            if self.pressed(REC) {
                return None;
            }
            if self.pressed(PREV) {
                at = if at != 0 { at - 1 } else { count - 1 };
            } else if self.pressed(NEXT) {
                at = if at + 1 < count { at + 1 } else { 0 };
            } else if self.pressed(PLAY) {
                let net = self.s.list.net[index];
                match actions[at] {
                    screens::ACTION_TEST => {
                        let mut r = Joined::default();
                        let why = self.test(net.ssid(), net.pass(), &mut r);
                        let (top, colour): (&[u8], u16) = if why.is_some() { (b"Didn't join", RED) } else { (b"It works", GREEN) };
                        self.show_result(net.ssid(), why, &r, top, colour, ok_keys(self.s.board));
                    }
                    screens::ACTION_USE => {
                        self.s.list.in_use = index;
                        let error = self.save_list();
                        note = Some(error.unwrap_or("In use now"));
                        note_colour = if error.is_some() { RED } else { GREEN };
                    }
                    screens::ACTION_PASSWORD => {
                        self.s.typed = net.pass;
                        let mut saved = None;
                        if self.password_and_save(net.ssid(), true, &mut saved) {
                            return saved;
                        }
                    }
                    screens::ACTION_DELETE if self.hold(actions, at) => {
                        self.s.list.remove(index);
                        return Some(self.save_list().unwrap_or("Deleted"));
                    }
                    _ => {}
                }
            }
        }
    }

    // ---- the app ----

    /// No keys, for a test from a computer: what's heard, then each saved network joined, all in the log.
    fn autotest(&mut self) {
        self.scan();
        for ap in &self.s.heard[..self.s.heard_count] {
            log!("heard '", ap.ssid(), "': ", i32::from(ap.rssi), " dBm, channel ", ap.channel, ", security ", ap.security);
        }
        for i in 0..self.s.list.count {
            let net = self.s.list.net[i];
            let mut r = Joined::default();
            let why = self.test(net.ssid(), net.pass(), &mut r);
            log!("RESULT '", net.ssid(), "': ", why.unwrap_or("joined"), " (step ", r.step, ", ", i32::from(r.rssi),
                " dBm, ch ", r.channel, ", ip ", Hex(r.ip, 8), ", ", r.ms, " ms)");
            screens::test(self.canvas, net.ssid(), r.step, true, why.map(str::as_bytes), &r, b"Test", DIM, b"");
            self.send_all();
        }
    }

    /// Everything as it was found, before returning: the radio off, the card, the board's pins, the clock.
    fn finish(&mut self) {
        self.radio.off();
        if self.s.card_started {
            self.leave_card();
        }
        board_restore();
        clock_back(self.s);
    }

    fn run(&mut self) {
        let board = self.s.board;
        let mut note = self.read_list();
        let mut note_colour = if note == Some(NO_CARD) { RED } else { DIM };
        let mut selection = if self.s.list.count != 0 { self.s.list.in_use + 1 } else { screens::ADD };
        if cfg!(wifi_autotest) {
            self.autotest();
            return;
        }
        // The key that started this, still held, doesn't count.
        keys::scan(&mut self.s.keys);
        keys::scan(&mut self.s.keys);
        loop {
            let items = screens::list_items(&self.s.list);
            if selection >= items {
                selection = items - 1;
            }
            screens::list(self.canvas, &self.s.list, board, selection, bytes(note), note_colour);
            self.send_all();
            note = None;
            note_colour = DIM;
            self.wait_press();
            let back = self.pressed(REC);
            if self.pressed(PREV) {
                selection = if selection != 0 { selection - 1 } else { items - 1 };
            } else if self.pressed(NEXT) {
                selection = if selection + 1 < items { selection + 1 } else { 0 };
            } else if back || self.pressed(PLAY) {
                if back || selection == items - 1 {
                    break;
                }
                if !self.s.card_ok {
                    note = Some(NO_CARD);
                    note_colour = RED;
                    continue;
                }
                let before = self.s.list.count;
                if selection == screens::ADD {
                    note = self.add_network();
                    if self.s.list.count != before || note.is_some() {
                        selection = self.s.list.in_use + 1;
                    }
                } else {
                    note = self.network_page(selection - 1);
                }
                note_colour = match note {
                    Some(n) if n == "Deleted" || n.starts_with("Saved") => GREEN,
                    _ => DIM,
                };
                if self.s.list.count < before && selection > self.s.list.count {
                    selection = self.s.list.count;
                }
            }
        }
    }
}

fn main(lsb_mhz: u32) -> i32 {
    // Safety: each is taken here once, the only reference to it.
    let (s, canvas, volume, radio) = unsafe { (STATE.get(), canvas::claim(), VOLUME.get(), Radio::claim()) };
    s.lsb = clock_320(s, lsb_mhz);
    s.board = app::board();
    if s.board == 0 {
        // Its pins aren't known, so nothing is touched.
        log!("which board this is isn't known");
        clock_back(s);
        return 1;
    }
    log!("board ", s.board, ", LSB ", s.lsb, " MHz (given ", lsb_mhz, ")");
    // TIMER4, which the delete's hold and the cursor's blink read, may not be running yet.
    fm1_time::init();
    board_init(s.board == 2);
    keys::init(s.board == 2);
    display::init(s.board);
    canvas.forget();
    let mut app = App { s, canvas, volume, radio };
    app.run();
    app.finish();
    0
}
