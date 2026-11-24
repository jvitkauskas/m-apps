// SPDX-License-Identifier: MIT
//! Runs the kit's FAT writing (kit/fat.rs) on this computer, on a card image. See tests/fat_write_test.py.
//!
//! image, cut after this many writes or -1, then: mkdir PATH | new PATH NAME FILE | newlong PATH NAME LONG FILE |
//! swap PATH NAME FILE | remove PATH NAME | names PATH EXT | reserve BYTES
use kit::fat::{self, ATTR_ARCHIVE, Named, Volume};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

static SECTORS: AtomicU32 = AtomicU32::new(0);
static WRITES: AtomicI64 = AtomicI64::new(0);
static CUT_AFTER: AtomicI64 = AtomicI64::new(-1);

fn read_hook(sector: u32) -> bool {
    if sector >= SECTORS.load(Ordering::Relaxed) {
        eprintln!("read beyond the card: sector {sector}");
        std::process::abort();
    }
    true
}

fn write_hook(sector: u32) -> bool {
    if sector >= SECTORS.load(Ordering::Relaxed) {
        eprintln!("write beyond the card: sector {sector}");
        std::process::abort();
    }
    // Power goes before this write.
    if WRITES.fetch_add(1, Ordering::Relaxed) == CUT_AFTER.load(Ordering::Relaxed) {
        save_and_exit(42);
    }
    WRITTEN.lock().unwrap().push(sector);
    true
}

static WRITTEN: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
static IMAGE_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static IMAGE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// What was written goes back to the image's file, as the C program's was a shared mapping, then the program ends.
fn save_and_exit(code: i32) -> ! {
    use std::io::{Seek, SeekFrom, Write};
    let (ptr, n) = (*IMAGE.get().unwrap() as *const u8, SECTORS.load(Ordering::Relaxed) as usize * 512);
    // Safety: the image this program leaked, of that size.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, n) };
    let mut file = std::fs::OpenOptions::new().write(true).open(IMAGE_PATH.get().unwrap()).unwrap();
    for &s in WRITTEN.lock().unwrap().iter() {
        let at = s as usize * 512;
        file.seek(SeekFrom::Start(at as u64)).unwrap();
        file.write_all(&bytes[at..at + 512]).unwrap();
    }
    std::process::exit(code)
}

fn pad(name: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    out
}

fn check<T>(r: fat::Result<T>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            println!("error {e}");
            save_and_exit(3)
        }
    }
}

/// Follows a path, making its folders if asked.
fn folder(v: &mut Volume, path: &str, make: bool) -> u32 {
    let mut c = v.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        let name = pad(part);
        check(if make { v.make_folder(&mut c, &name) } else { v.folder(&mut c, &name, "NO FOLDER") });
    }
    c
}

/// A chain holding a file's bytes, padded with zeros to whole sectors.
fn write_data(v: &mut Volume, path: &str) -> (u32, u32) {
    let mut data = std::fs::read(path).unwrap_or_else(|_| std::process::exit(2));
    let size = data.len() as u32;
    data.resize(data.len().div_ceil(512) * 512, 0);
    let start = check(v.reserve(size));
    if !data.is_empty() && !v.write(start, 0, &data) {
        check::<()>(Err("write failed"));
    }
    (start, size)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        std::process::exit(2);
    }
    let image = std::fs::read(&args[1]).unwrap_or_else(|_| std::process::exit(2)).leak();
    SECTORS.store((image.len() / 512) as u32, Ordering::Relaxed);
    IMAGE_PATH.set(args[1].clone()).unwrap();
    IMAGE.set(image.as_ptr() as usize).unwrap();
    CUT_AFTER.store(args[2].parse().unwrap(), Ordering::Relaxed);
    kit::card::attach(image);
    kit::card::hooks(Some(read_hook), Some(write_hook));
    let v: &mut Volume = Box::leak(Box::new(Volume::new()));
    check(v.mount(SECTORS.load(Ordering::Relaxed)));
    let a = |i: usize| args[i].as_str();
    match a(3) {
        "mkdir" => {
            folder(v, a(4), true);
        }
        "new" => {
            let dir = folder(v, a(4), false);
            let (start, size) = write_data(v, a(6));
            check(v.add(dir, &pad(a(5)), ATTR_ARCHIVE, start, size));
        }
        "newlong" => {
            let dir = folder(v, a(4), false);
            let (start, size) = write_data(v, a(7));
            check(v.add_named(dir, &pad(a(5)), a(6).as_bytes(), ATTR_ARCHIVE, start, size));
        }
        "swap" => {
            let dir = folder(v, a(4), false);
            let (start, size) = write_data(v, a(6));
            check(v.swap(dir, &pad(a(5)), start, size));
        }
        "remove" => {
            let dir = folder(v, a(4), false);
            check(v.remove(dir, &pad(a(5))));
        }
        "names" => {
            let dir = folder(v, a(4), false);
            let mut named = [Named::empty(); 8];
            let ext: [u8; 3] = a(5).as_bytes().try_into().unwrap();
            let (count, more) = check(v.list_named(dir, &ext, &mut named));
            println!("count {count} {}", u32::from(more));
            for n in &named[..count] {
                println!("name\t-\t{}\t{}", n.file.size, String::from_utf8_lossy(n.name()));
            }
        }
        "reserve" => {
            check(v.reserve(a(4).parse().unwrap()));
        }
        _ => std::process::exit(2),
    }
    check(v.commit());
    println!("ok {}", WRITES.load(Ordering::Relaxed));
    save_and_exit(0)
}
