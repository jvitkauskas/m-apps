// SPDX-License-Identifier: MIT
//! A FAT32 volume on the card, in the first partition or with no partition table. What's on the card can't be
//! trusted, so everything here is bounded, and a loop in the FAT ends with an error. What fails returns what to show
//! ([`Error`]).
//!
//! Changing the volume keeps to an order in which losing power at any write leaves, at worst, clusters no file uses,
//! which fsck can reclaim. A file a directory names is complete, either as it was or as it now is:
//! - a new file: [`Volume::reserve`], [`Volume::write`], [`Volume::add`];
//! - a file replaced: [`Volume::reserve`], [`Volume::write`], [`Volume::swap`], which changes one directory sector,
//!   then frees the old one;
//! - a file removed: [`Volume::remove`] takes it out of the directory, then frees its clusters.
//!
//! Call [`Volume::commit`] at the end, which updates the volume's count of free clusters.
use crate::{Aligned, card};

/// What to show when something fails.
pub type Error = &'static str;
pub type Result<T> = core::result::Result<T, Error>;

pub const READ_ERROR: Error = "CARD READ ERROR";
pub const NOT_FAT32: Error = "CARD IS NOT FAT32";
pub const BAD_FOLDER: Error = "BAD FOLDER ON CARD";
pub const WRITE_ERROR: Error = "CARD WRITE ERROR";
pub const BAD_NAME: Error = "NAME NOT ALLOWED";
pub const CARD_FULL: Error = "CARD IS FULL";

pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_FOLDER: u8 = 0x10;
pub const ATTR_LABEL: u8 = 0x08;
/// A long name is at most this many pieces of 13 characters ([`Volume::add_named`]).
pub const LONG_PIECES: usize = 5;
/// A name as a computer shows it, in UTF-8, with room for its terminating zero ([`Named`]).
pub const NAME_BYTES: usize = 72;

const SECTOR: usize = 512;
const ENTRY: usize = 32;
const FAT_PER_SECTOR: u32 = 128;
/// FAT32's limit for a directory: 2 MiB.
const MOST_ENTRIES: u32 = 65536;
/// No FAT sector cached: sector 0 never holds the FAT, which comes after the reserved sectors.
const NO_SECTOR: u32 = 0;
const FSINFO_SIGNATURE: u32 = 0x4161_5252;
const FSINFO_SIGNATURE2: u32 = 0x6141_7272;
const NO_COUNT: u32 = u32::MAX;
const END_OF_CHAIN: u32 = 0x0fff_ffff;
/// No clock on the unit: 2026-01-01.
const DOS_DATE: u16 = 0x5c21;
const LONG_PIECE: usize = 13;
const MOST_LONG_PIECES: usize = 20;
const LONG_FLAG_LAST: u8 = 0x40;
const LONG_ATTRIBUTES: u8 = 0x0f;
const LONG_PLACES: [usize; LONG_PIECE] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
/// Entries to add at once: up to LONG_PIECES long-name pieces, then the short entry.
const MOST_ADDED: usize = LONG_PIECES + 1;

/// A file or folder in a directory: its 8.3 name's first 8 bytes, its first cluster and its size.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
#[repr(C)]
pub struct Entry {
    pub name: [u8; 8],
    pub start: u32,
    pub size: u32,
}

/// A file with its name as a computer shows it ([`Volume::list_named`]), ending in a zero.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Named {
    pub file: Entry,
    pub name: [u8; NAME_BYTES],
}

impl Named {
    pub const fn empty() -> Named {
        Named { file: Entry { name: [0; 8], start: 0, size: 0 }, name: [0; NAME_BYTES] }
    }

    /// The name, without its zero.
    pub fn name(&self) -> &[u8] {
        until_zero(&self.name)
    }
}

/// The bytes before the first zero.
pub fn until_zero(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |n| &s[..n])
}

fn le16(b: &[u8], at: usize) -> u32 {
    u32::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn entry_start(e: &[u8]) -> u32 {
    le16(e, 26) | le16(e, 20) << 16
}

fn short_sum(e: &[u8]) -> u8 {
    e[..11].iter().fold(0u8, |sum, &c| sum.rotate_right(1).wrapping_add(c))
}

fn looks_fat32(b: &[u8; SECTOR]) -> bool {
    let per = b[13];
    le16(b, 510) == 0xaa55
        && le16(b, 11) == SECTOR as u32
        && per.is_power_of_two()
        && le16(b, 14) != 0
        && (b[16] == 1 || b[16] == 2)
        && le16(b, 17) == 0
        && le16(b, 19) == 0
        && le16(b, 22) == 0
        && le32(b, 36) != 0
}

/// Where a directory walk is, for what a visit finds.
#[derive(Default)]
struct Walk {
    sector: u32,
    previous: u32,
    at: usize,
    /// The long name being read: pieces come last first, each with the short name's checksum.
    long_next: u32,
    long_sum: u8,
    long_whole: bool,
}

/// A mounted volume, with its two sector buffers.
pub struct Volume {
    block: Aligned<SECTOR>,
    fat_block: Aligned<SECTOR>,
    long_units: [u16; LONG_PIECE * MOST_LONG_PIECES + 1],
    added: [u8; MOST_ADDED * ENTRY],
    fat_at: u32,
    fat_cached: u32,
    data_at: u32,
    clusters: u32,
    root: u32,
    fat_sectors: u32,
    info_sector: u32,
    first_free: u32,
    per_cluster: u32,
    fat_copies: u32,
    fat_dirty: bool,
    cursor_start: u32,
    cursor_index: u32,
    cursor_cluster: u32,
    freed: i32,
}

/// The long name the entries before one gave, for a visit.
struct Long<'a> {
    units: &'a [u16],
    whole: bool,
    sum: u8,
}

/// What a directory walk looks for: each entry that isn't deleted, a long-name piece or a label goes to it, and
/// what it returns ends the walk. As a trait object, an app has only the visits it makes.
trait Visitor {
    fn visit(&mut self, e: &[u8; ENTRY], long: &Long<'_>) -> Option<Found>;
}

/// A folder of this 8.3 name.
struct FindFolder<'a>(&'a [u8; 11]);

impl Visitor for FindFolder<'_> {
    fn visit(&mut self, e: &[u8; ENTRY], _: &Long<'_>) -> Option<Found> {
        (e[11] & ATTR_FOLDER != 0 && e[..11] == self.0[..]).then(|| Found { start: entry_start(e), size: 0 })
    }
}

/// A file of this name: never a folder or the volume's label, which swap() and remove() would wreck.
struct FindFile<'a>(&'a [u8; 11]);

impl Visitor for FindFile<'_> {
    fn visit(&mut self, e: &[u8; ENTRY], _: &Long<'_>) -> Option<Found> {
        (e[11] & (ATTR_FOLDER | ATTR_LABEL) == 0 && e[..11] == self.0[..])
            .then(|| Found { start: entry_start(e), size: le32(e, 28) })
    }
}

/// Files with this extension, into a list in name order.
struct ListFiles<'a, 'b>(&'a [u8; 3], List<'b, Entry>);

impl Visitor for ListFiles<'_, '_> {
    fn visit(&mut self, e: &[u8; ENTRY], _: &Long<'_>) -> Option<Found> {
        if e[11] & ATTR_FOLDER == 0 && e[8..11] == self.0[..] {
            insert(&mut self.1, e);
        }
        None
    }
}

/// Folders, without . and .., into a list in name order.
struct ListFolders<'b>(List<'b, Entry>);

impl Visitor for ListFolders<'_> {
    fn visit(&mut self, e: &[u8; ENTRY], _: &Long<'_>) -> Option<Found> {
        if e[11] & ATTR_FOLDER != 0 && e[0] != b'.' {
            insert(&mut self.0, e);
        }
        None
    }
}

/// Files with this extension, with names as a computer shows them.
struct ListNamed<'a, 'b>(&'a [u8; 3], List<'b, Named>);

impl Visitor for ListNamed<'_, '_> {
    fn visit(&mut self, e: &[u8; ENTRY], long: &Long<'_>) -> Option<Found> {
        if e[11] & ATTR_FOLDER == 0 && e[8..11] == self.0[..] {
            let mut name = [0u8; NAME_BYTES];
            if long.whole && long.sum == short_sum(e) {
                long_name_utf8(long.units, &mut name);
            } else {
                short_name(&mut name, e);
            }
            let new = until_zero(&name);
            if let Some(at) = self.1.place(|n| cmp_upper(n.name(), new).is_lt()) {
                let n = &mut self.1.entries[at];
                n.file.name.copy_from_slice(&e[..8]);
                n.file.start = entry_start(e);
                n.file.size = le32(e, 28);
                n.name = name;
            }
        }
        None
    }
}

/// A list a walk fills: the first `entries.len()` in name order, and whether there were more.
struct List<'a, T> {
    entries: &'a mut [T],
    count: usize,
    more: bool,
}

impl<T: Copy> List<'_, T> {
    /// Where an entry goes, if it's among the first by name, with room made for it.
    fn place(&mut self, before: impl Fn(&T) -> bool) -> Option<usize> {
        let max = self.entries.len();
        let at = self.entries[..self.count].iter().position(|e| !before(e)).unwrap_or(self.count);
        if self.count == max {
            self.more = true;
            if at == max {
                return None;
            }
            self.count -= 1;
        }
        self.entries.copy_within(at..self.count, at + 1);
        self.count += 1;
        Some(at)
    }
}

/// What a walk found, for find(), folder(), swap() and remove().
#[derive(Clone, Copy)]
struct Found {
    start: u32,
    size: u32,
}

fn cmp_upper(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    let up = |c: u8| c.to_ascii_uppercase();
    a.iter().map(|&c| up(c)).cmp(b.iter().map(|&c| up(c)))
}

/// Stops between characters, and leaves a terminator.
fn long_name_utf8(units: &[u16], out: &mut [u8; NAME_BYTES]) {
    let mut at = 0;
    let mut i = 0;
    while i < LONG_PIECE * MOST_LONG_PIECES && units[i] != 0 && units[i] != 0xffff {
        let mut c = u32::from(units[i]);
        let low = u32::from(units[i + 1]);
        if (0xd800..0xdc00).contains(&c) && (0xdc00..0xe000).contains(&low) {
            c = 0x10000 + ((c - 0xd800) << 10) + (low - 0xdc00);
            i += 1;
        } else if (0xd800..0xe000).contains(&c) {
            c = 0xfffd;
        }
        let n = match c {
            0..0x80 => 1,
            0x80..0x800 => 2,
            0x800..0x10000 => 3,
            _ => 4,
        };
        if at + n >= NAME_BYTES {
            break;
        }
        const LEAD: [u8; 5] = [0, 0, 0xc0, 0xe0, 0xf0];
        if n == 1 {
            out[at] = c as u8;
        } else {
            out[at] = LEAD[n] | (c >> (6 * (n - 1))) as u8;
            for b in 1..n {
                out[at + b] = 0x80 | (c >> (6 * (n - 1 - b)) & 0x3f) as u8;
            }
        }
        at += n;
        i += 1;
    }
    out[at] = 0;
}

/// A computer may keep a short name's case in two bits of its entry, rather than a long name. Bytes past ASCII are
/// in a code page the name doesn't say, so they can't be turned into UTF-8.
fn short_char(c: u8, lower: bool) -> u8 {
    if c >= 0x80 {
        b'?'
    } else if lower {
        c.to_ascii_lowercase()
    } else {
        c
    }
}

fn short_name(out: &mut [u8; NAME_BYTES], e: &[u8]) {
    let mut at = 0;
    for &c in e[..8].iter().take_while(|&&c| c != b' ') {
        out[at] = short_char(c, e[12] & 0x08 != 0);
        at += 1;
    }
    if e[8] != b' ' {
        out[at] = b'.';
        at += 1;
    }
    for &c in e[8..11].iter().take_while(|&&c| c != b' ') {
        out[at] = short_char(c, e[12] & 0x10 != 0);
        at += 1;
    }
    out[at] = 0;
}

impl Volume {
    /// Not mounted yet: all zeros, so a static of it takes no room in the app's file.
    pub const fn new() -> Volume {
        Volume {
            block: Aligned([0; SECTOR]),
            fat_block: Aligned([0; SECTOR]),
            long_units: [0; LONG_PIECE * MOST_LONG_PIECES + 1],
            added: [0; MOST_ADDED * ENTRY],
            fat_at: 0,
            fat_cached: NO_SECTOR,
            data_at: 0,
            clusters: 0,
            root: 0,
            fat_sectors: 0,
            info_sector: 0,
            first_free: 0,
            per_cluster: 0,
            fat_copies: 0,
            fat_dirty: false,
            cursor_start: 0,
            cursor_index: 0,
            cursor_cluster: 0,
            freed: 0,
        }
    }

    /// Mounts the volume on a card of this many sectors, from card::start_card().
    pub fn mount(&mut self, sectors: u32) -> Result<()> {
        let b = &mut self.block.0;
        let mut base = 0;
        if !card::read(0, b) {
            return Err(READ_ERROR);
        }
        if !looks_fat32(b) {
            if le16(b, 510) != 0xaa55 {
                return Err(NOT_FAT32);
            }
            base = le32(b, 454);
            if base >= sectors {
                return Err(NOT_FAT32);
            }
            if !card::read(base, b) {
                return Err(READ_ERROR);
            }
            if !looks_fat32(b) {
                return Err(NOT_FAT32);
            }
        }
        let (total, reserved, fat_size, fats) = (le32(b, 32), le16(b, 14), le32(b, 36), u32::from(b[16]));
        if total > sectors - base || fat_size > total / fats || reserved + fats * fat_size >= total {
            return Err(NOT_FAT32);
        }
        let per_cluster = u32::from(b[13]);
        let clusters = (total - reserved - fats * fat_size) / per_cluster;
        // Past FAT32's 28 bits, a cluster number can't be told from its end-of-chain mark.
        if clusters == 0
            || clusters > 0x0fff_fff0
            || fat_size > u32::MAX / FAT_PER_SECTOR
            || fat_size * FAT_PER_SECTOR < clusters + 2
        {
            return Err(NOT_FAT32);
        }
        self.per_cluster = per_cluster;
        self.fat_at = base + reserved;
        self.data_at = self.fat_at + fats * fat_size;
        self.clusters = clusters;
        self.root = le32(b, 44);
        self.fat_sectors = fat_size;
        self.fat_copies = fats;
        self.info_sector = if le16(b, 48) != 0 { base + le16(b, 48) } else { 0 };
        self.fat_cached = NO_SECTOR;
        self.fat_dirty = false;
        self.cursor_cluster = 0;
        self.first_free = 2;
        self.freed = 0;
        if self.info_sector != 0 {
            let b = &mut self.block.0;
            if !card::read(self.info_sector, b) {
                return Err(READ_ERROR);
            }
            let hint = le32(b, 492);
            if le32(b, 0) == FSINFO_SIGNATURE && le32(b, 484) == FSINFO_SIGNATURE2 && hint >= 2 && hint - 2 < clusters {
                self.first_free = hint;
            }
        }
        Ok(())
    }

    /// The root directory's cluster.
    pub fn root(&self) -> u32 {
        self.root
    }

    fn cluster_ok(&self, c: u32) -> bool {
        c >= 2 && c - 2 < self.clusters
    }

    fn is_end(c: u32) -> bool {
        c >= 0x0fff_fff8
    }

    fn cluster_sector(&self, c: u32) -> u32 {
        self.data_at + (c - 2) * self.per_cluster
    }

    fn cluster_bytes(&self) -> u32 {
        self.per_cluster * SECTOR as u32
    }

    /// The cached sector, in every copy of the FAT.
    fn flush_fat(&mut self) -> bool {
        if !self.fat_dirty {
            return true;
        }
        for copy in 0..self.fat_copies {
            if !card::write(self.fat_cached + copy * self.fat_sectors, &self.fat_block.0) {
                return false;
            }
        }
        self.fat_dirty = false;
        true
    }

    fn fat_sector(&self, c: u32) -> u32 {
        self.fat_at + c / FAT_PER_SECTOR
    }

    /// The next cluster, a value that isn't one, or 0 if that can't be read.
    fn next(&mut self, c: u32) -> u32 {
        let sector = self.fat_sector(c);
        if sector != self.fat_cached {
            if !self.flush_fat() {
                return 0;
            }
            self.fat_cached = NO_SECTOR;
            if !card::read(sector, &mut self.fat_block.0) {
                return 0;
            }
            self.fat_cached = sector;
        }
        le32(&self.fat_block.0, (c % FAT_PER_SECTOR * 4) as usize) & 0x0fff_ffff
    }

    /// Whether the FAT said c is free, rather than failing to be read.
    fn free(&mut self, c: u32) -> Result<bool> {
        let value = self.next(c);
        if value == 0 && self.fat_cached != self.fat_sector(c) {
            return Err(READ_ERROR);
        }
        Ok(value == 0)
    }

    /// Keeps the top four bits, which FAT32 reserves.
    fn set_next(&mut self, c: u32, value: u32) -> bool {
        if c == 0 || (self.next(c) == 0 && self.fat_cached != self.fat_sector(c)) {
            return false;
        }
        let at = (c % FAT_PER_SECTOR * 4) as usize;
        let old = le32(&self.fat_block.0, at);
        put32(&mut self.fat_block.0, at, (old & 0xf000_0000) | value);
        self.fat_dirty = true;
        true
    }

    fn long_piece(&mut self, w: &mut Walk, e: &[u8]) {
        let piece = u32::from(e[0] & (LONG_FLAG_LAST - 1));
        if e[0] & LONG_FLAG_LAST != 0 {
            w.long_whole = false;
            w.long_next = piece;
            w.long_sum = e[13];
            if (1..=MOST_LONG_PIECES as u32).contains(&piece) {
                self.long_units[piece as usize * LONG_PIECE] = 0;
            } else {
                w.long_next = 0;
            }
        }
        if w.long_next == 0 || piece != w.long_next || e[13] != w.long_sum {
            w.long_next = 0;
            return;
        }
        let first = (piece as usize - 1) * LONG_PIECE;
        for (i, &place) in LONG_PLACES.iter().enumerate() {
            self.long_units[first + i] = le16(e, place) as u16;
        }
        w.long_next -= 1;
        if w.long_next == 0 {
            w.long_whole = true;
        }
    }

    /// Reads a directory, from its first cluster, for what `visit` looks for. `w` says where it stopped.
    fn walk(&mut self, mut c: u32, visitor: &mut dyn Visitor, w: &mut Walk) -> Result<Option<Found>> {
        let mut seen = 0u32;
        if !self.cluster_ok(c) {
            return Err(BAD_FOLDER);
        }
        loop {
            for s in 0..self.per_cluster {
                w.previous = w.sector;
                w.sector = self.cluster_sector(c) + s;
                if !card::read(w.sector, &mut self.block.0) {
                    return Err(READ_ERROR);
                }
                for at in (0..SECTOR).step_by(ENTRY) {
                    w.at = at;
                    let mut e = [0u8; ENTRY];
                    e.copy_from_slice(&self.block.0[at..at + ENTRY]);
                    if e[0] == 0 {
                        return Ok(None);
                    }
                    seen += 1;
                    if seen > MOST_ENTRIES {
                        return Err(BAD_FOLDER);
                    }
                    if e[0] != 0xe5 && e[11] == LONG_ATTRIBUTES {
                        self.long_piece(w, &e);
                        continue;
                    }
                    // Deleted, or a volume label.
                    if e[0] != 0xe5 && e[11] & ATTR_LABEL == 0 {
                        let long = Long { units: &self.long_units, whole: w.long_whole, sum: w.long_sum };
                        if let Some(found) = visitor.visit(&e, &long) {
                            return Ok(Some(found));
                        }
                    }
                    // A long name belongs only to the entry right after it.
                    w.long_next = 0;
                    w.long_whole = false;
                }
            }
            crate::app::feed();
            let following = self.next(c);
            if following == 0 {
                return Err(READ_ERROR);
            }
            if Self::is_end(following) {
                return Ok(None);
            }
            if !self.cluster_ok(following) {
                return Err(BAD_FOLDER);
            }
            c = following;
        }
    }

    fn walk_for(&mut self, directory: u32, visitor: &mut dyn Visitor) -> Result<(Option<Found>, Walk)> {
        let mut w = Walk::default();
        let found = self.walk(directory, visitor, &mut w)?;
        Ok((found, w))
    }

    /// Finds a folder in a directory: its 8.3 name, padded with spaces to 11 bytes. `missing` is what to show if it
    /// isn't there.
    pub fn folder(&mut self, cluster: &mut u32, name: &[u8; 11], missing: Error) -> Result<()> {
        let (found, _) = self.walk_for(*cluster, &mut FindFolder(name))?;
        *cluster = found.ok_or(missing)?.start;
        Ok(())
    }

    /// The files with this extension, in name order. With more than `entries` holds, it keeps the first by name and
    /// says so: (how many, whether there were more).
    pub fn list(&mut self, cluster: u32, extension: &[u8; 3], entries: &mut [Entry]) -> Result<(usize, bool)> {
        let mut v = ListFiles(extension, List { entries, count: 0, more: false });
        self.walk_for(cluster, &mut v)?;
        Ok((v.1.count, v.1.more))
    }

    /// The folders in a directory, without . and .., in name order as list() has files: start is the folder's
    /// cluster, and size is 0.
    pub fn list_folders(&mut self, cluster: u32, entries: &mut [Entry]) -> Result<(usize, bool)> {
        let mut v = ListFolders(List { entries, count: 0, more: false });
        self.walk_for(cluster, &mut v)?;
        Ok((v.0.count, v.0.more))
    }

    /// The same as list() with each file's name, as a computer shows it, in UTF-8: its long name if it has one, cut
    /// at NAME_BYTES-1 bytes between characters, and otherwise its short one, as NAME.EXT. They come in name order,
    /// ignoring ASCII case. What to do with characters the screen doesn't have is the app's business.
    pub fn list_named(&mut self, cluster: u32, extension: &[u8; 3], entries: &mut [Named]) -> Result<(usize, bool)> {
        let mut v = ListNamed(extension, List { entries, count: 0, more: false });
        self.walk_for(cluster, &mut v)?;
        Ok((v.1.count, v.1.more))
    }

    /// Reads `to.len()` bytes from `offset`, and no more. False if the card or the file's chain fails, or it's beyond
    /// the file's end.
    pub fn read(&mut self, file: &Entry, mut offset: u32, mut to: &mut [u8]) -> bool {
        let n = to.len() as u32;
        if offset > file.size || n > file.size - offset {
            return false;
        }
        let mut c = file.start;
        let cluster_bytes = self.cluster_bytes();
        let index = offset / cluster_bytes;
        let mut skip = index;
        if !self.cluster_ok(c) {
            return n == 0;
        }
        // Sequential reads go on from the last, as walking a long file's chain each time would get slower and slower.
        if self.cursor_start == file.start && self.cursor_index <= index && self.cursor_cluster != 0 {
            c = self.cursor_cluster;
            skip = index - self.cursor_index;
        }
        while skip > 0 {
            c = self.next(c);
            if !self.cluster_ok(c) {
                return false;
            }
            skip -= 1;
        }
        self.cursor_start = file.start;
        self.cursor_index = index;
        self.cursor_cluster = c;
        offset %= cluster_bytes;
        while !to.is_empty() {
            let sector = self.cluster_sector(c) + offset / SECTOR as u32;
            let in_sector = (offset % SECTOR as u32) as usize;
            let part = (SECTOR - in_sector).min(to.len());
            let (now, rest) = to.split_at_mut(part);
            let aligned = now.as_ptr() as usize & 3 == 0;
            if let Some(whole) = now.first_chunk_mut::<SECTOR>().filter(|_| aligned) {
                // The card's DMA goes straight to the target.
                if !card::read(sector, whole) {
                    return false;
                }
            } else {
                if !card::read(sector, &mut self.block.0) {
                    return false;
                }
                now.copy_from_slice(&self.block.0[in_sector..in_sector + part]);
            }
            to = rest;
            offset += part as u32;
            if offset == cluster_bytes && !to.is_empty() {
                crate::app::feed();
                offset = 0;
                c = self.next(c);
                if !self.cluster_ok(c) {
                    return false;
                }
            }
        }
        true
    }

    /// A chain of enough clusters for `size` bytes, none of them written yet, and its first cluster (0 for an empty
    /// file). If there isn't room, nothing changes.
    pub fn reserve(&mut self, size: u32) -> Result<u32> {
        let need = size.div_ceil(self.cluster_bytes());
        // An empty file has no chain.
        if need == 0 {
            return Ok(0);
        }
        // First, without changing anything, whether there's room.
        let (mut found, mut c) = (0, self.first_free);
        let mut looked = 0;
        while looked < self.clusters && found < need {
            if looked & 0xfff == 0 {
                crate::app::feed();
            }
            if self.free(c)? {
                found += 1;
            }
            c += 1;
            if c >= self.clusters + 2 {
                c = 2;
            }
            looked += 1;
        }
        if found < need {
            return Err(CARD_FULL);
        }
        let (mut first, mut last) = (0, 0);
        c = self.first_free;
        looked = 0;
        found = 0;
        while found < need {
            if looked & 0xfff == 0 {
                crate::app::feed();
            }
            looked += 1;
            if self.free(c)? {
                if !self.set_next(c, END_OF_CHAIN) || (last != 0 && !self.set_next(last, c)) {
                    return Err(WRITE_ERROR);
                }
                if first == 0 {
                    first = c;
                }
                last = c;
                found += 1;
            }
            c += 1;
            if c >= self.clusters + 2 {
                c = 2;
            }
        }
        self.first_free = c;
        self.freed -= need as i32;
        if !self.flush_fat() {
            return Err(WRITE_ERROR);
        }
        Ok(first)
    }

    /// Writes whole sectors only, within a chain from reserve().
    pub fn write(&mut self, start: u32, mut offset: u32, mut from: &[u8]) -> bool {
        if offset % SECTOR as u32 != 0 || from.len() % SECTOR != 0 || !self.cluster_ok(start) {
            return false;
        }
        let mut c = start;
        let cluster_bytes = self.cluster_bytes();
        for _ in 0..offset / cluster_bytes {
            c = self.next(c);
            if !self.cluster_ok(c) {
                return false;
            }
        }
        offset %= cluster_bytes;
        while let Some((sector, rest)) = from.split_first_chunk::<SECTOR>() {
            if !card::write(self.cluster_sector(c) + offset / SECTOR as u32, sector) {
                return false;
            }
            from = rest;
            offset += SECTOR as u32;
            if offset == cluster_bytes && !from.is_empty() {
                crate::app::feed();
                offset = 0;
                c = self.next(c);
                if !self.cluster_ok(c) {
                    return false;
                }
            }
        }
        true
    }

    /// Frees a chain no directory names.
    pub fn release(&mut self, start: u32) -> Result<()> {
        let mut c = start;
        self.cursor_cluster = 0;
        // Bounded by the volume, in case the chain loops. It stops at the first cluster already free.
        let mut n = 0;
        while n < self.clusters && self.cluster_ok(c) {
            if n & 0xfff == 0 {
                crate::app::feed();
            }
            let following = self.next(c);
            if following == 0 {
                break;
            }
            if !self.set_next(c, 0) {
                return Err(WRITE_ERROR);
            }
            self.freed += 1;
            if c < self.first_free {
                self.first_free = c;
            }
            c = following;
            n += 1;
        }
        if self.flush_fat() { Ok(()) } else { Err(WRITE_ERROR) }
    }

    /// A cluster for a directory: all zeros, as a zero entry ends a directory.
    fn blank_cluster(&mut self, c: u32) -> Result<()> {
        self.block.0.fill(0);
        for s in 0..self.per_cluster {
            if !card::write(self.cluster_sector(c) + s, &self.block.0) {
                return Err(WRITE_ERROR);
            }
        }
        Ok(())
    }

    fn write_block(&mut self, sector: u32) -> Result<()> {
        if card::write(sector, &self.block.0) { Ok(()) } else { Err(WRITE_ERROR) }
    }

    /// Marks the old end's free entries deleted, from `at` in `sector`, so the directory goes on past them.
    fn delete_from(&mut self, sector: u32, at: usize) -> Result<()> {
        if !card::read(sector, &mut self.block.0) {
            return Err(READ_ERROR);
        }
        for a in (at..SECTOR).step_by(ENTRY) {
            self.block.0[a] = 0xe5;
        }
        self.write_block(sector)
    }

    /// A sector starting with the entries to add, and zeros after them.
    fn block_with_added(&mut self, n: usize) {
        self.block.0.fill(0);
        self.block.0[..n * ENTRY].copy_from_slice(&self.added[..n * ENTRY]);
    }

    /// Writes n entries from `added` into one directory sector, so one write makes them all appear at once. They go
    /// in a run of free entries in a sector; or, where the directory ends without room, at the start of the next
    /// sector, or of a new cluster, before the old end's free entries are marked deleted so the directory goes on to
    /// them.
    fn add_entries(&mut self, directory: u32, n: usize) -> Result<()> {
        if !self.cluster_ok(directory) {
            return Err(BAD_FOLDER);
        }
        let (mut c, mut seen, mut end_sector, mut end_at) = (directory, 0u32, 0u32, 0usize);
        loop {
            crate::app::feed();
            for s in 0..self.per_cluster {
                let sector = self.cluster_sector(c) + s;
                if end_sector != 0 {
                    // Past the end, whatever is here was never part of the directory.
                    self.block_with_added(n);
                    self.write_block(sector)?;
                    return self.delete_from(end_sector, end_at);
                }
                if !card::read(sector, &mut self.block.0) {
                    return Err(READ_ERROR);
                }
                let mut run = 0;
                for at in (0..SECTOR).step_by(ENTRY) {
                    // As walk() does, so a chain that loops ends with an error, and its FAT stays as it is.
                    seen += 1;
                    if seen > MOST_ENTRIES {
                        return Err(BAD_FOLDER);
                    }
                    let b = self.block.0[at];
                    if b != 0 && b != 0xe5 {
                        run = 0;
                        continue;
                    }
                    let first = at - run * ENTRY;
                    if b == 0 {
                        // A zero entry ends the directory: all after it is free, though what follows in this sector
                        // may never have been written.
                        if first + n * ENTRY > SECTOR {
                            end_sector = sector;
                            end_at = at;
                            break;
                        }
                        self.block.0[first..first + n * ENTRY].copy_from_slice(&self.added[..n * ENTRY]);
                        self.block.0[first + n * ENTRY..].fill(0);
                        return self.write_block(sector);
                    }
                    run += 1;
                    if run == n {
                        self.block.0[first..first + n * ENTRY].copy_from_slice(&self.added[..n * ENTRY]);
                        return self.write_block(sector);
                    }
                }
            }
            let following = self.next(c);
            if following == 0 {
                return Err(READ_ERROR);
            }
            if Self::is_end(following) {
                break;
            }
            if !self.cluster_ok(following) {
                return Err(BAD_FOLDER);
            }
            c = following;
        }
        // Full, or ending in the last sector of its last cluster: a blank cluster goes on the end first, so the
        // directory is never without its end, then the entries, then the old end's free entries are marked deleted.
        let more = self.reserve(self.cluster_bytes())?;
        self.blank_cluster(more)?;
        if !self.set_next(c, more) || !self.flush_fat() {
            return Err(WRITE_ERROR);
        }
        self.block_with_added(n);
        self.write_block(self.cluster_sector(more))?;
        if end_sector == 0 {
            return Ok(());
        }
        self.delete_from(end_sector, end_at)
    }

    /// Adds a directory entry: an 8.3 name, padded with spaces to 11 bytes, that the directory doesn't hold yet.
    pub fn add(&mut self, directory: u32, name: &[u8; 11], attributes: u8, start: u32, size: u32) -> Result<()> {
        put_entry(&mut self.added[..ENTRY], name, attributes, start, size);
        self.add_entries(directory, 1)
    }

    /// The same with a long name too, as a computer shows it ([`long_name_ok`]). The short name is still the caller's
    /// to make unique. All the entries go in one directory sector, so the file appears whole or not at all.
    /// remove() takes them all away again.
    pub fn add_named(
        &mut self,
        directory: u32,
        name: &[u8; 11],
        long_name: &[u8],
        attributes: u8,
        start: u32,
        size: u32,
    ) -> Result<()> {
        if !long_name_ok(long_name) {
            return Err(BAD_NAME);
        }
        let length = long_name.len();
        let pieces = length.div_ceil(LONG_PIECE);
        put_entry(&mut self.added[pieces * ENTRY..(pieces + 1) * ENTRY], name, attributes, start, size);
        let sum = short_sum(&self.added[pieces * ENTRY..]);
        for p in 0..pieces {
            // The last piece comes first.
            let piece = pieces - p;
            let e = &mut self.added[p * ENTRY..(p + 1) * ENTRY];
            e.fill(0);
            e[0] = piece as u8 | if p == 0 { LONG_FLAG_LAST } else { 0 };
            e[11] = LONG_ATTRIBUTES;
            e[13] = sum;
            for (i, &place) in LONG_PLACES.iter().enumerate() {
                let at = (piece - 1) * LONG_PIECE + i;
                // The name, a zero after it if there's room, then 0xFFFF.
                let unit = match at.cmp(&length) {
                    core::cmp::Ordering::Less => u16::from(long_name[at]),
                    core::cmp::Ordering::Equal => 0,
                    core::cmp::Ordering::Greater => 0xffff,
                };
                put16(e, place, unit);
            }
        }
        self.add_entries(directory, pieces + 1)
    }

    /// Whether a directory holds a file of this name, and the file if so. A folder or the volume's label of that name
    /// isn't one, here or in swap() and remove(), but add() would still clash with it.
    pub fn find(&mut self, directory: u32, name: &[u8; 11]) -> Result<Option<Entry>> {
        let (found, _) = self.walk_for(directory, &mut FindFile(name))?;
        Ok(found.map(|f| {
            let mut short = [0u8; 8];
            short.copy_from_slice(&name[..8]);
            Entry { name: short, start: f.start, size: f.size }
        }))
    }

    /// Points a directory entry at another chain, and frees the old one.
    pub fn swap(&mut self, directory: u32, name: &[u8; 11], start: u32, size: u32) -> Result<()> {
        let (found, w) = self.walk_for(directory, &mut FindFile(name))?;
        let found = found.ok_or(BAD_FOLDER)?;
        // The block still holds the entry's sector. This one write is what changes the file.
        let e = &mut self.block.0[w.at..w.at + ENTRY];
        put16(e, 20, (start >> 16) as u16);
        put16(e, 26, start as u16);
        put32(e, 28, size);
        self.write_block(w.sector)?;
        // On a damaged card whose FAT had the old chain's first cluster free, reserve() can have taken it for the new
        // one: freeing the old chain would free the new. (The old chain running into the new further on is possible
        // on such a card too; it isn't checked.)
        if found.start == start {
            return Ok(());
        }
        self.release(found.start)
    }

    /// Marks deleted the long-name pieces with this checksum that end just before `at` in the block, and returns
    /// where they started.
    fn delete_pieces_before(&mut self, mut at: usize, sum: u8) -> usize {
        let b = &mut self.block.0;
        while at >= ENTRY && b[at - ENTRY + 11] == LONG_ATTRIBUTES && b[at - ENTRY] != 0xe5 && b[at - ENTRY + 13] == sum {
            b[at - ENTRY] = 0xe5;
            at -= ENTRY;
        }
        at
    }

    pub fn remove(&mut self, directory: u32, name: &[u8; 11]) -> Result<()> {
        let (found, w) = self.walk_for(directory, &mut FindFile(name))?;
        let found = found.ok_or(BAD_FOLDER)?;
        let sum = short_sum(&self.block.0[w.at..]);
        self.block.0[w.at] = 0xe5;
        // A name given on a computer can have long-name pieces just before it. Those in this sector go with it.
        let at = self.delete_pieces_before(w.at, sum);
        self.write_block(w.sector)?;
        // And those that start in the sector before, after the entry itself has gone: a cut between leaves pieces
        // that belong to nothing, which readers skip.
        if at == 0 && w.previous != 0 && w.previous != w.sector {
            if !card::read(w.previous, &mut self.block.0) {
                return Err(READ_ERROR);
            }
            if self.delete_pieces_before(SECTOR, sum) != SECTOR {
                self.write_block(w.previous)?;
            }
        }
        self.release(found.start)
    }

    /// Finds or makes a folder.
    pub fn make_folder(&mut self, cluster: &mut u32, name: &[u8; 11]) -> Result<()> {
        let (found, _) = self.walk_for(*cluster, &mut FindFolder(name))?;
        if let Some(f) = found {
            *cluster = f.start;
            return Ok(());
        }
        let made = self.reserve(self.cluster_bytes())?;
        self.blank_cluster(made)?;
        // Its own entries, where a parent that's the root counts as cluster 0.
        let parent = if *cluster == self.root { 0 } else { *cluster };
        self.block.0.fill(0);
        put_entry(&mut self.block.0[..ENTRY], b".          ", ATTR_FOLDER, made, 0);
        put_entry(&mut self.block.0[ENTRY..2 * ENTRY], b"..         ", ATTR_FOLDER, parent, 0);
        self.write_block(self.cluster_sector(made))?;
        self.add(*cluster, name, ATTR_FOLDER, made, 0)?;
        *cluster = made;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<()> {
        if !self.flush_fat() {
            return Err(WRITE_ERROR);
        }
        if self.info_sector == 0 {
            return Ok(());
        }
        if !card::read(self.info_sector, &mut self.block.0) {
            return Err(READ_ERROR);
        }
        let b = &mut self.block.0;
        if le32(b, 0) != FSINFO_SIGNATURE || le32(b, 484) != FSINFO_SIGNATURE2 {
            return Ok(());
        }
        let count = le32(b, 488);
        if count != NO_COUNT {
            put32(b, 488, count.wrapping_add(self.freed as u32));
        }
        put32(b, 492, self.first_free);
        self.freed = 0;
        self.write_block(self.info_sector)
    }
}

impl Default for Volume {
    fn default() -> Self {
        Self::new()
    }
}

/// e into the list, in name order, keeping the first by name.
fn insert(list: &mut List<'_, Entry>, e: &[u8]) {
    if let Some(at) = list.place(|x| x.name[..] < e[..8]) {
        let x = &mut list.entries[at];
        x.name.copy_from_slice(&e[..8]);
        x.start = entry_start(e);
        x.size = le32(e, 28);
    }
}

fn put_entry(e: &mut [u8], name: &[u8; 11], attributes: u8, start: u32, size: u32) {
    e.fill(0);
    e[..11].copy_from_slice(name);
    e[11] = attributes;
    put16(e, 16, DOS_DATE);
    put16(e, 18, DOS_DATE);
    put16(e, 24, DOS_DATE);
    put16(e, 20, (start >> 16) as u16);
    put16(e, 26, start as u16);
    put32(e, 28, size);
}

/// Whether add_named() takes a long name, to ask before anything is written: printable ASCII, not \ / : * ? " < > |,
/// not ending in a space or a dot, at most LONG_PIECES*13 characters.
pub fn long_name_ok(long_name: &[u8]) -> bool {
    let Some(&last) = long_name.last() else { return false };
    long_name.len() <= LONG_PIECES * LONG_PIECE
        && long_name.iter().all(|&c| (0x20..=0x7e).contains(&c) && !b"\\/:*?\"<>|".contains(&c))
        && last != b' '
        && last != b'.'
}
