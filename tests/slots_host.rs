// SPDX-License-Identifier: MIT
//! Runs apps/slots' logic on the computer, with a card image and a flash image. See tests/slots_test.py.
//!
//! card, flash, cut after this many changes or -1, then: list | save NAME | load NAME | delete NAME
// The screens' half of the app isn't here, so some of slotfile goes unused.
#[allow(dead_code)]
#[path = "../apps/slots/slotfile.rs"]
mod slotfile;

use kit::card;
use kit::mboot::formats::{Flash, Nor};
use slotfile::{Hooks, SLOT_FLASH_SIZE, Slots};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

const UID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const JEDEC: u32 = 0x85_6014;
/// The board the test runs as: the SMK-37, whose kept sectors in the middle of the flash a load must leave alone.
const BOARD: u32 = 2;

static CARD_SECTORS: AtomicU32 = AtomicU32::new(0);
static OPS: AtomicI64 = AtomicI64::new(0);
static CUT_AFTER: AtomicI64 = AtomicI64::new(-1);
static PATHS: OnceLock<(String, String)> = OnceLock::new();
static CARD: OnceLock<usize> = OnceLock::new();
/// The card's sectors written, and the flash: what goes back to the files when the program ends, as the C program's
/// were shared mappings.
static WRITTEN: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());
static FLASH: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// What was written goes back to the images' files, then the program ends.
fn save_and_exit(code: i32) -> ! {
    use std::io::{Seek, SeekFrom, Write};
    let (card_path, flash_path) = PATHS.get().unwrap();
    let n = CARD_SECTORS.load(Ordering::Relaxed) as usize * 512;
    // Safety: the image this program leaked, of that size, which nothing writes now.
    let bytes = unsafe { std::slice::from_raw_parts(*CARD.get().unwrap() as *const u8, n) };
    let mut file = std::fs::OpenOptions::new().write(true).open(card_path).unwrap();
    for &s in WRITTEN.lock().unwrap().iter() {
        let at = s as usize * 512;
        file.seek(SeekFrom::Start(at as u64)).unwrap();
        file.write_all(&bytes[at..at + 512]).unwrap();
    }
    std::fs::write(flash_path, &*FLASH.lock().unwrap()).unwrap();
    std::process::exit(code)
}

/// Power goes before this change.
fn op() {
    if OPS.fetch_add(1, Ordering::Relaxed) == CUT_AFTER.load(Ordering::Relaxed) {
        save_and_exit(42);
    }
}

fn read_hook(sector: u32) -> bool {
    if sector >= CARD_SECTORS.load(Ordering::Relaxed) {
        std::process::abort();
    }
    true
}

fn write_hook(sector: u32) -> bool {
    if sector >= CARD_SECTORS.load(Ordering::Relaxed) {
        std::process::abort();
    }
    op();
    WRITTEN.lock().unwrap().insert(sector);
    true
}

/// The unit's flash, as an image.
struct TestFlash;

/// Only the app area, and never what's kept.
fn writable(address: u32) -> bool {
    (0x4000..0xf_f000).contains(&address) && !(0xc_0000..0xc_2000).contains(&address)
}

impl Flash for TestFlash {
    fn capacity(&self) -> u32 {
        SLOT_FLASH_SIZE
    }

    fn read_inside(&self, address: u32, dst: &mut [u8]) -> bool {
        let at = address as usize;
        if at + dst.len() > SLOT_FLASH_SIZE as usize {
            std::process::abort();
        }
        dst.copy_from_slice(&FLASH.lock().unwrap()[at..at + dst.len()]);
        true
    }
}

impl Nor for TestFlash {
    fn program(&self, address: u32, data: &[u8]) -> bool {
        if !writable(address) {
            std::process::abort();
        }
        let at = address as usize;
        // A cut here leaves half a sector, as pages are written in turn.
        if OPS.load(Ordering::Relaxed) == CUT_AFTER.load(Ordering::Relaxed) {
            {
                let mut flash = FLASH.lock().unwrap();
                for (f, &d) in flash[at..at + data.len() / 2].iter_mut().zip(data) {
                    *f &= d;
                }
            }
            save_and_exit(42);
        }
        OPS.fetch_add(1, Ordering::Relaxed);
        let mut flash = FLASH.lock().unwrap();
        for (f, &d) in flash[at..at + data.len()].iter_mut().zip(data) {
            *f &= d;
        }
        true
    }

    fn erase(&self, address: u32, size: u32) -> bool {
        if !writable(address) || size != 0x1000 {
            std::process::abort();
        }
        op();
        let at = address as usize;
        FLASH.lock().unwrap()[at..at + size as usize].fill(0xff);
        true
    }
}

/// As the bootloader's check does, it starts from the boot area's header, which says where the firmware is.
struct Check;

impl Hooks for Check {
    fn valid(&mut self, image: &dyn Flash) -> bool {
        let (mut boot, mut head) = ([0u8; 4], [0u8; 4]);
        image.read(0, &mut boot) && boot != [0xff; 4] && image.read(0x4000, &mut head) && &head == b"GOOD"
    }
}

fn named(s: &Slots, name: &str) -> usize {
    match s.list[..s.count].iter().position(|slot| slot.name() == name.as_bytes()) {
        Some(i) => i,
        None => {
            println!("no slot {name}");
            save_and_exit(4)
        }
    }
}

fn shown(s: &Slots, index: Option<usize>) -> String {
    index.map_or("-".into(), |i| String::from_utf8_lossy(s.list[i].name()).into_owned())
}

fn find_id(s: &Slots, id: &[u8; 8]) -> Option<usize> {
    s.list[..s.count].iter().position(|slot| id[0] != 0 && &slot.id == id)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        std::process::exit(2);
    }
    PATHS.set((args[1].clone(), args[2].clone())).unwrap();
    let Ok(image) = std::fs::read(&args[1]) else { std::process::exit(2) };
    let Ok(flash) = std::fs::read(&args[2]) else { std::process::exit(2) };
    *FLASH.lock().unwrap() = flash;
    let image: &'static mut [u8] = Vec::leak(image);
    CARD_SECTORS.store((image.len() / 512) as u32, Ordering::Relaxed);
    CARD.set(image.as_ptr() as usize).unwrap();
    card::attach(image);
    card::hooks(Some(read_hook), Some(write_hook));
    CUT_AFTER.store(args[3].parse().unwrap_or(-1), Ordering::Relaxed);
    let sectors = CARD_SECTORS.load(Ordering::Relaxed);

    let mut s = Box::new(Slots::new());
    let mut error = s.open(sectors, &UID, JEDEC, BOARD).err();
    let what = args[4].as_str();
    if error.is_none() {
        error = match what {
            "save" => s.save_new(&TestFlash, &mut Check, args[5].as_bytes()).err(),
            "load" => {
                let i = named(&s, &args[5]);
                s.load(&TestFlash, &mut Check, i).err()
            }
            "delete" => {
                let i = named(&s, &args[5]);
                s.delete(i).err()
            }
            _ => None,
        };
    }
    if let Some(e) = error {
        println!("error {e} {}", OPS.load(Ordering::Relaxed));
        save_and_exit(3);
    }
    // Read it all again, as the unit would after a restart.
    let mut s = Box::new(Slots::new());
    if let Err(e) = s.open(sectors, &UID, JEDEC, BOARD) {
        println!("error {e}");
        save_and_exit(3);
    }
    let here = s.on_unit(&TestFlash);
    let pending = s.pending();
    let mut line = format!(
        "ok {}\there={}\tpending={}\tactive={}\tslots=",
        OPS.load(Ordering::Relaxed),
        shown(&s, here),
        shown(&s, pending),
        shown(&s, find_id(&s, &s.active))
    );
    for slot in &s.list[..s.count] {
        line += &String::from_utf8_lossy(slot.name());
        line += "|";
    }
    println!("{line}");
    save_and_exit(0)
}
