// SPDX-License-Identifier: MIT
//! A file that's loaded whole and run: an app (m-bootloader's docs/SPEC.md §11.8), or the card launcher's LAUNCH.BIN
//! (apps/menu). And what the launcher shows for an app: its folder's META.TXT.
use crate::fat::{Entry, Error, READ_ERROR, Volume};

pub const APP_ADDRESS: u32 = 0x01c3_0000;
const TRAILER: u32 = 8;
const SMALLEST: u32 = 16;
const FORMAT: u8 = 1;

/// A file that's loaded whole and run: its trailer's magic, the largest it can be, and what to show when it's wrong.
pub struct Kind {
    pub magic: [u8; 4],
    pub largest: u32,
    pub not_one: Error,
    pub too_big: Error,
    pub damaged: Error,
}

/// §11.8 allows 0x4fd80, but the launcher logs through the load, and the log starts here (kit/log.rs).
pub const APP: Kind =
    Kind { magic: *b"APP1", largest: 0x4f000, not_one: "NOT AN APP FILE", too_big: "APP TOO BIG", damaged: "APP FILE DAMAGED" };
/// Linked at 0x01C0A800, up to 0x01C16800 (apps/menu).
pub const LAUNCHER: Kind = Kind {
    magic: *b"LNC1",
    largest: 0xc000,
    not_one: "NOT A LAUNCHER FILE",
    too_big: "LAUNCHER TOO BIG",
    damaged: "LAUNCHER FILE DAMAGED",
};

/// Checks the file's trailer, loads it to `address` and checks its CRC.
///
/// # Safety
/// The `kind.largest` bytes from `address` are RAM that nothing in this program uses, as an app's slot is to the
/// launcher.
pub unsafe fn load_at(volume: &mut Volume, file: &Entry, address: u32, kind: &Kind, board: u32) -> Result<(), Error> {
    // Safety: as the caller promises.
    let to = unsafe {
        core::slice::from_raw_parts_mut(core::ptr::with_exposed_provenance_mut::<u8>(address as usize), kind.largest as usize)
    };
    load(volume, file, to, kind, board)
}

/// The same into `to`, from its start: a file bigger than `to` is too big, as one bigger than `kind.largest` is.
pub fn load(volume: &mut Volume, file: &Entry, to: &mut [u8], kind: &Kind, board: u32) -> Result<(), Error> {
    let mut trailer = [0u8; TRAILER as usize];
    if file.size < SMALLEST {
        return Err(kind.not_one);
    }
    // First, as this also limits how far a damaged chain gets walked.
    if file.size > kind.largest || file.size as usize > to.len() {
        return Err(kind.too_big);
    }
    if !volume.read(file, file.size - TRAILER, &mut trailer) {
        return Err(READ_ERROR);
    }
    if trailer[..4] != kind.magic {
        return Err(kind.not_one);
    }
    if trailer[7] != FORMAT {
        return Err("NEWER FILE FORMAT");
    }
    if trailer[6] != 0 && u32::from(trailer[6]) != board {
        return Err("OTHER BOARD");
    }
    let to = &mut to[..file.size as usize];
    if !volume.read(file, 0, to) {
        return Err(READ_ERROR);
    }
    let body = &to[..(file.size - TRAILER) as usize];
    if mboot::formats::crc16(0, body) != u16::from_le_bytes([trailer[4], trailer[5]]) {
        return Err(kind.damaged);
    }
    Ok(())
}

/// Where the launcher calls the app with its folder: the kit's header (kit/start.S), if the file has one and the
/// address is inside the app as loaded at APP_ADDRESS, and otherwise 0.
pub fn launched_entry(volume: &mut Volume, file: &Entry) -> u32 {
    let mut header = [0u8; 12];
    // The header, then at least the trailer after it.
    if file.size < 4 + 12 + TRAILER || file.size > APP.largest || !volume.read(file, 4, &mut header) {
        return 0;
    }
    if &header[..4] != crate::app::HEADER_MAGIC || header[4] != crate::app::HEADER_FORMAT {
        return 0;
    }
    let entry = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    // An instruction's address: inside the image and on a word.
    if entry & 3 == 0 && entry >= APP_ADDRESS && entry < APP_ADDRESS + file.size - TRAILER { entry } else { 0 }
}

/// What the card's launcher shows for an app, as text the screen can show: each ends in a zero, and anything outside
/// printable ASCII is a '?'.
#[derive(Clone, Copy)]
pub struct AppText {
    pub name: [u8; 25],
    pub about: [u8; 61],
    pub version: [u8; 13],
}

impl AppText {
    pub const fn empty() -> AppText {
        AppText { name: [0; 25], about: [0; 61], version: [0; 13] }
    }
}

/// How much of a META.TXT is read.
pub const META_MAX: usize = 512;

/// The value into `to`, up to the line's end: a character beyond ASCII as one '?'.
fn copy_value(to: &mut [u8], from: &[u8]) {
    let mut n = 0;
    let mut i = 0;
    while i < from.len() && n + 1 < to.len() {
        let c = from[i];
        i += 1;
        if c >= 0x80 {
            while i < from.len() && from[i] & 0xc0 == 0x80 {
                i += 1;
            }
        }
        to[n] = if (b' '..=b'~').contains(&c) { c } else { b'?' };
        n += 1;
    }
    while n > 0 && to[n - 1] == b' ' {
        n -= 1;
    }
    to[n] = 0;
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let [b' ', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' '] = s {
        s = rest;
    }
    s
}

/// A folder's META.TXT: lines of key=value, with name, about and version, as UTF-8. Each one there replaces text's;
/// other keys, and lines that aren't key=value, are skipped. Returns how many it took.
pub fn meta_parse(meta: &[u8], text: &mut AppText) -> u32 {
    let meta = &meta[..meta.len().min(META_MAX)];
    let mut taken = 0;
    for line in meta.split(|&c| c == b'\n' || c == b'\r') {
        let Some(eq) = line.iter().position(|&c| c == b'=') else { continue };
        let key = trim(&line[..eq]);
        let mut value = &line[eq + 1..];
        while let [b' ', rest @ ..] = value {
            value = rest;
        }
        let field: &mut [u8] = match key {
            b"name" => &mut text.name,
            b"about" => &mut text.about,
            b"version" => &mut text.version,
            _ => continue,
        };
        let mut copy = [0u8; 64];
        copy_value(&mut copy[..field.len()], value);
        // An empty value leaves what was there.
        if copy[0] != 0 {
            field.copy_from_slice(&copy[..field.len()]);
            taken += 1;
        }
    }
    taken
}
