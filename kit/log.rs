// SPDX-License-Identifier: MIT
//! The app's log: a ring in RAM that a host reads after the app has ended, on any route. See README.
//!
//! Below the retained area, and outside the app's image, so every route and any reset leave it alone.
//! tools/applog.py reads the same layout: a 32-byte header, then the ring.
//!
//! [`log!`](crate::log!) adds a line from its arguments, each written as it is: text, numbers in decimal, and
//! [`hex`]/[`Hex`] and [`Dec`] for hex or a width. It needs nothing of `core::fmt`, which costs kilobytes here.

pub const ADDRESS: usize = 0x01c7_f000;
pub const SIZE: u32 = 0xd80;
pub const HEADER: u32 = 32;
pub const CAPACITY: u32 = SIZE - HEADER;
const MAGIC: u32 = 0x474f_4c41; // "ALOG"
const FORMAT: u32 = 1;
const RUNNING: u32 = 1;
const RETURNED: u32 = 2;
const CRASHED: u32 = 3;

/// The log's RAM: on the unit the fixed area, and in the tests on this computer a buffer of theirs.
#[cfg(target_arch = "pi32v2")]
fn ram() -> *mut u8 {
    core::ptr::with_exposed_provenance_mut(ADDRESS)
}

#[cfg(not(target_arch = "pi32v2"))]
fn ram() -> *mut u8 {
    static mut RAM: crate::Aligned<{ SIZE as usize }> = crate::Aligned([0; SIZE as usize]);
    (&raw mut RAM).cast()
}

fn word(offset: u32) -> *mut u32 {
    ram().wrapping_add(offset as usize).cast()
}

fn read(offset: u32) -> u32 {
    // Safety: the log's area is this program's, and no reference to it is ever made.
    unsafe { word(offset).read_volatile() }
}

fn write(offset: u32, value: u32) {
    unsafe { word(offset).write_volatile(value) }
}

fn put_byte(c: u8) {
    let total = read(16);
    unsafe { ram().add((HEADER + total % CAPACITY) as usize).write_volatile(c) };
    write(16, total.wrapping_add(1));
}

fn number(mut value: u32, base: u32, width: u8, pad: u8) {
    let mut digits = [0u8; 10];
    let mut n = 0;
    loop {
        digits[n] = b"0123456789abcdef"[(value % base) as usize];
        n += 1;
        value /= base;
        if value == 0 {
            break;
        }
    }
    for _ in n..width as usize {
        put_byte(pad);
    }
    while n > 0 {
        n -= 1;
        put_byte(digits[n]);
    }
}

/// What [`log!`](crate::log!) can write.
pub trait Arg {
    fn put(self);
}

impl Arg for &str {
    fn put(self) {
        self.as_bytes().put()
    }
}

impl Arg for &[u8] {
    fn put(self) {
        for &c in self {
            put_byte(c);
        }
    }
}

impl<const N: usize> Arg for &[u8; N] {
    fn put(self) {
        self.as_slice().put()
    }
}

impl Arg for u32 {
    fn put(self) {
        number(self, 10, 0, b' ')
    }
}

impl Arg for usize {
    fn put(self) {
        (self as u32).put()
    }
}

impl Arg for u16 {
    fn put(self) {
        u32::from(self).put()
    }
}

impl Arg for u8 {
    fn put(self) {
        u32::from(self).put()
    }
}

impl Arg for i32 {
    fn put(self) {
        if self < 0 {
            put_byte(b'-');
        }
        self.unsigned_abs().put()
    }
}

/// One character.
pub struct Char(pub u8);

impl Arg for Char {
    fn put(self) {
        put_byte(self.0)
    }
}

/// In hex, with at least `width` digits, padded with zeros.
pub struct Hex(pub u32, pub u8);

/// In hex, as few digits as it takes.
pub fn hex(value: u32) -> Hex {
    Hex(value, 0)
}

impl Arg for Hex {
    fn put(self) {
        number(self.0, 16, self.1, b'0')
    }
}

/// In decimal, at least `width` characters, padded with spaces.
pub struct Dec(pub u32, pub u8);

impl Arg for Dec {
    fn put(self) {
        number(self.0, 10, self.1, b' ')
    }
}

/// Ends the line [`log!`](crate::log!) wrote, and feeds the guard.
pub fn end_line() {
    put_byte(b'\n');
    crate::hw::sync_cpu();
    crate::app::feed();
    #[cfg(not(target_arch = "pi32v2"))]
    FEEDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// On this computer, how many times a line has fed the guard, for the tests.
#[cfg(not(target_arch = "pi32v2"))]
pub static FEEDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Adds one line: `log!("board ", board, ", chip ", Hex(id, 6))`.
#[macro_export]
macro_rules! log {
    ($($arg:expr),* $(,)?) => {{
        $( $crate::log::Arg::put($arg); )*
        $crate::log::end_line();
    }};
}

/// For the kit's start ([`crate::app`]): a new log, for the file whose trailer this is.
pub fn begin(trailer: &[u8; 8]) {
    write(0, 0);
    crate::hw::sync_cpu();
    write(4, FORMAT);
    for (i, &b) in trailer.iter().enumerate() {
        unsafe { ram().add(8 + i).write_volatile(b) };
    }
    write(16, 0);
    write(20, RUNNING);
    write(24, 0);
    write(28, 0);
    write(0, MAGIC);
    crate::hw::sync_cpu();
}

pub fn end(result: i32) {
    write(24, result as u32);
    write(20, RETURNED);
    crate::hw::sync_cpu();
}

pub fn crashed() {
    write(20, CRASHED);
    crate::hw::sync_cpu();
}

/// The log's RAM as it is, for the tests on this computer.
#[cfg(not(target_arch = "pi32v2"))]
pub fn ram_bytes() -> &'static mut [u8; SIZE as usize] {
    // Safety: the tests run one at a time, and use this between log lines.
    unsafe { &mut *ram().cast::<[u8; SIZE as usize]>() }
}

/// What the log holds so far, for the tests on this computer.
#[cfg(not(target_arch = "pi32v2"))]
pub fn text() -> ([u8; CAPACITY as usize], usize) {
    let mut out = [0u8; CAPACITY as usize];
    let total = read(16) as usize;
    let n = total.min(CAPACITY as usize);
    for (i, o) in out.iter_mut().enumerate().take(n) {
        let at = (total - n + i) % CAPACITY as usize;
        *o = unsafe { ram().add(HEADER as usize + at).read_volatile() };
    }
    (out, n)
}
