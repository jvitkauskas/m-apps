// SPDX-License-Identifier: MIT
//! Runs kit/log.rs on this computer, against a plain buffer. tests/applog_test.py decodes what it writes.
use kit::log;
use kit::log::{Char, Dec, Hex, hex};
use std::sync::atomic::Ordering;

fn save(path: &str) {
    std::fs::write(path, log::ram_bytes()).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    log::ram_bytes().fill(0x5a);
    log::begin(b"APP1\x12\x34\x01\x01");
    log!("plain");
    log!("text", " ", Char(b'c'), " ", -42i32, " ", 42u32, " ", hex(0xabc), " ", Hex(0xabc, 8), " ", Dec(7, 5), " % %q");
    log!(i32::MIN);
    save(&args[1]);
    log::end(-3);
    save(&args[2]);
    for i in 0..400u32 {
        log!("line ", i);
    }
    save(&args[3]);
    if log::FEEDS.load(Ordering::Relaxed) != 403 {
        std::process::exit(1);
    }
    log::crashed();
    save(&args[4]);
}
