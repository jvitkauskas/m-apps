// SPDX-License-Identifier: MIT
//! apps/jpeg's header parser on a computer: the sample pictures parse, and malformed headers don't.
// The decoder's half is for the unit, so this leaves it unused.
#[allow(dead_code)]
#[path = "../apps/jpeg/jpeg_hw.rs"]
mod jpeg_hw;
// The names are for the unit's log and screen.
#[allow(dead_code)]
#[path = "../apps/jpeg/images.rs"]
mod images;

use std::sync::atomic::{AtomicBool, Ordering};

static FAILED: AtomicBool = AtomicBool::new(false);

macro_rules! check {
    ($x:expr) => {
        if !$x {
            println!("{}:{}: {}", file!(), line!(), stringify!($x));
            FAILED.store(true, Ordering::Relaxed);
        }
    };
}

/// A copy of exactly the bytes given, so that a read past them is caught.
fn parse(p: &[u8]) -> Result<(), &'static str> {
    let copy = p.to_vec().into_boxed_slice();
    jpeg_hw::parse(&copy).map(|_| ())
}

fn main() {
    for (i, picture) in images::IMAGES.iter().enumerate() {
        let e = parse(picture.data);
        check!(e.is_ok());
        if let Err(e) = e {
            println!("picture {i}: {e}");
        }
    }
    let p = images::IMAGES[0].data;
    let n = p.len();
    // The first marker without its FF.
    let mut bad = p.to_vec();
    bad[2] = 0;
    check!(parse(&bad).is_err());
    // Cut right after the scan's header: no coded data.
    let mut sos = 2;
    while sos + 3 < n && !(p[sos] == 0xff && p[sos + 1] == 0xda) {
        sos += 1;
    }
    let header_end = sos + 2 + (usize::from(p[sos + 2]) << 8 | usize::from(p[sos + 3]));
    check!(parse(&p[..header_end]) == Err("NO DATA"));
    // A scan that isn't baseline's: spectral selection 1..63.
    let mut bad = p.to_vec();
    bad[header_end - 3] = 1;
    check!(parse(&bad) == Err("NOT BASELINE"));
    // An end of image before any scan, a restart marker in the headers, the found inputs.
    let eoi = [0xff, 0xd8, 0xff, 0xd9];
    let rst = [0xff, 0xd8, 0xff, 0xd0, 0, 4, 0, 0];
    let dri = [0xff, 0xd8, 0xff, 0xdd, 0x00, 0x02];
    check!(parse(&eoi) == Err("NO SCAN"));
    check!(parse(&rst) == Err("BAD MARKER"));
    check!(parse(&dri) == Err("BAD SEGMENT"));
    let failed = FAILED.load(Ordering::Relaxed);
    println!("{}", if failed { "FAILED" } else { "jpeg parse OK" });
    std::process::exit(failed as i32);
}
