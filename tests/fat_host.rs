// SPDX-License-Identifier: MIT
//! Runs apps/picker's FAT and loader code on the computer, on a card image. See tests/picker_test.py.
//!
//! image, board, name to load or - (or @catalog: the launcher's list, apps/picker/catalog.rs), out file for what was
//! loaded, sector that fails or -.
#[path = "../apps/picker/catalog.rs"]
mod catalog;

use kit::fat::{Entry, Volume, until_zero};
use kit::{card, load};
use std::sync::atomic::{AtomicU32, Ordering};

const GUARD: usize = 64;
const BUFFER: usize = 0x4fd80 + GUARD;

static SECTORS: AtomicU32 = AtomicU32::new(0);
static FAIL_SECTOR: AtomicU32 = AtomicU32::new(u32::MAX);

fn read_hook(sector: u32) -> bool {
    if sector >= SECTORS.load(Ordering::Relaxed) {
        eprintln!("read beyond the card: sector {sector}");
        std::process::abort();
    }
    sector != FAIL_SECTOR.load(Ordering::Relaxed)
}

/// The picker never writes, so a write here is a bug.
fn write_hook(_sector: u32) -> bool {
    eprintln!("a write");
    std::process::abort();
}

fn text(s: &[u8]) -> String {
    String::from_utf8_lossy(until_zero(s)).into_owned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        std::process::exit(2);
    }
    let Ok(image) = std::fs::read(&args[1]) else { std::process::exit(2) };
    let image: &'static mut [u8] = image.leak();
    SECTORS.store((image.len() / 512) as u32, Ordering::Relaxed);
    if args[5] != "-" {
        let s = &args[5];
        let n = s.strip_prefix("0x").map_or_else(|| s.parse(), |h| u32::from_str_radix(h, 16));
        FAIL_SECTOR.store(n.unwrap_or(0), Ordering::Relaxed);
    }
    let sectors = SECTORS.load(Ordering::Relaxed);
    card::attach(image);
    card::hooks(Some(read_hook), Some(write_hook));
    let mut volume = Box::new(Volume::new());
    let v = &mut *volume;
    let mut apps = [Entry::default(); 100];
    let (mut count, mut more) = (0, false);
    let mut folder = 0;
    let mut error = v.mount(sectors).err();
    if error.is_none() {
        folder = v.root();
        error = v.folder(&mut folder, b"MBOOT      ", "NO /MBOOT/APPS").err();
    }
    if error.is_none() {
        error = v.folder(&mut folder, b"APPS       ", "NO /MBOOT/APPS").err();
    }
    if error.is_none() {
        match v.list(folder, b"APP", &mut apps) {
            Ok((n, m)) => (count, more) = (n, m),
            Err(e) => error = Some(e),
        }
    }
    if args[3] == "@catalog" {
        let mut c = Box::new(catalog::Catalog::new());
        let mut more = false;
        if error.is_none() {
            error = catalog::read(v, folder, &mut c, &mut more).err();
        }
        let shown = if error.is_some() { 0 } else { c.count };
        println!("catalog {} {} {}", error.unwrap_or("ok"), shown, u32::from(more));
        if error.is_none() {
            for a in &c.apps[..c.count] {
                println!(
                    "app {}|{}|{}|{}|{}|{}|{:x}",
                    String::from_utf8_lossy(&a.id),
                    a.file.size,
                    a.icon.size,
                    text(&a.text.name),
                    text(&a.text.about),
                    text(&a.text.version),
                    a.launched_entry
                );
            }
        }
        return;
    }
    println!("list {} {} {}", error.unwrap_or("ok"), count, u32::from(more));
    for a in &apps[..count] {
        println!("app {} {}", String::from_utf8_lossy(until_zero(&a.name)), a.size);
    }
    if args[3] != "-" {
        let name = args[3].as_bytes();
        for a in &apps[..count] {
            if a.name.starts_with(name) {
                let mut to = vec![0xa5u8; BUFFER];
                let result = load::load(v, a, &mut to, &load::APP, args[2].parse().unwrap_or(0));
                println!("load {}", result.err().unwrap_or("ok"));
                if std::fs::write(&args[4], &to).is_err() {
                    std::process::exit(2);
                }
            }
        }
    }
}
