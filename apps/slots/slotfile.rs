// SPDX-License-Identifier: MIT
//! Firmware and its settings on the card, one file a slot, shared by every unit of the model:
//! /MBOOT/SLOTS/FM1/ or /MBOOT/SLOTS/SMK37/, each slot SLOTnnnn.SLT with its name as the long name, NAME.slt.
//!
//! A slot holds the firmware's part of the flash only: 0x4000 up to the key area at 0xFF000, and on the SMK-37 not its
//! Bluetooth and calibration sectors at 0xC0000-0xC1FFF. The boot area and the unit's own data are never in a slot,
//! and a load never writes them (as clean-flash's firmware scope). A 512-byte header says which of those sectors the
//! file holds, in order after it; the others are blank (all 0xFF).
//!
//! Header: "SLOT", format 1, board (1 FM-1, 2 SMK-37), header size (u16), sectors stored (u32), the flash chip's JEDEC
//! ID (u32), at 16 a bitmap of 256 bits (bit n: flash sector n is stored), at 48 the name (64 bytes, NUL-padded), at
//! 112 the CRC32 of the stored sectors, at 508 the CRC32 of the 508 bytes before it. Little-endian.
//!
//! Each unit keeps which slot it holds, and a load it hasn't finished, in <8 hex digits of its unique ID>.DAT there.
//!
//! The flash is the caller's ([`Nor`]: m-bootloader's chip on the unit, an image in tests/slots_host.rs), and so are
//! the progress shown and the check that firmware would start ([`Hooks`]).
use core::cell::RefCell;
use kit::app::feed;
use kit::fat::{self, ATTR_ARCHIVE, Entry, Error, Named, Volume};
use kit::mboot::formats::{Flash, Nor, crc16, crc32, le16, le32, put_le16, put_le32};
use kit::Aligned;

pub const SLOT_FLASH_SIZE: u32 = 0x10_0000;
pub const SLOT_NAME_MOST: usize = 40;
pub const MOST_SLOTS: usize = 40;
const SECTOR: u32 = 0x1000;
const CARD_SECTOR: usize = 512;
const HEADER: usize = 512;
const FIRST: u32 = 0x4000;
const END: u32 = 0xf_f000;
const FORMAT: u8 = 1;
const STATE_MAGIC: &[u8; 4] = b"MSLT";
const READ_ERROR: Error = "CARD READ ERROR";
const WRITE_ERROR: Error = "CARD WRITE ERROR";
const FLASH_ERROR: Error = "FLASH ERROR";
const DAMAGED: Error = "SLOT IS DAMAGED";
const MISSING: Error = "MISSING";
const PROGRESS_EVERY: u32 = 8;

/// What the app gives: where progress shows, and the bootloader's check.
pub trait Hooks {
    /// What's being done, in words, and how far it's got.
    fn progress(&mut self, _what: &'static str, _done: u32, _total: u32) {}
    /// Whether an image holds firmware that would start: `image` reads it like flash. (One type for both images, so
    /// the check is built once, as C's took a function to read with.)
    fn valid(&mut self, image: &dyn Flash) -> bool;
}

/// A slot in the list.
#[derive(Clone, Copy)]
pub struct Slot {
    /// The short name, padded with spaces.
    pub id: [u8; 8],
    /// What to show: the long name without .slt, in UTF-8, ending in a zero.
    pub name: [u8; SLOT_NAME_MOST + 1],
    pub file: Entry,
    /// Its header is whole.
    pub sound: bool,
    pub stored: [u8; 32],
    pub sectors: u32,
}

impl Slot {
    pub const EMPTY: Slot = Slot {
        id: [0; 8],
        name: [0; SLOT_NAME_MOST + 1],
        file: Entry { name: [0; 8], start: 0, size: 0 },
        sound: false,
        stored: [0; 32],
        sectors: 0,
    };

    /// The name, without its zero.
    pub fn name(&self) -> &[u8] {
        fat::until_zero(&self.name)
    }
}

/// The card's volume, and the board, which says what a load writes: what reads a slot as if it were flash.
struct Store {
    volume: Volume,
    /// 1 FM-1, 2 SMK-37.
    board: u32,
}

/// The buffers, which the DMA and the card fill.
struct Buffers {
    flash_data: Aligned<{ SECTOR as usize }>,
    file_data: Aligned<{ SECTOR as usize }>,
    header: Aligned<HEADER>,
    state_block: Aligned<CARD_SECTOR>,
    named: [Named; MOST_SLOTS],
}

pub struct Slots {
    store: Store,
    work: Buffers,
    pub folder: u32,
    pub has_folder: bool,
    pub list: [Slot; MOST_SLOTS],
    pub count: usize,
    pub more: bool,
    /// Ids, with no slot as all zeros.
    pub active: [u8; 8],
    pub pending: [u8; 8],
    has_state: bool,
    state_file: Entry,
    unit_name: [u8; 11],
    jedec: u32,
}

/// What a load writes: the firmware's part of the flash, without the SMK-37's Bluetooth and calibration sectors.
fn in_scope(board: u32, address: u32) -> bool {
    if !(FIRST..END).contains(&address) {
        return false;
    }
    board != 2 || !(0xc_0000..0xc_2000).contains(&address)
}

/// The firmware's part of the flash, a sector at a time, with what a load doesn't write left out.
fn firmware_sectors(board: u32) -> impl Iterator<Item = u32> {
    (FIRST..END).step_by(SECTOR as usize).filter(move |&at| in_scope(board, at))
}

fn blank(data: &[u8]) -> bool {
    data.iter().all(|&b| b == 0xff)
}

fn is_stored(stored: &[u8; 32], address: u32) -> bool {
    let n = (address / SECTOR) as usize;
    stored[n / 8] >> (n % 8) & 1 != 0
}

/// Where a stored sector is in its slot's file.
fn file_offset(stored: &[u8; 32], address: u32) -> u32 {
    let before = (0..address / SECTOR).filter(|&i| is_stored(stored, i * SECTOR)).count() as u32;
    HEADER as u32 + before * SECTOR
}

fn lower(c: u8) -> u8 {
    c | if c.is_ascii_uppercase() { 32 } else { 0 }
}

/// Equal, ignoring ASCII case.
fn lower_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| lower(x) == lower(y))
}

/// In order, ignoring ASCII case, with a name before any longer one it starts.
fn name_before(a: &[u8], b: &[u8]) -> bool {
    let (mut a, mut b) = (a.iter().chain(&[0]), b.iter().chain(&[0]));
    loop {
        let (x, y) = (lower(*a.next().unwrap_or(&0)), lower(*b.next().unwrap_or(&0)));
        if x != y || x == 0 {
            return x < y;
        }
    }
}

/// The name to show: the long name without its extension.
fn show_name(out: &mut [u8; SLOT_NAME_MOST + 1], from: &[u8]) {
    let mut n = from.len().min(SLOT_NAME_MOST + 4);
    if n >= 4 && from[n - 4] == b'.' && lower_equal(&from[n - 3..], b"slt") {
        n -= 4;
    }
    let n = n.min(SLOT_NAME_MOST);
    out[..n].copy_from_slice(&from[..n]);
    out[n] = 0;
}

fn with_extension(name: &[u8; 8], extension: &[u8; 3]) -> [u8; 11] {
    let mut out = [b' '; 11];
    out[..8].copy_from_slice(name);
    out[8..].copy_from_slice(extension);
    out
}

/// A name and ".slt" after it: the slot's long name. Returns its length.
fn long_name(name: &[u8], out: &mut [u8; SLOT_NAME_MOST + 4]) -> usize {
    out[..name.len()].copy_from_slice(name);
    out[name.len()..name.len() + 4].copy_from_slice(b".slt");
    name.len() + 4
}

impl Store {
    fn read_header(&mut self, header: &mut [u8; HEADER], slot: &mut Slot) -> bool {
        slot.sound = false;
        if (slot.file.size as usize) < HEADER || !self.volume.read(&slot.file, 0, header) {
            return false;
        }
        if &header[..4] != b"SLOT"
            || header[4] != FORMAT
            || u32::from(header[5]) != self.board
            || usize::from(le16(header, 6)) != HEADER
            || le32(header, 508) != crc32(u32::MAX, &header[..508]) ^ u32::MAX
        {
            return false;
        }
        slot.stored.copy_from_slice(&header[16..48]);
        slot.sectors = le32(header, 8);
        let mut count = 0;
        for at in (0..SLOT_FLASH_SIZE).step_by(SECTOR as usize) {
            if is_stored(&slot.stored, at) {
                if !in_scope(self.board, at) {
                    return false;
                }
                count += 1;
            }
        }
        if count != slot.sectors || slot.file.size != HEADER as u32 + count * SECTOR || !is_stored(&slot.stored, FIRST) {
            return false;
        }
        slot.sound = true;
        true
    }

    /// The flash as it would be after loading the slot: its sectors, blank where it has none, and the unit's own flash
    /// where a load doesn't write. The firmware check reads the boot area's header to find the firmware, so it needs
    /// that.
    fn slot_read(&mut self, flash: &impl Flash, slot: &Slot, mut address: u32, mut to: &mut [u8]) -> bool {
        feed();
        while !to.is_empty() {
            let inside = SECTOR - address % SECTOR;
            let n = (to.len() as u32).min(inside);
            if address >= SLOT_FLASH_SIZE {
                return false;
            }
            let (part, rest) = to.split_at_mut(n as usize);
            if is_stored(&slot.stored, address) {
                if !self.volume.read(&slot.file, file_offset(&slot.stored, address) + address % SECTOR, part) {
                    return false;
                }
            } else if !in_scope(self.board, address) {
                if !flash.read(address, part) {
                    return false;
                }
            } else {
                part.fill(0xff);
            }
            to = rest;
            address += n;
        }
        true
    }

    /// A sector of the firmware's part as the slot holds it.
    fn slot_sector(&mut self, flash: &impl Flash, slot: &Slot, address: u32, out: &mut [u8; SECTOR as usize]) -> bool {
        self.slot_read(flash, slot, address, out)
    }
}

/// A slot, read as if it were the flash after loading it, for the firmware check.
struct SlotImage<'a, F> {
    store: RefCell<&'a mut Store>,
    flash: &'a F,
    slot: &'a Slot,
}

impl<F: Flash> Flash for SlotImage<'_, F> {
    fn capacity(&self) -> u32 {
        SLOT_FLASH_SIZE
    }

    fn read_inside(&self, address: u32, dst: &mut [u8]) -> bool {
        self.store.borrow_mut().slot_read(self.flash, self.slot, address, dst)
    }
}

/// The unit's flash, for the firmware check, feeding the guard as it goes.
struct OnFlash<'a, F>(&'a F);

impl<F: Flash> Flash for OnFlash<'_, F> {
    fn capacity(&self) -> u32 {
        SLOT_FLASH_SIZE
    }

    fn read_inside(&self, address: u32, dst: &mut [u8]) -> bool {
        feed();
        self.0.read(address, dst)
    }
}

fn hex_name(value: u32) -> [u8; 11] {
    let mut out = [b' '; 11];
    for (i, o) in out[..8].iter_mut().enumerate() {
        *o = b"0123456789ABCDEF"[(value >> (28 - 4 * i) & 15) as usize];
    }
    out
}

/// Whether a name can be a slot's: what a computer allows in a file name, at most SLOT_NAME_MOST characters.
pub fn name_ok(name: &[u8]) -> bool {
    let n = name.len();
    if n == 0 || n > SLOT_NAME_MOST {
        return false;
    }
    let mut full = [0u8; SLOT_NAME_MOST + 4];
    let length = long_name(name, &mut full);
    fat::long_name_ok(&full[..length]) && name[0] != b' ' && name[n - 1] != b' ' && name[n - 1] != b'.'
}

impl Slots {
    /// Nothing read yet: all zeros, so a static of it takes no room in the app's file.
    pub const fn new() -> Slots {
        Slots {
            store: Store { volume: Volume::new(), board: 0 },
            work: Buffers {
                flash_data: Aligned([0; SECTOR as usize]),
                file_data: Aligned([0; SECTOR as usize]),
                header: Aligned([0; HEADER]),
                state_block: Aligned([0; CARD_SECTOR]),
                named: [Named::empty(); MOST_SLOTS],
            },
            folder: 0,
            has_folder: false,
            list: [Slot::EMPTY; MOST_SLOTS],
            count: 0,
            more: false,
            active: [0; 8],
            pending: [0; 8],
            has_state: false,
            state_file: Entry { name: [0; 8], start: 0, size: 0 },
            unit_name: [0; 11],
            jedec: 0,
        }
    }

    fn volume(&mut self) -> &mut Volume {
        &mut self.store.volume
    }

    // ---- the list ----

    fn slot_with(&self, id: &[u8; 8]) -> Option<usize> {
        if id[0] == 0 {
            return None;
        }
        self.list[..self.count].iter().position(|s| &s.id == id)
    }

    /// The slot an interrupted load was writing.
    pub fn pending(&self) -> Option<usize> {
        self.slot_with(&self.pending)
    }

    fn refresh(&mut self) -> Result<(), Error> {
        self.count = 0;
        self.more = false;
        if !self.has_folder {
            return Ok(());
        }
        let folder = self.folder;
        let (n, more) = self.store.volume.list_named(folder, b"SLT", &mut self.work.named)?;
        for i in 0..n {
            let slot = &mut self.list[self.count];
            self.count += 1;
            *slot = Slot::EMPTY;
            let named = &self.work.named[i];
            slot.file = named.file;
            slot.id = named.file.name;
            show_name(&mut slot.name, named.name());
            feed();
            self.store.read_header(&mut self.work.header.0, slot);
        }
        self.more = more;
        // In the order of the names shown, ignoring ASCII case, rather than the files' with .slt after them.
        for i in 1..self.count {
            let mut j = i;
            while j > 0 && name_before(self.list[j].name(), self.list[j - 1].name()) {
                self.list.swap(j, j - 1);
                j -= 1;
            }
        }
        Ok(())
    }

    // ---- reading a slot as if it were flash ----

    /// The stored sectors' CRC, as the header says.
    fn check_data(&mut self, hooks: &mut impl Hooks, index: usize) -> Result<(), Error> {
        let slot = self.list[index];
        if !slot.sound {
            return Err(DAMAGED);
        }
        let work = &mut self.work;
        if !self.store.volume.read(&slot.file, 0, &mut work.header.0) {
            return Err(READ_ERROR);
        }
        let mut crc = u32::MAX;
        for i in 0..slot.sectors {
            if i % PROGRESS_EVERY == 0 {
                hooks.progress("CHECKING THE SLOT", i, slot.sectors);
            }
            feed();
            if !self.store.volume.read(&slot.file, HEADER as u32 + i * SECTOR, &mut work.file_data.0) {
                return Err(READ_ERROR);
            }
            crc = crc32(crc, &work.file_data.0);
        }
        if crc ^ u32::MAX == le32(&work.header.0, 112) { Ok(()) } else { Err(DAMAGED) }
    }

    // ---- this unit's state ----

    fn state_name(&self) -> [u8; 11] {
        let mut name = self.unit_name;
        name[8..].copy_from_slice(b"DAT");
        name
    }

    fn read_state(&mut self) -> Result<(), Error> {
        let name = self.state_name();
        let folder = self.folder;
        let Some(file) = self.volume().find(folder, &name)? else { return Ok(()) };
        self.state_file = file;
        if file.size as usize != CARD_SECTOR {
            return Ok(());
        }
        let block = &mut self.work.state_block.0;
        if !self.store.volume.read(&file, 0, block) {
            return Err(READ_ERROR);
        }
        self.has_state = true;
        if &block[..4] == STATE_MAGIC && block[4] == 2 && crc16(0, &block[..26]) == le16(block, 26) {
            self.active.copy_from_slice(&block[8..16]);
            self.pending.copy_from_slice(&block[16..24]);
        }
        Ok(())
    }

    /// One sector of the card, so it's either as it was or as it now is.
    fn write_state(&mut self) -> Result<(), Error> {
        let block = &mut self.work.state_block.0;
        block.fill(0);
        block[..4].copy_from_slice(STATE_MAGIC);
        block[4] = 2;
        block[8..16].copy_from_slice(&self.active);
        block[16..24].copy_from_slice(&self.pending);
        put_le16(block, 26, crc16(0, &block[..26]));
        let volume = &mut self.store.volume;
        if self.has_state {
            return if volume.write(self.state_file.start, 0, block) { Ok(()) } else { Err(WRITE_ERROR) };
        }
        let start = volume.reserve(CARD_SECTOR as u32)?;
        if !volume.write(start, 0, block) {
            return Err(WRITE_ERROR);
        }
        let name = self.state_name();
        let folder = self.folder;
        self.volume().add(folder, &name, ATTR_ARCHIVE, start, CARD_SECTOR as u32)?;
        self.state_file.start = start;
        self.state_file.size = CARD_SECTOR as u32;
        self.has_state = true;
        Ok(())
    }

    /// Mounts, lists the slots, and reads what this unit's last load left. `board` is 1 FM-1 or 2 SMK-37; `jedec` goes
    /// in what's saved.
    pub fn open(&mut self, card_sectors: u32, uid: &[u8; 8], jedec: u32, board: u32) -> Result<(), Error> {
        self.store.board = board;
        self.count = 0;
        self.more = false;
        self.has_folder = false;
        self.has_state = false;
        self.jedec = jedec;
        self.active = [0; 8];
        self.pending = [0; 8];
        self.unit_name = hex_name(crc32(u32::MAX, uid) ^ u32::MAX);
        let volume = self.volume();
        volume.mount(card_sectors)?;
        // Nothing's made until there's something to save.
        let mut c = volume.root();
        let found = volume
            .folder(&mut c, b"MBOOT      ", MISSING)
            .and_then(|()| volume.folder(&mut c, b"SLOTS      ", MISSING))
            .and_then(|()| volume.folder(&mut c, model_folder(board), MISSING));
        match found {
            Err(MISSING) => return Ok(()),
            Err(e) => return Err(e),
            Ok(()) => {}
        }
        self.folder = c;
        self.has_folder = true;
        self.read_state()?;
        self.refresh()
    }

    fn make_folders(&mut self) -> Result<(), Error> {
        if self.has_folder {
            return Ok(());
        }
        let board = self.store.board;
        let volume = self.volume();
        let mut c = volume.root();
        volume.make_folder(&mut c, b"MBOOT      ")?;
        volume.make_folder(&mut c, b"SLOTS      ")?;
        volume.make_folder(&mut c, model_folder(board))?;
        self.folder = c;
        self.has_folder = true;
        Ok(())
    }

    // ---- comparing ----

    /// Whether a slot and the flash hold the same thing, as far as a load is concerned.
    fn same_as_flash(&mut self, flash: &impl Flash, index: usize) -> Result<bool, Error> {
        let slot = &self.list[index];
        if !slot.sound {
            return Ok(false);
        }
        let work = &mut self.work;
        for at in firmware_sectors(self.store.board) {
            if !flash.read(at, &mut work.flash_data.0) {
                return Err(FLASH_ERROR);
            }
            if !is_stored(&slot.stored, at) {
                feed();
                if !blank(&work.flash_data.0) {
                    return Ok(false);
                }
                continue;
            }
            if !self.store.slot_sector(flash, slot, at, &mut work.file_data.0) {
                return Err(READ_ERROR);
            }
            if work.file_data.0 != work.flash_data.0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The slot holding what's on the unit. It leaves out what a load doesn't write.
    pub fn on_unit(&mut self, flash: &impl Flash) -> Option<usize> {
        // The active slot is the likely one, and differing slots stop at their first sector.
        let active = self.slot_with(&self.active);
        if let Some(a) = active
            && self.same_as_flash(flash, a) == Ok(true)
        {
            return active;
        }
        (0..self.count).find(|&i| Some(i) != active && self.same_as_flash(flash, i) == Ok(true))
    }

    // ---- saving ----

    fn name_taken(&self, name: &[u8]) -> bool {
        self.list[..self.count].iter().any(|s| lower_equal(s.name(), name))
    }

    /// A name no slot has: base, or base with a number after it, ending in a zero. Returns its length.
    pub fn free_name(&self, base: &[u8], out: &mut [u8; SLOT_NAME_MOST + 1]) -> usize {
        let mut n = base.len().min(SLOT_NAME_MOST);
        out[..n].copy_from_slice(&base[..n]);
        out[n] = 0;
        if !self.name_taken(&out[..n]) {
            return n;
        }
        // Room for " 999", and no space left before it.
        n = n.min(SLOT_NAME_MOST - 4);
        while n > 0 && out[n - 1] == b' ' {
            n -= 1;
        }
        let mut m = n;
        for k in 2..1000u32 {
            m = n;
            out[m] = b' ';
            m += 1;
            if k >= 100 {
                out[m] = b'0' + (k / 100) as u8;
                m += 1;
            }
            if k >= 10 {
                out[m] = b'0' + (k / 10 % 10) as u8;
                m += 1;
            }
            out[m] = b'0' + (k % 10) as u8;
            m += 1;
            out[m] = 0;
            if !self.name_taken(&out[..m]) {
                return m;
            }
        }
        m
    }

    /// What's on the unit, the firmware's part of it, into a new chain: the header, then the sectors that aren't
    /// blank. It's read back before anything names it. On error the chain is gone again. Returns its start and size.
    fn copy_flash(&mut self, flash: &impl Flash, hooks: &mut impl Hooks, name: &[u8]) -> Result<(u32, u32), Error> {
        let board = self.store.board;
        let work = &mut self.work;
        let mut stored = [0u8; 32];
        let mut count = 0;
        for at in firmware_sectors(board) {
            feed();
            if !flash.read(at, &mut work.flash_data.0) {
                return Err(FLASH_ERROR);
            }
            if at == FIRST || !blank(&work.flash_data.0) {
                stored[(at / SECTOR / 8) as usize] |= 1 << (at / SECTOR % 8);
                count += 1;
            }
        }
        let size = HEADER as u32 + count * SECTOR;
        let volume = &mut self.store.volume;
        let start = volume.reserve(size)?;
        let mut error = None;
        let (mut crc, mut done) = (u32::MAX, 0);
        for at in (FIRST..END).step_by(SECTOR as usize) {
            if error.is_some() {
                break;
            }
            if !is_stored(&stored, at) {
                continue;
            }
            if done % PROGRESS_EVERY == 0 {
                hooks.progress("SAVING", done, count);
            }
            feed();
            if !flash.read(at, &mut work.flash_data.0) {
                error = Some(FLASH_ERROR);
            } else if !volume.write(start, HEADER as u32 + done * SECTOR, &work.flash_data.0) {
                error = Some(WRITE_ERROR);
            }
            crc = crc32(crc, &work.flash_data.0);
            done += 1;
        }
        if error.is_none() {
            let header = &mut work.header.0;
            header.fill(0);
            header[..4].copy_from_slice(b"SLOT");
            header[4] = FORMAT;
            header[5] = board as u8;
            put_le16(header, 6, HEADER as u16);
            put_le32(header, 8, count);
            put_le32(header, 12, self.jedec);
            header[16..48].copy_from_slice(&stored);
            for (h, &c) in header[48..48 + 63].iter_mut().zip(name.iter().take_while(|&&c| c != 0)) {
                *h = c;
            }
            put_le32(header, 112, crc ^ u32::MAX);
            put_le32(header, 508, crc32(u32::MAX, &header[..508]) ^ u32::MAX);
            if !volume.write(start, 0, header) {
                error = Some(WRITE_ERROR);
            }
        }
        let mut copy = Slot::EMPTY;
        copy.file.start = start;
        copy.file.size = size;
        copy.stored = stored;
        copy.sectors = count;
        if error.is_none() && !self.store.read_header(&mut work.header.0, &mut copy) {
            error = Some("CARD WROTE WRONGLY");
        }
        done = 0;
        for at in (FIRST..END).step_by(SECTOR as usize) {
            if error.is_some() {
                break;
            }
            if !is_stored(&stored, at) {
                continue;
            }
            if done % PROGRESS_EVERY == 0 {
                hooks.progress("CHECKING", done, count);
            }
            done += 1;
            if !self.store.slot_sector(flash, &copy, at, &mut work.file_data.0) {
                error = Some(READ_ERROR);
            } else if !flash.read(at, &mut work.flash_data.0) {
                error = Some(FLASH_ERROR);
            } else if work.file_data.0 != work.flash_data.0 {
                error = Some("CARD WROTE WRONGLY");
            }
        }
        if let Some(e) = error {
            let volume = &mut self.store.volume;
            let _ = volume.release(start);
            let _ = volume.commit();
            return Err(e);
        }
        Ok((start, size))
    }

    /// SLOT0001 and on, the first no file here has.
    fn free_id(&self) -> [u8; 8] {
        let mut id = *b"SLOT0000";
        for n in 1..10000u32 {
            id[4] = b'0' + (n / 1000) as u8;
            id[5] = b'0' + (n / 100 % 10) as u8;
            id[6] = b'0' + (n / 10 % 10) as u8;
            id[7] = b'0' + (n % 10) as u8;
            if self.slot_with(&id).is_none() {
                break;
            }
        }
        id
    }

    /// Saves what's on the unit as a new slot with this name (made unique if a slot has it already).
    pub fn save_new(&mut self, flash: &impl Flash, hooks: &mut impl Hooks, wanted: &[u8]) -> Result<(), Error> {
        if self.count >= MOST_SLOTS {
            return Err("TOO MANY SLOTS");
        }
        if !name_ok(wanted) {
            return Err("NAME NOT ALLOWED");
        }
        let mut name = [0u8; SLOT_NAME_MOST + 1];
        let n = self.free_name(wanted, &mut name);
        let mut full = [0u8; SLOT_NAME_MOST + 4];
        let full_length = long_name(&name[..n], &mut full);
        self.make_folders()?;
        let (start, size) = self.copy_flash(flash, hooks, &name[..n])?;
        let id = self.free_id();
        let folder = self.folder;
        let volume = self.volume();
        if let Err(e) = volume.add_named(folder, &with_extension(&id, b"SLT"), &full[..full_length], ATTR_ARCHIVE, start, size) {
            // Nothing names it, so it would only be lost.
            let _ = volume.release(start);
            let _ = volume.commit();
            return Err(e);
        }
        self.active = id;
        self.write_state()?;
        self.volume().commit()?;
        self.refresh()
    }

    /// Settings go over the slot they came from: a new chain, then its entry points at it. Its name stays.
    fn save_over(&mut self, flash: &impl Flash, hooks: &mut impl Hooks, index: usize) -> Result<(), Error> {
        let slot = self.list[index];
        let (start, size) = self.copy_flash(flash, hooks, slot.name())?;
        let folder = self.folder;
        let volume = self.volume();
        volume.swap(folder, &with_extension(&slot.id, b"SLT"), start, size)?;
        volume.commit()?;
        self.refresh()
    }

    /// What's on the unit, saved unless a slot holds it, or it isn't valid: an interrupted load can leave that, and the
    /// slot being loaded then holds the only good copy. Settings go over the slot they came from, but firmware that
    /// isn't that slot's gets its own. So does what's on the unit when that slot is the one being loaded, or there'd
    /// be nothing to go back to.
    fn save_first(&mut self, flash: &impl Flash, hooks: &mut impl Hooks, target: &[u8; 8]) -> Result<(), Error> {
        if self.on_unit(flash).is_some() || !hooks.valid(&OnFlash(flash)) {
            return Ok(());
        }
        if let Some(active) = self.slot_with(&self.active)
            && &self.active != target
            && self.list[active].sound
        {
            let work = &mut self.work;
            if !self.store.slot_sector(flash, &self.list[active], FIRST, &mut work.file_data.0) {
                return Err(READ_ERROR);
            }
            if !flash.read(FIRST, &mut work.flash_data.0) {
                return Err(FLASH_ERROR);
            }
            if work.file_data.0 == work.flash_data.0 {
                return self.save_over(flash, hooks, active);
            }
        }
        let mut name = [0u8; SLOT_NAME_MOST + 1];
        let n = self.free_name(b"Saved", &mut name);
        self.save_new(flash, hooks, &name[..n])
    }

    // ---- loading ----

    /// A sector of the slot onto the flash: what differs is erased and written, and a blank sector only erased.
    fn write_sector(&mut self, flash: &impl Nor, slot: &Slot, at: u32, erased: bool) -> Result<(), Error> {
        let work = &mut self.work;
        if !self.store.slot_sector(flash, slot, at, &mut work.file_data.0) {
            return Err(READ_ERROR);
        }
        if !flash.read(at, &mut work.flash_data.0) {
            return Err(FLASH_ERROR);
        }
        if work.file_data.0 == work.flash_data.0 && !erased {
            return Ok(());
        }
        feed();
        if !erased && !flash.erase(at, SECTOR) {
            return Err(FLASH_ERROR);
        }
        if !blank(&work.file_data.0) && !flash.program(at, &work.file_data.0) {
            return Err(FLASH_ERROR);
        }
        if !flash.read(at, &mut work.flash_data.0) || work.file_data.0 != work.flash_data.0 {
            return Err(FLASH_ERROR);
        }
        Ok(())
    }

    /// Saves what's on the unit, then writes this slot to it. Only returns with no error once it's all done.
    pub fn load(&mut self, flash: &impl Nor, hooks: &mut impl Hooks, index: usize) -> Result<(), Error> {
        if index >= self.count {
            return Err(DAMAGED);
        }
        // Ids, as saving can move things in the list.
        let target = self.list[index].id;
        hooks.progress("CHECKING THE SLOT", 0, 1);
        if !self.list[index].sound {
            return Err(DAMAGED);
        }
        self.check_data(hooks, index)?;
        let image = SlotImage { store: RefCell::new(&mut self.store), flash, slot: &self.list[index] };
        if !hooks.valid(&image) {
            return Err(DAMAGED);
        }
        self.save_first(flash, hooks, &target)?;
        let moved = self.slot_with(&target).ok_or(DAMAGED)?;
        let slot = self.list[moved];
        self.pending = target;
        self.write_state()?;
        // Only what differs, with nothing erased until something does.
        let mut any = false;
        let board = self.store.board;
        for at in firmware_sectors(board) {
            if any {
                break;
            }
            let work = &mut self.work;
            if !self.store.slot_sector(flash, &slot, at, &mut work.file_data.0) {
                return Err(READ_ERROR);
            }
            if !flash.read(at, &mut work.flash_data.0) {
                return Err(FLASH_ERROR);
            }
            any = work.file_data.0 != work.flash_data.0;
        }
        if any {
            // The first firmware sector is erased first and written last, so what's interrupted can't pass as
            // firmware.
            feed();
            if !flash.erase(FIRST, SECTOR) {
                return Err(FLASH_ERROR);
            }
            for at in firmware_sectors(board).skip(1) {
                if at / SECTOR % PROGRESS_EVERY == 0 {
                    hooks.progress("WRITING", at / SECTOR, END / SECTOR);
                }
                self.write_sector(flash, &slot, at, false)?;
            }
            self.write_sector(flash, &slot, FIRST, true)?;
        }
        self.active = target;
        self.pending = [0; 8];
        self.write_state()?;
        self.volume().commit()
    }

    pub fn delete(&mut self, index: usize) -> Result<(), Error> {
        if index >= self.count {
            return Err(DAMAGED);
        }
        let id = self.list[index].id;
        let folder = self.folder;
        self.volume().remove(folder, &with_extension(&id, b"SLT"))?;
        if self.active == id {
            self.active = [0; 8];
        }
        if self.pending == id {
            self.pending = [0; 8];
        }
        if self.has_state {
            self.write_state()?;
        }
        self.volume().commit()?;
        self.refresh()
    }
}

impl Default for Slots {
    fn default() -> Self {
        Self::new()
    }
}

/// The board's folder in /MBOOT/SLOTS.
fn model_folder(board: u32) -> &'static [u8; 11] {
    if board == 2 { b"SMK37      " } else { b"FM1        " }
}
