// SPDX-License-Identifier: MIT
//! Plays the MP3 files in the card's root, one after another, polled, in the look of a 90s desktop player (ui.rs).
//! OCT- and OCT+ (< and > on the SMK-37) move through the playlist, and a track moved to starts at once; held while
//! playing, they rewind and fast-forward. PLAY starts and pauses, and on "back" it leaves, as REC (SEQ REC) does from
//! anywhere.
//!
//! Decoded sound goes into a ring of about 190 ms, and the DMA's half buffers are filled from it (pump()) between every
//! card read, decode and piece of the screen. Decoding keeps the ring full, and the screen is drawn when it is, so
//! nothing has to fit inside one half buffer's 26 ms.
#![no_std]
// The decoder, the ring and the analyser are optimised for speed, as the C was -O2 (app.mk).
#![cfg_attr(target_arch = "pi32v2", feature(optimize_attribute))]
mod minimp3;
mod spectrum;
mod ui;

use kit::audio::{self, Audio, HALF_FRAMES, RATE_DEN, RATE_NUM};
use kit::board::{KEY_NEXT, KEY_PREV, KEY_RECOVERY, KEY_SELECT, board_init, board_keys, board_restore};
use kit::canvas::{Hooks, Plain};
use kit::fat::{Entry, Named, Volume};
use kit::hw::Reg;
use kit::pixel::{self, Pixel};
use kit::platform::hal_delay_us;
use kit::{Aligned, Global, app, card, display, log};
use minimp3::{FrameInfo, MAX_SAMPLES_PER_FRAME, Mp3Dec, mp3dec_decode_frame, mp3dec_init};
use ui::{Model, NAME_BYTES, Names, PAUSED, PLAYING, STOPPED, Ui};

const MOST_FILES: usize = 64;
const CHUNK: usize = 4096;
const INPUT: usize = 16 * 1024;
/// Samples are 16-bit; the DAC takes 24.
const SAMPLE_SHIFT: u32 = 7;
const TIMER_CON: Reg = Reg::at::<0x10800>();
const TIMER_COUNT: Reg = Reg::at::<0x10804>();
const TIMER_PERIOD: Reg = Reg::at::<0x10808>();
const TICKS_PER_US: u32 = 24;
const TICKS_PER_MS: u32 = 24000;
const REPEAT_AFTER_MS: u32 = 400;
const REPEAT_EVERY_MS: u32 = 120;
/// Holding OCT-/OCT+ while playing jumps this far, this often: about thirteen times as fast.
const SEEK_STEP_S: u32 = 2;
const SEEK_EVERY_MS: u32 = 150;
/// PREV goes back to the start of the track after this long, and to the one before it sooner.
const RESTART_AFTER_S: u32 = 3;
const IDLE_POLL_US: u32 = 10000;
/// The ring, in stereo frames: a power of two.
const RING: u32 = 8192;
/// The most one decoded frame can make: 1152 samples, or 576 at 8 kHz made into 44.1 kHz.
const MOST_PER_FRAME: u32 = 3200;
/// How often the screen and keys are looked at while playing.
const PLAY_UI_MS: u32 = 15;
/// What the screen may send at a time while playing: about 10 ms at 96 MHz, then decoding catches up.
const PLAY_SCREEN_BYTES: u32 = 12288;

// What's big, as C had it: each all zeros, so none of it is in the file.
static VOLUME: Global<Volume> = Global::new(Volume::new());
static FILES: Global<[Named; MOST_FILES]> = Global::new([Named::empty(); MOST_FILES]);
static DECODER: Global<Mp3Dec> = Global::new(Mp3Dec::new());
static CHUNK_BUFFER: Global<Aligned<CHUNK>> = Global::new(Aligned([0; CHUNK]));
static INPUT_BUFFER: Global<[u8; INPUT]> = Global::new([0; INPUT]);
static PCM: Global<[i16; MAX_SAMPLES_PER_FRAME]> = Global::new([0; MAX_SAMPLES_PER_FRAME]);
static RING_BUFFER: Global<[[i16; 2]; RING as usize]> = Global::new([[0; 2]; RING as usize]);

kit::app!(main);

#[derive(Clone, Copy, Default)]
struct Stats {
    frames: u32,
    most_decode: u32,
    most_read: u32,
    most_draw: u32,
    idle: u32,
    busy: u32,
}

fn timer_start() {
    if TIMER_CON.read() & 0xf == (2 << 2) | 1 {
        return;
    }
    TIMER_CON.write(0);
    TIMER_PERIOD.write(u32::MAX);
    TIMER_COUNT.write(0);
    TIMER_CON.write((2 << 2) | 1 | (1 << 14));
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

/// The ring and the sound it goes to.
struct Sound {
    audio: Option<Audio>,
    /// Written at head, read at tail, both counting up for ever.
    ring: &'static mut [[i16; 2]; RING as usize],
    head: u32,
    tail: u32,
    /// Muted while paused. Primed once a half's worth has come after a clear, so the gap while a track starts isn't
    /// counted as running short.
    muted: bool,
    primed: bool,
    short_halves: u32,
}

impl Sound {
    fn level(&self) -> u32 {
        self.head.wrapping_sub(self.tail)
    }

    fn clear(&mut self) {
        self.tail = self.head;
        self.primed = false;
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn put(&mut self, left: i32, right: i32) {
        self.ring[(self.head % RING) as usize] = [left as i16, right as i16];
        self.head = self.head.wrapping_add(1);
    }

    /// Fills a free half buffer, if there is one, from the ring; or with silence while paused or stopped. A ring that
    /// runs short is counted while playing, and the rest of that half is silent.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn pump(&mut self, playing: bool) {
        let level = self.level() as usize;
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let Some(which) = audio.free_half() else {
            return;
        };
        let mut n = if self.muted { 0 } else { level };
        if n >= HALF_FRAMES {
            n = HALF_FRAMES;
            self.primed = true;
        } else if !self.muted && self.primed && playing {
            self.short_halves += 1;
        }
        let half = audio.half(which);
        let (sound, silence) = half.split_at_mut(2 * n);
        for frame in sound.chunks_exact_mut(2) {
            let [left, right] = self.ring[(self.tail % RING) as usize];
            frame[0] = i32::from(left) << SAMPLE_SHIFT;
            frame[1] = i32::from(right) << SAMPLE_SHIFT;
            self.tail = self.tail.wrapping_add(1);
        }
        silence.fill(0);
        audio.filled();
    }
}

/// What's done around sending the screen while playing: the card and the screen share SPI1, and the sound has to
/// keep going.
struct Playing<'a> {
    sound: &'a mut Sound,
    lsb: u32,
    playing: bool,
}

impl Hooks for Playing<'_> {
    fn before(&mut self) {
        card_off();
    }

    fn after(&mut self) {
        card_on(self.lsb);
    }

    fn between(&mut self) {
        self.sound.pump(self.playing);
    }
}

/// The card and the screen share SPI1.
fn card_off() {
    card::close();
}

fn card_on(lsb: u32) {
    card::open(lsb);
    card::fast(lsb);
}

/// The names for the screen, without the .MP3. The font has ASCII, so anything past it shows as '?'.
impl<const N: usize> Names for [Named; N] {
    fn name(&self, track: u32, out: &mut [u8; NAME_BYTES]) -> usize {
        let mut n = 0;
        for &b in self[track as usize].name() {
            if n + 1 >= NAME_BYTES {
                break;
            }
            if b < 0x80 {
                out[n] = b;
                n += 1;
            } else if b >= 0xc0 {
                // the first byte of a character
                out[n] = b'?';
                n += 1;
            }
        }
        if n >= 4 && out[n - 4] == b'.' && out[n - 3] | 32 == b'm' && out[n - 2] | 32 == b'p' && out[n - 1] == b'3' {
            n -= 4;
        }
        n
    }
}

// Keys, as events. Stopped, OCT-/OCT+ move when pressed, and repeat when held. Playing, a tap moves when let go, and
// holding seeks instead. PLAY counts when pressed.
const EV_PREV: u32 = 1;
const EV_NEXT: u32 = 2;
const EV_SELECT: u32 = 4;
const EV_REWIND: u32 = 8;
const EV_FORWARD: u32 = 16;
const EV_LEAVE: u32 = 32;

struct Keys {
    held: u32,
    repeat_at: u32,
    long_press: bool,
}

/// A VBR file says its frame count in its first frame. Otherwise, the size and the bitrate say about how long it is.
fn frames_said(frame: &[u8]) -> u32 {
    for at in [36, 21, 13] {
        if at + 12 > frame.len() || (&frame[at..at + 4] != b"Xing" && &frame[at..at + 4] != b"Info") {
            continue;
        }
        if frame[at + 7] & 1 != 0 {
            return u32::from_be_bytes([frame[at + 8], frame[at + 9], frame[at + 10], frame[at + 11]]);
        }
    }
    0
}

#[derive(Clone, Copy, PartialEq)]
enum Play {
    End,
    Switch,
    Leave,
    Failed,
}

struct Player {
    lsb: u32,
    volume: &'static mut Volume,
    files: &'static [Named],
    decoder: &'static mut Mp3Dec,
    chunk: &'static mut Aligned<CHUNK>,
    input: &'static mut [u8; INPUT],
    pcm: &'static mut [i16; MAX_SAMPLES_PER_FRAME],
    pix: &'static mut Pixel,
    sound: Sound,
    clock: Clock,
    keys: Keys,
    stats: Stats,
    ui: Ui,
    ui_last_ms: u32,
    model: Model<'static>,
    /// Whether sending the screen has to keep the sound going (play_from()).
    playing_hooks: bool,
}

impl Player {
    fn count(&self) -> u32 {
        self.files.len() as u32
    }

    fn pump(&mut self) {
        self.sound.pump(self.model.mode == PLAYING);
    }

    fn ui_now(&mut self) {
        let now = self.clock.now_ms();
        let ms = now.wrapping_sub(self.ui_last_ms);
        if self.playing_hooks {
            let mut hooks = Playing { sound: &mut self.sound, lsb: self.lsb, playing: self.model.mode == PLAYING };
            self.ui.update(self.pix, &self.model, ms, &mut hooks);
        } else {
            self.ui.update(self.pix, &self.model, ms, &mut Plain);
        }
        self.ui_last_ms = now;
    }

    fn read_files(&mut self, files: &'static mut [Named; MOST_FILES]) -> Option<&'static str> {
        card::open(self.lsb);
        let sectors = card::start_card(self.lsb);
        let mut error = Some("NO CARD FOUND");
        let mut count = 0;
        if sectors != 0 {
            error = match self.volume.mount(sectors) {
                Err(e) => Some(e),
                Ok(()) => {
                    let root = self.volume.root();
                    match self.volume.list_named(root, b"MP3", files) {
                        Err(e) => Some(e),
                        Ok((n, _more)) => {
                            count = n;
                            if n == 0 { Some("NO MP3 FILES") } else { None }
                        }
                    }
                }
            }
        }
        card::close();
        let files: &'static [Named; MOST_FILES] = files;
        self.files = &files[..count];
        self.model.names = files;
        for f in self.files {
            log!(f.name(), " ", f.file.size);
        }
        error
    }

    fn keys_reset(&mut self) {
        self.keys.held = board_keys();
        self.keys.long_press = true; // a key held from before doesn't count
    }

    fn key_events(&mut self, playing: bool) -> u32 {
        const KEY: [u32; 2] = [KEY_PREV, KEY_NEXT];
        const MOVE: [u32; 2] = [EV_PREV, EV_NEXT];
        const SEEK: [u32; 2] = [EV_REWIND, EV_FORWARD];
        let now = board_keys();
        let keys = &mut self.keys;
        let (pressed, released) = (now & !keys.held, keys.held & !now);
        let mut ev = 0;
        let t = self.clock.now_ms();
        for i in 0..2 {
            if pressed & KEY[i] != 0 {
                keys.long_press = false;
                keys.repeat_at = t.wrapping_add(REPEAT_AFTER_MS);
                if !playing {
                    ev |= MOVE[i];
                }
            } else if now & KEY[i] != 0 && t.wrapping_sub(keys.repeat_at) as i32 >= 0 {
                keys.long_press = true;
                ev |= if playing { SEEK[i] } else { MOVE[i] };
                keys.repeat_at = t.wrapping_add(if playing { SEEK_EVERY_MS } else { REPEAT_EVERY_MS });
            } else if released & KEY[i] != 0 && playing && !keys.long_press {
                ev |= MOVE[i];
            }
        }
        if pressed & KEY_SELECT != 0 {
            ev |= EV_SELECT;
        }
        if pressed & KEY_RECOVERY != 0 {
            ev |= EV_LEAVE;
        }
        keys.held = now;
        self.model.keys = now;
        ev
    }

    /// The playlist's cursor, moved while playing. A different track starts at once; "back" changes nothing until
    /// PLAY.
    fn move_cursor(&mut self, ev: u32) -> Option<Play> {
        if ev & (EV_PREV | EV_NEXT) == 0 {
            return None;
        }
        let (at, count) = (self.model.selected, self.count());
        if ev & EV_PREV != 0 && at as i32 == self.model.playing && self.model.elapsed >= RESTART_AFTER_S {
            return Some(Play::Switch); // back to the start of this one
        }
        self.model.selected = if ev & EV_PREV != 0 {
            if at != 0 { at - 1 } else { count }
        } else if at < count {
            at + 1
        } else {
            0
        };
        if self.model.selected < count && self.model.selected as i32 != self.model.playing {
            Some(Play::Switch)
        } else {
            None
        }
    }

    /// Paused: silence goes out, what's in the ring waits, the time blinks, and the cursor still moves. None goes on
    /// playing.
    fn pause(&mut self) -> Option<Play> {
        self.sound.muted = true;
        self.model.mode = PAUSED;
        loop {
            self.pump();
            let ev = self.key_events(true);
            if ev & EV_LEAVE != 0 {
                return Some(Play::Leave);
            }
            if ev & EV_SELECT != 0 {
                if self.model.selected == self.count() {
                    return Some(Play::Leave);
                }
                self.sound.muted = false;
                self.model.mode = PLAYING;
                return None;
            }
            if let Some(action) = self.move_cursor(ev) {
                self.sound.muted = false;
                return Some(action);
            }
            self.ui_now();
            app::feed();
            hal_delay_us(IDLE_POLL_US);
        }
    }

    /// Reads and decodes a track into the ring, with the audio running, until it ends or a key says otherwise. The
    /// end of one track runs into the next, as the ring keeps what's left of it.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn play(&mut self, track: u32, error: &mut Option<&'static str>) -> Play {
        let file: Entry = self.files[track as usize].file;
        let (mut offset, mut have, mut step, mut position, mut samples) = (0u32, 0u32, 0u32, 0u32, 0u32);
        let (mut total_frames, mut first_frame, mut bytes_per_s) = (0u32, 0u32, 0u32);
        let mut hz = 0u32;
        let (mut before_left, mut before_right) = (0i32, 0i32);
        let mut first = true;
        let mut result = Play::End;
        self.stats = Stats::default();
        self.sound.short_halves = 0;
        self.decoder.clear();
        mp3dec_init(self.decoder);
        self.model.mode = PLAYING;
        self.model.playing = track as i32;
        self.model.elapsed = 0;
        self.model.total = 0;
        self.model.kbps = 0;
        self.model.hz = 0;
        self.model.channels = 0;
        *error = None;
        card_on(self.lsb);
        let started = TIMER_COUNT.read();
        let mut ui_at = self.clock.now_ms();
        loop {
            self.pump();
            if RING - self.sound.level() >= MOST_PER_FRAME {
                let a = TIMER_COUNT.read();
                // Before a frame could run off the end of what's read.
                while have < (INPUT - CHUNK) as u32 && offset < file.size {
                    let n = (file.size - offset).min(CHUNK as u32) as usize;
                    if !self.volume.read(&file, offset, &mut self.chunk.0[..n]) {
                        *error = Some("CARD READ ERROR");
                        break;
                    }
                    self.input[have as usize..have as usize + n].copy_from_slice(&self.chunk.0[..n]);
                    have += n as u32;
                    offset += n as u32;
                    self.pump();
                }
                if error.is_some() {
                    result = Play::Failed;
                    break;
                }
                let b = TIMER_COUNT.read();
                // With no frame found, as in a tag bigger than input, it only sets frame_bytes.
                let mut info = FrameInfo::default();
                let decoded =
                    mp3dec_decode_frame(self.decoder, &self.input[..have as usize], Some(&mut *self.pcm), &mut info)
                        as u32;
                let c = TIMER_COUNT.read();
                self.pump();
                if info.frame_bytes == 0 {
                    break;
                }
                let (frame_offset, frame_bytes) = (info.frame_offset as u32, info.frame_bytes as u32);
                if first && info.hz != 0 {
                    total_frames = frames_said(&self.input[frame_offset as usize..frame_bytes as usize]);
                    first_frame = offset - have + frame_offset;
                }
                have -= frame_bytes;
                self.input.copy_within(frame_bytes as usize..(frame_bytes + have) as usize, 0);
                self.stats.most_read = self.stats.most_read.max(b.wrapping_sub(a) / TICKS_PER_US);
                self.stats.most_decode = self.stats.most_decode.max(c.wrapping_sub(b) / TICKS_PER_US);
                app::feed();
                if first && info.hz != 0 {
                    first = false;
                    hz = info.hz as u32;
                    let per_frame = if decoded != 0 { decoded } else { 1152 };
                    let kbps = info.bitrate_kbps as u32;
                    let seconds = if total_frames != 0 {
                        total_frames.wrapping_mul(per_frame) / hz
                    } else if kbps != 0 {
                        (file.size - offset + have) / (kbps * 125)
                    } else {
                        0
                    };
                    self.model.total = seconds;
                    self.model.hz = hz;
                    self.model.channels = info.channels as u32;
                    // For seeking: about where a second starts, from the length if known, or else the bitrate.
                    bytes_per_s = if seconds != 0 { (file.size - first_frame) / seconds } else { kbps * 125 };
                    log!(
                        hz,
                        " Hz, ",
                        info.channels,
                        " channels, ",
                        info.bitrate_kbps,
                        " kbps, ",
                        self.model.total,
                        " s, ",
                        total_frames,
                        " frames said"
                    );
                    // In 1/65536ths of a frame, from the rate the DAC really runs at.
                    step = if hz == 44100 {
                        0
                    } else {
                        ((u64::from(hz) * u64::from(RATE_DEN) << 16) / u64::from(RATE_NUM)) as u32
                    };
                }
                if decoded == 0 {
                    continue;
                }
                self.stats.frames += 1;
                samples = samples.wrapping_add(decoded);
                let stereo = info.channels == 2;
                for i in 0..decoded as usize {
                    let (l, r) = if stereo { (2 * i, 2 * i + 1) } else { (i, i) };
                    let (left, right) = (i32::from(self.pcm[l]), i32::from(self.pcm[r]));
                    if step == 0 {
                        self.sound.put(left, right);
                        continue;
                    }
                    // Linear, from the last frame to this one.
                    while position < 65536 {
                        let t = position as i32;
                        // The difference can take 17 bits, so t gives up one to keep the product in 32.
                        self.sound.put(
                            before_left + (((left - before_left) * (t >> 1)) >> 15),
                            before_right + (((right - before_right) * (t >> 1)) >> 15),
                        );
                        position += step;
                    }
                    position -= 65536;
                    before_left = left;
                    before_right = right;
                }
                self.pump();
                self.model.elapsed = if hz != 0 { samples / hz } else { 0 };
                self.model.kbps = info.bitrate_kbps as u32;
                if let Some(levels) = &mut self.model.levels {
                    spectrum::run(&self.pcm[..], info.channels as u32, decoded as usize, levels);
                }
                continue;
            }
            // The ring is full: time for the keys and the screen.
            let now = self.clock.now_ms();
            if now.wrapping_sub(ui_at) < PLAY_UI_MS {
                let idle = TIMER_COUNT.read();
                while self.sound.audio.as_mut().is_some_and(|a| a.free_half().is_none())
                    && self.clock.now_ms().wrapping_sub(ui_at) < PLAY_UI_MS
                {}
                self.stats.idle += TIMER_COUNT.read().wrapping_sub(idle) / TICKS_PER_US;
                continue;
            }
            ui_at = now;
            let ev = self.key_events(true);
            let mut action = None;
            if ev & EV_LEAVE != 0 {
                action = Some(Play::Leave);
            } else if ev & EV_SELECT != 0 {
                action = if self.model.selected == self.count() { Some(Play::Leave) } else { self.pause() };
            } else if ev & (EV_REWIND | EV_FORWARD) != 0 && bytes_per_s != 0 {
                // A jump, then a moment of what's there, as a CD player does.
                let at = self.model.elapsed;
                let to = if ev & EV_FORWARD != 0 {
                    at + SEEK_STEP_S
                } else if at > SEEK_STEP_S {
                    at - SEEK_STEP_S
                } else {
                    0
                };
                let at_byte = first_frame.wrapping_add(to.wrapping_mul(bytes_per_s));
                if at_byte >= file.size {
                    action = Some(Play::End);
                } else {
                    offset = at_byte;
                    have = 0;
                    mp3dec_init(self.decoder);
                    samples = to.wrapping_mul(hz);
                    position = 0;
                    before_left = 0;
                    before_right = 0;
                    self.model.elapsed = to;
                    self.sound.clear();
                }
            } else {
                action = self.move_cursor(ev);
            }
            if let Some(action) = action {
                result = action;
                break;
            }
            let d = TIMER_COUNT.read();
            self.ui_now();
            self.stats.most_draw = self.stats.most_draw.max(TIMER_COUNT.read().wrapping_sub(d) / TICKS_PER_US);
        }
        self.stats.busy = TIMER_COUNT.read().wrapping_sub(started) / TICKS_PER_US;
        card_off();
        // Anything but the end drops what was waiting, so that it happens at once.
        if result != Play::End {
            self.sound.clear();
        }
        result
    }

    /// Plays from a track on, until the list ends or PLAY on "back". Returns true for leaving the app.
    fn play_from(&mut self, mut track: u32) -> bool {
        let mut error = None;
        let mut leave = false;
        self.sound.clear();
        self.sound.muted = false;
        // None only if sound were going already, which it isn't here.
        self.sound.audio = audio::start();
        self.playing_hooks = true;
        self.pix.budget(PLAY_SCREEN_BYTES);
        self.keys_reset();
        loop {
            let r = self.play(track, &mut error);
            let s = self.stats;
            log!(
                "track ",
                track + 1,
                ": ",
                s.frames,
                " frames in ",
                s.busy / 1000,
                " ms: ",
                self.sound.short_halves,
                " short halves, longest read ",
                s.most_read,
                " us, decode ",
                s.most_decode,
                " us, draw ",
                s.most_draw,
                " us, idle ",
                s.idle / 1000,
                " ms"
            );
            if r == Play::End {
                if track + 1 >= self.count() {
                    break;
                }
                // The cursor follows the music, unless it was put somewhere else.
                if self.model.selected == track {
                    self.model.selected = track + 1;
                }
                track += 1;
            } else if r == Play::Switch {
                track = self.model.selected;
            } else {
                leave = r == Play::Leave;
                if let Some(error) = error {
                    self.model.note = Some(error.as_bytes());
                }
                break;
            }
        }
        // What's left of the last track plays out, then silence in both halves, so that stopping doesn't leave the
        // DAC holding a sample.
        while self.sound.level() != 0 && !leave && error.is_none() {
            self.pump();
            let ev = self.key_events(true);
            if ev & (EV_SELECT | EV_LEAVE) != 0 {
                leave = ev & EV_LEAVE != 0 || self.model.selected == self.count();
                break;
            }
            self.ui_now();
        }
        self.sound.clear();
        self.sound.muted = true;
        for _ in 0..2 {
            while self.sound.audio.as_mut().is_some_and(|a| a.free_half().is_none()) {}
            self.pump();
        }
        if let Some(audio) = self.sound.audio.take() {
            audio.stop();
        }
        self.playing_hooks = false;
        self.pix.budget(0);
        self.model.mode = STOPPED;
        self.model.playing = -1;
        self.model.elapsed = 0;
        self.model.kbps = 0;
        self.model.hz = 0;
        self.model.channels = 0;
        self.keys_reset();
        leave
    }
}

fn main(lsb_mhz: u32) -> i32 {
    timer_start();
    let mut clock = Clock { ms: 0, ticks: 0 };
    clock.now_ms();
    let board = app::board();
    if board == 0 {
        // Its pins aren't known, so nothing is touched.
        log!("which board this is isn't known");
        return 1;
    }
    board_init(board == 2);
    display::init(board);
    // Safety: each is claimed here once, and only the player uses them from here on.
    let mut player = unsafe {
        Player {
            lsb: lsb_mhz,
            volume: VOLUME.get(),
            files: &[],
            decoder: DECODER.get(),
            chunk: CHUNK_BUFFER.get(),
            input: INPUT_BUFFER.get(),
            pcm: PCM.get(),
            pix: pixel::claim(),
            sound: Sound {
                audio: None,
                ring: RING_BUFFER.get(),
                head: 0,
                tail: 0,
                muted: false,
                primed: false,
                short_halves: 0,
            },
            clock,
            keys: Keys { held: 0, repeat_at: 0, long_press: false },
            stats: Stats::default(),
            ui: Ui::new(),
            ui_last_ms: 0,
            model: Model {
                mode: STOPPED,
                elapsed: 0,
                total: 0,
                kbps: 0,
                hz: 0,
                channels: 0,
                count: 0,
                selected: 0,
                playing: -1,
                names: &[] as &[Named; 0],
                note: Some(b"READING THE CARD"),
                keys: 0,
                levels: Some([0; spectrum::BANDS]),
            },
            playing_hooks: false,
        }
    };
    let p = &mut player;
    p.ui.start(p.pix, board, &mut Plain);
    p.ui_now();
    // Safety: the only use of it.
    let note = p.read_files(unsafe { FILES.get() });
    p.model.note = note.map(str::as_bytes);
    p.model.count = p.count();
    log!(p.count(), " files, LSB ", lsb_mhz, ", audio clock stock ", u32::from(audio::clock_is_stock()));
    p.keys_reset();
    loop {
        let ev = p.key_events(false);
        let count = p.count();
        if ev & EV_LEAVE != 0 {
            break;
        }
        if ev & EV_PREV != 0 {
            p.model.selected = if p.model.selected != 0 { p.model.selected - 1 } else { count };
        } else if ev & EV_NEXT != 0 {
            p.model.selected = if p.model.selected < count { p.model.selected + 1 } else { 0 };
        } else if ev & EV_SELECT != 0 && (p.model.selected >= count || p.play_from(p.model.selected)) {
            break;
        }
        p.ui_now();
        app::feed();
        hal_delay_us(IDLE_POLL_US);
    }
    card::open(lsb_mhz);
    card::finish();
    card::close();
    board_restore();
    0
}
