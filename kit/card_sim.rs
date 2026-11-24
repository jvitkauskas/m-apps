// SPDX-License-Identifier: MIT
//! The card, for the tests on this computer: an image in memory, which a test gives with [`attach`]. The same
//! functions as kit/card.rs, on that image; a sector outside it fails, as a card's would.
use crate::Global;

struct Sim {
    image: *mut u8,
    sectors: u32,
    /// Called before each read and write with its sector: false fails it, as a card's error would.
    read_hook: Option<fn(u32) -> bool>,
    write_hook: Option<fn(u32) -> bool>,
    stream_at: u32,
    write_at: u32,
}

static SIM: Global<Sim> =
    Global::new(Sim { image: core::ptr::null_mut(), sectors: 0, read_hook: None, write_hook: None, stream_at: 0, write_at: 0 });

fn sim() -> &'static mut Sim {
    // Safety: the tests run one at a time (--test-threads=1), and each function here takes it once.
    unsafe { SIM.get() }
}

/// The image the card holds from now: whole sectors.
pub fn attach(image: &'static mut [u8]) {
    let s = sim();
    s.sectors = (image.len() / 512) as u32;
    s.image = image.as_mut_ptr();
}

/// What's called before each read and each write, with its sector: false fails it. A test can stop there too, as
/// power going would.
pub fn hooks(read: Option<fn(u32) -> bool>, write: Option<fn(u32) -> bool>) {
    let s = sim();
    s.read_hook = read;
    s.write_hook = write;
}

fn sector(n: u32, writing: bool) -> Option<&'static mut [u8; 512]> {
    let s = sim();
    let hook = if writing { s.write_hook } else { s.read_hook };
    if hook.is_some_and(|h| !h(n)) || n >= s.sectors {
        return None;
    }
    // Safety: inside the image attach() was given, which lives as long as the program.
    Some(unsafe { &mut *s.image.add(n as usize * 512).cast::<[u8; 512]>() })
}

pub fn open(_lsb_mhz: u32) {}
pub fn close() {}
pub fn fast(_lsb_mhz: u32) {}
pub fn finish() {}

pub fn start_card(_lsb_mhz: u32) -> u32 {
    sim().sectors
}

pub fn read(n: u32, data: &mut [u8; 512]) -> bool {
    sector(n, false).map(|s| data.copy_from_slice(s)).is_some()
}

pub fn write(n: u32, data: &[u8; 512]) -> bool {
    sector(n, true).map(|s| s.copy_from_slice(data)).is_some()
}

pub fn stream_start(n: u32) -> bool {
    sim().stream_at = n;
    n < sim().sectors
}

pub struct Reading<'a>(&'a mut [u8; 512]);

pub fn stream_begin(data: &mut [u8; 512]) -> Option<Reading<'_>> {
    Some(Reading(data))
}

impl Reading<'_> {
    pub fn finish(self) -> bool {
        let at = sim().stream_at;
        sim().stream_at = at + 1;
        read(at, self.0)
    }
}

pub fn stream_read(data: &mut [u8; 512]) -> bool {
    stream_begin(data).is_some_and(Reading::finish)
}

pub fn stream_stop() {}

pub fn write_stream_start(n: u32) -> bool {
    sim().write_at = n;
    n < sim().sectors
}

pub struct Writing<'a>(&'a [u8; 512]);

pub fn write_stream_begin(data: &[u8; 512]) -> Writing<'_> {
    Writing(data)
}

impl Writing<'_> {
    pub fn finish(self) -> bool {
        let at = sim().write_at;
        sim().write_at = at + 1;
        write(at, self.0)
    }
}

pub fn write_stream_stop() -> bool {
    true
}
