// SPDX-License-Identifier: MIT
//! Keeps firmware and its settings on the card, and swaps between them, with no computer (slotfile.rs). Slots are named
//! on the piano keys (kit/textentry.rs). It writes flash, so after a load it resets, and doesn't return.
#![no_std]
use kit::board::{board_init, board_restore};
use kit::canvas::{self, Canvas, GREEN, ORANGE, RED};
use kit::fat::{Error, until_zero};
use kit::hw::{Reg, sync_cpu};
use kit::keys::{self, Keys};
use kit::mboot::formats::{self, App, AppState, Flash, Header, Work};
use kit::platform::{Chip, flash_init, flash_jedec, flash_uid, hal_delay_us, hal_efuse, hal_reset};
use kit::{Global, app, card, display, log, textentry};

mod screens;
mod slotfile;

use screens::{Action, SCREEN_SAVE};
use slotfile::{SLOT_NAME_MOST, Slots};

const POLL_US: u32 = 10_000;
/// How long PLAY is held to confirm: a load can be undone by loading back, a delete can't.
const LOAD_HOLD_MS: u32 = 1000;
const DELETE_HOLD_MS: u32 = 2000;
const BLINK_MS: u32 = 500;
const NO_CARD: Error = "No card found";

/// Milliseconds, from the 24 MHz timer the player and usbdisk use. It wraps every 179 s, so differences only.
const TIMER_CON: Reg = Reg::at::<0x10800>();
const TIMER_COUNT: Reg = Reg::at::<0x10804>();
const TIMER_PERIOD: Reg = Reg::at::<0x10808>();
const TICKS_PER_MS: u32 = 24_000;

static SLOTS: Global<Slots> = Global::new(Slots::new());
static WORK: Global<Work> = Global::new([0; formats::WORK_SIZE]);
static CHECKED: Global<App> = Global::new(App::EMPTY);

kit::app!(main);

fn timer_start() {
    if TIMER_CON.read() & 0xf == (2 << 2) | 1 {
        return;
    }
    TIMER_CON.write(0);
    TIMER_PERIOD.write(0xffff_ffff);
    TIMER_COUNT.write(0);
    TIMER_CON.write((2 << 2) | 1 | (1 << 14));
}

/// The card and the panel share SPI1: drawing steps off the card, and back on.
fn card_on(lsb: u32) {
    card::open(lsb);
    card::fast(lsb);
}

/// What a send does around it: steps off the card and back on, while working on the card, and otherwise nothing. One
/// type for both, so the canvas's sending is built once.
struct Sending {
    on_card: bool,
    lsb: u32,
}

impl Sending {
    const fn plain() -> Sending {
        Sending { on_card: false, lsb: 0 }
    }
}

impl canvas::Hooks for Sending {
    fn before(&mut self) {
        if self.on_card {
            card::close();
        }
    }

    fn after(&mut self) {
        if self.on_card {
            card_on(self.lsb);
        }
    }
}

/// An image, for the bootloader's check, through the one type it's built for.
struct Image<'a>(&'a dyn Flash);

impl Flash for Image<'_> {
    fn capacity(&self) -> u32 {
        self.0.capacity()
    }

    fn read_inside(&self, address: u32, dst: &mut [u8]) -> bool {
        self.0.read_inside(address, dst)
    }
}

/// The screen, and what slotfile asks of the app.
struct Ui {
    canvas: &'static mut Canvas,
    lsb: u32,
    chip_key: u16,
    checked: &'static mut App,
    work: &'static mut Work,
    working: Option<&'static str>,
    work_shown: u32,
}

impl slotfile::Hooks for Ui {
    /// What slotfile is doing, in words, and how far it's got. It's only asked while on the card.
    fn progress(&mut self, what: &'static str, done: u32, total: u32) {
        app::feed();
        if self.working != Some(what) {
            self.working = Some(what);
            let writing = what == "WRITING";
            let title: &[u8] = match what {
                "SAVING" => b"Saving what's on the unit",
                "CHECKING" => b"Checking what was saved",
                "CHECKING THE SLOT" => b"Checking the slot",
                "WRITING" => b"Loading the slot",
                _ => what.as_bytes(),
            };
            let line: &[u8] = if writing { b"Writing the firmware and its settings" } else { b"On the card" };
            screens::screen_work(self.canvas, title, line, writing);
            self.canvas.send_all(&mut Sending { on_card: true, lsb: self.lsb });
            self.work_shown = 0;
        }
        let percent = if total != 0 { done * 100 / total } else { 0 };
        if percent != self.work_shown {
            self.work_shown = percent;
            screens::screen_work_progress(self.canvas, done, total);
            self.canvas.send(&mut Sending { on_card: true, lsb: self.lsb });
        }
    }

    /// The same check the bootloader makes before starting firmware.
    fn valid(&mut self, image: &dyn Flash) -> bool {
        let (image, mut header) = (Image(image), Header::EMPTY);
        formats::app_mount(&image, self.chip_key, self.checked, &mut header)
            && formats::app_check(&image, self.checked, self.work) == AppState::Ok
    }
}

/// The app's state between screens.
struct State {
    ui: Ui,
    slots: &'static mut Slots,
    board: u32,
    keys: Keys,
    typed: [u8; SLOT_NAME_MOST + 1],
    placeholder: [u8; SLOT_NAME_MOST + 1],
    name_note: &'static [u8],
    blink_ms: u32,
    layers_shown: u32,
}

impl State {
    fn lsb(&self) -> u32 {
        self.ui.lsb
    }

    /// Waits for a key, scanning the matrix. `tick`, if true, runs the naming page's tick each poll.
    fn next_press(&mut self, tick: bool) {
        loop {
            hal_delay_us(POLL_US);
            app::feed();
            keys::scan(&mut self.keys);
            if tick {
                self.name_tick(POLL_US / 1000);
            }
            if self.keys.piano_pressed != 0 || self.keys.buttons_pressed != 0 {
                return;
            }
        }
    }

    /// Errors come from below in capitals; the screen has room for them as they are.
    fn open_slots(&mut self, here: &mut Option<usize>) -> Option<Error> {
        let lsb = self.lsb();
        card::open(lsb);
        let sectors = card::start_card(lsb);
        let mut error = Some(NO_CARD);
        let mut uid = [0u8; 8];
        flash_uid(&mut uid);
        if sectors != 0 {
            error = self.slots.open(sectors, &uid, flash_jedec(), self.board).err();
            if error.is_none() {
                *here = self.slots.on_unit(&Chip);
            }
        }
        card::close();
        error
    }

    /// Runs work on the card, with the screen able to show its progress.
    fn on_card(&mut self, work: impl FnOnce(&mut Slots, &mut Ui, &[u8]) -> Result<(), Error>) -> Option<Error> {
        card_on(self.lsb());
        self.ui.working = None;
        let error = work(self.slots, &mut self.ui, until_zero(&self.typed)).err();
        card::close();
        error
    }

    fn find_here(&mut self) -> Option<usize> {
        card_on(self.lsb());
        let here = self.slots.on_unit(&Chip);
        card::close();
        here
    }

    fn leave_card(&self) {
        card::open(self.lsb());
        card::finish();
        card::close();
    }

    // ---- naming ----

    fn draw_name(&mut self) {
        let layer = textentry::layer(self.keys.buttons);
        let ui = &mut self.ui;
        screens::screen_name(ui.canvas, self.board, until_zero(&self.typed), until_zero(&self.placeholder), layer,
            self.keys.piano, self.blink_ms < BLINK_MS, self.name_note);
        ui.canvas.send_all(&mut Sending::plain());
    }

    /// Redraws while the layer keys change, which are held rather than pressed, and blinks the cursor.
    fn name_tick(&mut self, ms: u32) {
        let layers = self.keys.buttons & (keys::OCT_DOWN | keys::OCT_UP);
        let was = self.blink_ms < BLINK_MS;
        self.blink_ms = (self.blink_ms + ms) % (2 * BLINK_MS);
        if layers != self.layers_shown || (self.blink_ms < BLINK_MS) != was {
            self.layers_shown = layers;
            self.draw_name();
        }
    }

    /// A name typed on the keys into `typed`, or false if cancelled. Left empty, it's the placeholder.
    fn ask_name(&mut self) -> bool {
        self.typed[0] = 0;
        self.name_note = b"";
        self.slots.free_name(b"Slot", &mut self.placeholder);
        self.draw_name();
        loop {
            self.next_press(true);
            if self.keys.buttons_pressed & keys::REC != 0 {
                return false;
            }
            self.name_note = b"";
            if self.keys.buttons_pressed & keys::PLAY != 0 {
                if self.typed[0] == 0 {
                    self.typed = self.placeholder;
                }
                if slotfile::name_ok(until_zero(&self.typed)) {
                    return true;
                }
                self.name_note = b"A name can't end in a space or a dot";
            }
            textentry::type_keys(&mut self.typed, SLOT_NAME_MOST, self.keys.piano_pressed, textentry::layer(self.keys.buttons));
            self.blink_ms = 0;
            self.draw_name();
        }
    }

    // ---- a slot's page ----

    /// Held long enough, it's confirmed; let go sooner, it isn't.
    fn hold(&mut self, actions: &[Action], at: usize) -> bool {
        let needed = if actions[at] == Action::Delete { DELETE_HOLD_MS } else { LOAD_HOLD_MS };
        let mut shown = 0;
        let started = TIMER_COUNT.read();
        loop {
            let ms = TIMER_COUNT.read().wrapping_sub(started) / TICKS_PER_MS;
            if ms >= needed {
                return true;
            }
            app::feed();
            keys::scan(&mut self.keys);
            if self.keys.buttons & keys::PLAY == 0 {
                return false;
            }
            let percent = ms * 100 / needed + 1;
            if percent >= shown + 4 {
                shown = percent;
                screens::screen_slot_hold(self.ui.canvas, actions, at, percent);
                self.ui.canvas.send(&mut Sending::plain());
            }
            hal_delay_us(POLL_US / 2);
        }
    }

    /// What to do with a slot.
    fn slot_page(&mut self, index: usize, here: bool, pending: bool) -> Action {
        let mut actions = [Action::Back; 3];
        let mut count = 0;
        if !here {
            actions[count] = Action::Load;
            count += 1;
        }
        actions[count] = Action::Delete;
        actions[count + 1] = Action::Back;
        count += 2;
        let actions = &actions[..count];
        let mut at = 0;
        loop {
            let slot = &self.slots.list[index];
            screens::screen_slot(self.ui.canvas, slot, self.board, here, pending, actions, at, 0);
            self.ui.canvas.send_all(&mut Sending::plain());
            self.next_press(false);
            let pressed = self.keys.buttons_pressed;
            if pressed & keys::REC != 0 {
                return Action::Back;
            }
            if pressed & keys::PREV != 0 {
                at = if at != 0 { at - 1 } else { count - 1 };
            } else if pressed & keys::NEXT != 0 {
                at = if at + 1 < count { at + 1 } else { 0 };
            } else if pressed & keys::PLAY != 0 {
                if actions[at] == Action::Back {
                    return Action::Back;
                }
                if self.hold(actions, at) {
                    return actions[at];
                }
            }
        }
    }
}

fn main(lsb_mhz: u32) -> i32 {
    timer_start();
    let board = app::board();
    if board == 0 {
        // Its pins aren't known, so nothing is touched.
        log!("which board this is isn't known");
        return 1;
    }
    // Safety: each is taken once, here, and only through State from now on.
    let (canvas, slots, work, checked) = unsafe { (canvas::claim(), SLOTS.get(), WORK.get(), CHECKED.get()) };
    let ui = Ui { canvas, lsb: lsb_mhz, chip_key: 0, checked, work, working: None, work_shown: 0 };
    let mut s = State {
        ui,
        slots,
        board,
        keys: Keys::default(),
        typed: [0; SLOT_NAME_MOST + 1],
        placeholder: [0; SLOT_NAME_MOST + 1],
        name_note: b"",
        blink_ms: 0,
        layers_shown: 0,
    };
    board_init(board == 2);
    keys::init(board == 2);
    display::init(board);
    s.ui.canvas.forget();
    screens::screen_work(s.ui.canvas, b"Reading the card", b"", false);
    s.ui.canvas.send_all(&mut Sending::plain());
    if !flash_init(1, 1) {
        screens::screen_work(s.ui.canvas, b"Flash error", b"The unit's flash didn't answer", true);
        s.ui.canvas.send_all(&mut Sending::plain());
        hal_delay_us(5_000_000);
        board_restore();
        return 1;
    }
    s.ui.chip_key = (hal_efuse(1, 3) >> 8) as u16;
    let mut here = None;
    let mut note = s.open_slots(&mut here);
    let card_ok = note != Some(NO_CARD);
    let mut note_colour = RED;
    if note.is_none() && s.slots.pending().is_some() {
        note = Some("The last load stopped");
        note_colour = ORANGE;
    }
    if let Some(n) = note {
        log!(n);
    }
    log!(s.slots.count, " slots, on the unit: ", here.map_or(-1, |h| h as i32));
    let mut pending = s.slots.pending();
    let mut selection = pending.or(here).map_or(0, |i| i + 1);
    // The key that started this, still held, doesn't count.
    keys::scan(&mut s.keys);
    keys::scan(&mut s.keys);
    loop {
        let items = screens::screen_items(s.slots);
        if selection >= items {
            selection = items - 1;
        }
        pending = s.slots.pending();
        screens::screen_list(s.ui.canvas, s.slots, board, selection, here, pending, note.unwrap_or("").as_bytes(),
            note_colour);
        s.ui.canvas.send_all(&mut Sending::plain());
        note = None;
        note_colour = RED;
        s.next_press(false);
        let pressed = s.keys.buttons_pressed;
        let back = pressed & keys::REC != 0;
        if pressed & keys::PREV != 0 {
            selection = if selection != 0 { selection - 1 } else { items - 1 };
        } else if pressed & keys::NEXT != 0 {
            selection = if selection + 1 < items { selection + 1 } else { 0 };
        } else if back || pressed & keys::PLAY != 0 {
            if back || selection == items - 1 {
                if card_ok {
                    s.leave_card();
                }
                board_restore();
                return 0;
            }
            if !card_ok {
                continue;
            }
            if selection == SCREEN_SAVE {
                if !s.ask_name() {
                    continue;
                }
                note = s.on_card(|slots, ui, typed| slots.save_new(&Chip, ui, typed));
                log!("save: ", note.unwrap_or("ok"));
                if note.is_none() {
                    here = s.find_here();
                    note = Some("Saved");
                    note_colour = GREEN;
                    selection = here.map_or(0, |h| h + 1);
                }
                continue;
            }
            let index = selection - 1;
            let action = s.slot_page(index, Some(index) == here, Some(index) == pending);
            if action == Action::Delete {
                note = s.on_card(|slots, _, _| slots.delete(index));
                log!("delete: ", note.unwrap_or("ok"));
                if note.is_none() {
                    note = Some("Deleted");
                    note_colour = GREEN;
                }
                here = s.find_here();
            } else if action == Action::Load {
                // The guard stays on: each sector feeds it, so only a real hang reaches it, and it ends in download mode
                // rather than a unit that needs switching off. Off only for the reset, which should start the firmware.
                note = s.on_card(|slots, ui, _| slots.load(&Chip, ui, index));
                log!("load: ", note.unwrap_or("ok"));
                if note.is_none() {
                    s.leave_card();
                    screens::screen_work(s.ui.canvas, b"Done", b"Starting the firmware", false);
                    s.ui.canvas.send_all(&mut Sending::plain());
                    hal_delay_us(1_000_000);
                    board_restore();
                    app::guard(false);
                    sync_cpu();
                    hal_reset();
                }
                // It may have saved, so the list is read again. It's safe to try the load again from here.
                here = s.find_here();
            }
        }
    }
}
