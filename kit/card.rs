// SPDX-License-Identifier: MIT
//! The modded unit's SD card on the panel's SPI1, in SPI mode with CRCs on both ways. Between [`open`] and [`close`]
//! the screen can't be drawn.
//!
//! The card shares the panel's SPI1, with DO on the panel's D/C and CS the inverse of the panel's CS, so the card is
//! selected while that's high (m-bootloader's docs/SPEC.md §11.5 for the FM-1).
//! FM-1: CLK PC9, DI PC10, DO PC8, CS = NOT PC7. SMK-37: CLK PB6, DI PB7, DO PB5, CS = NOT PC8.
use crate::hw::{Reg, bit, dma_address, sync_cpu};
use crate::platform::{gpio_input, gpio_level, gpio_output, hal_delay_us};
use crate::{Aligned, Global, app, log, log::hex};

const SPI1: Reg = Reg::at::<0x11d00>();
const SPI1_BAUD: Reg = SPI1.add(4);
const SPI1_BUF: Reg = SPI1.add(8);
const SPI1_ADR: Reg = SPI1.add(0x0c);
const SPI1_CNT: Reg = SPI1.add(0x10);
const CS_PORT: u32 = 2;
const RETRIES: u32 = 3;
/// Single-block commands, and the wait for a block's start or for a write to finish.
const TOKEN_WAIT: u32 = 200_000;
const BUSY_WAIT: u32 = 2_000_000;
/// The log only holds so much.
const FAILURES_LOGGED: u32 = 12;
/// A card that has stopped answering can take two restarts: seen on a 32 GB SDHC card, below.
const START_ATTEMPTS: u32 = 3;
/// Wiring decides what works: an FM-1 prototype with long wires managed 8 MHz but not 12, and an SMK-37 16 but not
/// 24. CRCs catch a clock that's too fast, so startup takes the fastest that reads cleanly, and later errors step it
/// down. There's no step at 16, as it didn't make usbdisk faster, and it would run closer to where a card fails.
const CLOCKS: [u32; 5] = [24, 12, 8, 6, 4];

/// The SMK-37's card mod selects with PC8 (through an inverter) and answers on PB5; the FM-1's on PC7 and PC8.
fn smk() -> bool {
    app::board() == 2
}

fn cs_pin() -> u32 {
    if smk() { 8 } else { 7 }
}

fn do_pin() -> (u32, u32) {
    if smk() { (1, 5) } else { (2, 8) }
}

struct Card {
    crc_table: [u16; 256],
    block_addressed: bool,
    failures_logged: u32,
    clock_mhz: u32,
    clock_step: usize,
    clock_settled: bool,
    stream_at: u32,
    write_crc: u16,
    write_at: u32,
}

static CARD: Global<Card> = Global::new(Card {
    crc_table: [0; 256],
    block_addressed: false,
    failures_logged: 0,
    clock_mhz: 0,
    clock_step: 0,
    clock_settled: false,
    stream_at: 0,
    write_crc: 0,
    write_at: 0,
});
static PROBE: Global<Aligned<512>> = Global::new(Aligned([0; 512]));

fn card() -> &'static mut Card {
    // Safety: each function here takes it once, and none of them calls another that does while it holds it (they
    // take it again after).
    unsafe { CARD.get() }
}

fn x(b: u8) -> u8 {
    SPI1.set(bit(14));
    SPI1_BUF.write(u32::from(b));
    sync_cpu();
    let mut n = 0;
    while n < 400_000 && SPI1.read() & bit(15) == 0 {
        n += 1;
    }
    SPI1_BUF.read() as u8
}

fn select_card(on: bool) {
    gpio_level(CS_PORT, cs_pin(), on);
}

/// A card needs 8 more clocks to finish a transaction, and they may not come soon from the next command.
fn end_transaction() {
    select_card(false);
    x(0xff);
}

fn crc7(p: &[u8]) -> u8 {
    let mut c = 0u8;
    for &byte in p {
        let mut d = byte;
        for _ in 0..8 {
            c <<= 1;
            if (d ^ c) & 0x80 != 0 {
                c ^= 0x09;
            }
            d <<= 1;
        }
    }
    c & 0x7f
}

fn crc16(p: &[u8]) -> u16 {
    let table = &card().crc_table;
    p.iter().fold(0u16, |c, &b| (c << 8) ^ table[usize::from((c >> 8) as u8 ^ b)])
}

fn send_frame(frame: &[u8; 5]) {
    for &b in frame {
        x(b);
    }
    x(crc7(frame) << 1 | 1);
}

fn answer() -> u8 {
    let mut r = 0xff;
    for _ in 0..16 {
        r = x(0xff);
        if r & 0x80 == 0 {
            break;
        }
    }
    r
}

fn command(c: u8, argument: u32) -> u8 {
    let a = argument.to_be_bytes();
    select_card(false);
    x(0xff);
    select_card(true);
    x(0xff);
    send_frame(&[0x40 | c, a[0], a[1], a[2], a[3]]);
    answer()
}

/// Where a sector is, as the card counts.
fn address(sector: u32) -> u32 {
    if card().block_addressed { sector } else { sector << 9 }
}

pub fn open(lsb_mhz: u32) {
    // A panel NOP (D/C low, CS low), so it ignores what follows; then D/C is the card's DO, at about 400 kHz.
    let (port, pin) = do_pin();
    gpio_level(port, pin, false);
    select_card(false);
    SPI1.write(0x4021);
    x(0);
    gpio_input(port, pin, true, false);
    SPI1.write(0x4029);
    SPI1_BAUD.write(lsb_mhz * 5 / 2 - 1);
}

pub fn close() {
    end_transaction();
    SPI1.write(0x4021);
    SPI1_BAUD.write(4);
    let (port, pin) = do_pin();
    gpio_output(port, pin, true);
}

/// SPI1's clock is LSB/(baud+1). This divides LSB by enough to stay at or below a step, so 24 gives 20 MHz from LSB
/// 60.
fn divider(lsb_mhz: u32) -> u32 {
    let mhz = CLOCKS[card().clock_step];
    lsb_mhz.div_ceil(mhz)
}

pub fn fast(lsb_mhz: u32) {
    SPI1_BAUD.write(divider(lsb_mhz) - 1);
}

/// In kHz, as 60/8 is 7.5 MHz.
fn real_khz() -> u32 {
    let mhz = card().clock_mhz;
    mhz * 1000 / divider(mhz)
}

fn slow_down() {
    let step = card().clock_step;
    if step + 1 < CLOCKS.len() {
        card().clock_step = step + 1;
        fast(card().clock_mhz);
        log!("card: too many errors, now ", real_khz(), " kHz");
    }
}

fn log_failure(what: &str, sector: u32, detail: u32, more: u32) {
    let c = card();
    if c.failures_logged < FAILURES_LOGGED {
        c.failures_logged += 1;
        log!("card ", what, " failed: sector ", hex(sector), ", ", hex(detail), " ", hex(more));
    }
}

/// Starts the DMA into `p`, keeping the card's DI high between bytes, as it was left by the last x(0xff).
///
/// # Safety
/// `p` stays the DMA's, with nothing reading or writing it, until [`receive_finish`].
unsafe fn receive_start(p: *mut u8, n: u32) {
    SPI1.set(bit(14) | bit(12));
    SPI1_ADR.write(dma_address(p));
    sync_cpu();
    SPI1_CNT.write(n);
}

fn receive_finish() {
    let mut i = 0;
    while i < 400_000 && SPI1.read() & bit(15) == 0 {
        i += 1;
    }
    SPI1.clear(bit(12));
    SPI1.set(bit(14));
    #[cfg(target_arch = "pi32v2")]
    // Safety: an instruction that waits for the DMA's writes; it's a fence for the compiler too.
    unsafe {
        core::arch::asm!("ssync", options(nostack, preserves_flags))
    };
}

fn block_token(sector: u32) -> bool {
    let mut r = 0xff;
    for _ in 0..TOKEN_WAIT {
        r = x(0xff);
        if r != 0xff {
            break;
        }
    }
    if r != 0xfe {
        log_failure("token", sector, u32::from(r), 0);
    }
    r == 0xfe
}

/// After the data: its CRC, then the check.
fn block_check(p: &[u8], sector: u32) -> bool {
    let got = u16::from(x(0xff)) << 8 | u16::from(x(0xff));
    let want = crc16(p);
    if got != want {
        log_failure("crc", sector, u32::from(got), u32::from(want));
        if card().clock_settled {
            slow_down();
        }
    }
    got == want
}

/// A block after the command's answer, with its CRC checked.
fn read_block(p: &mut [u8], sector: u32) -> bool {
    if !block_token(sector) {
        return false;
    }
    // Safety: `p` is borrowed, and nothing else touches it, until the DMA has finished.
    unsafe { receive_start(p.as_mut_ptr(), p.len() as u32) };
    receive_finish();
    block_check(p, sector)
}

fn read_size() -> u32 {
    let mut csd = Aligned([0u8; 16]);
    let answer = command(9, 0);
    if answer != 0 {
        log_failure("size", 0, u32::from(answer), 0);
    }
    let ok = answer == 0 && read_block(&mut csd.0, 0);
    end_transaction();
    if !ok {
        return 0;
    }
    let csd = csd.0.map(u32::from);
    if csd[0] >> 6 == 1 {
        return (((csd[7] & 0x3f) << 16 | csd[8] << 8 | csd[9]) + 1) * 1024;
    }
    let size = ((csd[6] & 3) << 10 | csd[7] << 2 | csd[8] >> 6) + 1;
    let multiply = ((csd[9] & 3) << 1 | csd[10] >> 7) + 2;
    let block = csd[5] & 15;
    size << multiply << block >> 9
}

fn start_once() -> bool {
    select_card(false);
    SPI1_BAUD.write(card().clock_mhz * 5 / 2 - 1);
    for _ in 0..10 {
        x(0xff);
    }
    let mut r = 0xff;
    for _ in 0..20 {
        r = command(0, 0);
        if r == 1 {
            break;
        }
    }
    if r != 1 {
        log!("card: reset answered ", hex(u32::from(r)));
        return false;
    }
    // One lost answer here would treat an SDHC card as an old one, and it would never leave idle.
    let mut v2 = false;
    for _ in 0..5 {
        let answer = command(8, 0x1aa);
        let mut echo = 0;
        for _ in 0..4 {
            echo = x(0xff);
        }
        v2 = answer == 1 && echo == 0xaa;
        // An old card, for which this command is illegal.
        if v2 || answer == 5 {
            break;
        }
    }
    for _ in 0..1000 {
        if command(55, 0) <= 1 {
            r = command(41, if v2 { bit(30) } else { 0 });
            if r == 0 {
                break;
            }
        }
        hal_delay_us(1000);
    }
    if r != 0 {
        log!("card: start answered ", hex(u32::from(r)), ", ", if v2 { "v2" } else { "v1" });
        return false;
    }
    card().block_addressed = false;
    if v2 && command(58, 0) == 0 {
        card().block_addressed = x(0xff) & 0x40 != 0;
        for _ in 0..3 {
            x(0xff);
        }
    }
    let ok = command(59, 1) == 0 && (card().block_addressed || command(16, 512) == 0);
    end_transaction();
    if !ok {
        log!("card: CRC or block size refused");
        return false;
    }
    fast(card().clock_mhz);
    true
}

fn start() -> bool {
    (0..START_ATTEMPTS).any(|_| start_once())
}

fn probe_read(sector: u32) -> bool {
    // Safety: only these probes use it, one at a time.
    let block = unsafe { &mut PROBE.get().0 };
    let ok = command(17, address(sector)) == 0 && read_block(block, sector);
    end_transaction();
    ok
}

fn probe() -> &'static [u8; 512] {
    unsafe { &PROBE.get().0 }
}

/// Whether this clock reads cleanly. Blank sectors prove nothing, as all zeros pass a CRC however they're misread, and
/// most of a card's first sectors are blank. So this reads what's likely to hold data: the partition table, the first
/// partition's boot sector, and the start of its FAT. A card with no filesystem must still pass, so it can be
/// formatted, and these reads are then just more checked data.
fn probe_reads(size: u32) -> bool {
    if !probe_read(0) {
        return false;
    }
    let b = probe();
    let mut partition = if b[510] == 0x55 && b[511] == 0xaa && b[450] != 0 {
        u32::from_le_bytes([b[454], b[455], b[456], b[457]])
    } else {
        0
    };
    // Without a partition table, those bytes mean nothing, and sector 0 may be the boot sector itself.
    if partition >= size {
        partition = 0;
    }
    if !probe_read(partition) {
        return false;
    }
    let b = probe();
    let reserved = u32::from(u16::from_le_bytes([b[14], b[15]]));
    let mut with_data = 0u32;
    let mut i = 0;
    while i < 16 && partition.wrapping_add(reserved).wrapping_add(i) < size {
        if !probe_read(partition + reserved + i) {
            return false;
        }
        if probe().iter().any(|&b| b != 0 && b != 0xff) {
            with_data += 1;
        }
        i += 1;
    }
    log!("card: ", with_data, " of 16 test sectors held data");
    true
}

/// Starts the card, and returns its size in 512-byte sectors, or 0 if there's no usable card.
pub fn start_card(lsb_mhz: u32) -> u32 {
    let c = card();
    c.clock_mhz = lsb_mhz;
    for (i, t) in c.crc_table.iter_mut().enumerate() {
        let mut v = (i as u16) << 8;
        for _ in 0..8 {
            v = if v & 0x8000 != 0 { (v << 1) ^ 0x1021 } else { v << 1 };
        }
        *t = v;
    }
    c.clock_settled = false;
    c.clock_step = 0;
    while card().clock_step < CLOCKS.len() {
        log!("card: trying ", real_khz(), " kHz");
        if start() {
            let size = read_size();
            if size != 0 && probe_reads(size) {
                card().clock_settled = true;
                log!("card: ", real_khz(), " kHz");
                return size;
            }
        }
        card().clock_step += 1;
    }
    card().clock_step = CLOCKS.len() - 1;
    0
}

/// Leaves the card as a bootloader expects to find it.
pub fn finish() {
    // The restart clears whatever the card was left holding, as the bootloader starts it only once. Its commands
    // have no CRCs, so that goes too.
    if start() {
        command(59, 0);
        end_transaction();
    }
}

/// A rejected read of the card's last sector, after reading up to it with gaps of a few ms, leaves this card answering
/// "illegal command" to everything until it's restarted. It looks like read-ahead running off the end of the card. A
/// restart costs little, so any failure gets one, and the host never sees it.
pub fn read(sector: u32, data: &mut [u8; 512]) -> bool {
    for _ in 0..RETRIES {
        let answer = command(17, address(sector));
        if answer != 0 {
            log_failure("read", sector, u32::from(answer), 0);
        }
        let ok = answer == 0 && read_block(data, sector);
        end_transaction();
        if ok {
            return true;
        }
        if answer != 0 && !start() {
            return false;
        }
    }
    false
}

/// Many sectors in a row, which is much faster than one at a time. The card stays selected until the stop.
pub fn stream_start(sector: u32) -> bool {
    let answer = command(18, address(sector));
    if answer != 0 {
        log_failure("stream", sector, u32::from(answer), 0);
        end_transaction();
        if answer & 0x7e != 0 {
            start();
        }
        return false;
    }
    card().stream_at = sector;
    true
}

/// A sector of a stream being read into its buffer by the DMA, which other work can run alongside. Nothing else may
/// use the card until it's finished. Dropped, it finishes too, so the buffer is never given back while the DMA writes
/// it.
#[must_use]
pub struct Reading<'a> {
    data: &'a mut [u8; 512],
    done: bool,
}

/// The next sector of the stream, in two halves: this starts it.
///
/// # Safety
/// The Reading is finished or dropped, never leaked (`mem::forget`): that would end the buffer's borrow with the DMA
/// still writing it.
pub unsafe fn stream_begin(data: &mut [u8; 512]) -> Option<Reading<'_>> {
    if !block_token(card().stream_at) {
        return None;
    }
    // Safety: the buffer is the Reading's until it finishes or drops, which waits for the DMA, and the caller doesn't
    // leak it.
    unsafe { receive_start(data.as_mut_ptr(), 512) };
    Some(Reading { data, done: false })
}

impl Reading<'_> {
    fn complete(&mut self) -> bool {
        if self.done {
            return true;
        }
        self.done = true;
        receive_finish();
        let c = card();
        let at = c.stream_at;
        c.stream_at += 1;
        block_check(self.data, at)
    }

    pub fn finish(mut self) -> bool {
        self.complete()
    }
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        self.complete();
    }
}

/// One sector of the stream, whole.
pub fn stream_read(data: &mut [u8; 512]) -> bool {
    // Safety: finished here.
    unsafe { stream_begin(data) }.is_some_and(Reading::finish)
}

pub fn stream_stop() {
    // CMD12's answer comes after a stuffing byte, then the card may be busy for a while. The card stays selected.
    send_frame(&[0x40 | 12, 0, 0, 0, 0]);
    x(0xff);
    let r = answer();
    let ready = (0..BUSY_WAIT).any(|_| x(0xff) == 0xff);
    end_transaction();
    if r & 0x7e != 0 || !ready {
        log_failure("stream stop", card().stream_at, u32::from(r), u32::from(ready));
        start();
    }
}

fn wait_ready() -> bool {
    (0..BUSY_WAIT).any(|_| x(0xff) == 0xff)
}

/// The card can report a failure only after a write, so this asks.
fn write_status(what: &str, sector: u32) -> bool {
    let first = command(13, 0);
    let second = x(0xff);
    let ok = first == 0 && second == 0;
    if !ok {
        log_failure(what, sector, u32::from(first), u32::from(second));
    }
    ok
}

pub fn write(sector: u32, data: &[u8; 512]) -> bool {
    let crc = crc16(data);
    for _ in 0..RETRIES {
        let mut ok = false;
        let answer = command(24, address(sector));
        if answer == 0 {
            x(0xff);
            x(0xfe);
            for &b in data {
                x(b);
            }
            x((crc >> 8) as u8);
            x(crc as u8);
            let accepted = x(0xff) & 0x1f;
            ok = accepted == 0x05 && wait_ready();
            if !ok {
                log_failure("write", sector, u32::from(accepted), 0);
                slow_down();
            }
        } else {
            log_failure("write command", sector, u32::from(answer), 0);
        }
        if ok {
            ok = write_status("write status", sector);
        }
        end_transaction();
        if ok {
            return true;
        }
        if !start() {
            return false;
        }
    }
    false
}

/// The same for writing, with the card selected throughout. A sector is only safe once the stop has returned true.
pub fn write_stream_start(sector: u32) -> bool {
    let answer = command(25, address(sector));
    if answer != 0 {
        log_failure("write stream", sector, u32::from(answer), 0);
        end_transaction();
        start();
        return false;
    }
    x(0xff);
    card().write_at = sector;
    true
}

/// A sector of a write stream going out by DMA from its buffer, while other work runs. Dropped, it finishes too, so
/// the buffer is never changed while the DMA reads it.
#[must_use]
pub struct Writing<'a> {
    data: core::marker::PhantomData<&'a [u8; 512]>,
    done: bool,
}

/// # Safety
/// The Writing is finished or dropped, never leaked (`mem::forget`): that would end the buffer's borrow with the DMA
/// still reading it.
pub unsafe fn write_stream_begin(data: &[u8; 512]) -> Writing<'_> {
    card().write_crc = crc16(data);
    x(0xfc);
    SPI1.set(bit(14));
    SPI1_ADR.write(dma_address(data.as_ptr().cast_mut()));
    sync_cpu();
    SPI1_CNT.write(512);
    Writing { data: core::marker::PhantomData, done: false }
}

impl Writing<'_> {
    pub fn finish(mut self) -> bool {
        self.complete()
    }

    fn complete(&mut self) -> bool {
        if self.done {
            return true;
        }
        self.done = true;
        let mut i = 0;
        while i < 400_000 && SPI1.read() & bit(15) == 0 {
            i += 1;
        }
        SPI1.set(bit(14));
        let crc = card().write_crc;
        x((crc >> 8) as u8);
        x(crc as u8);
        let accepted = x(0xff) & 0x1f;
        let ok = accepted == 0x05 && wait_ready();
        let at = card().write_at;
        if !ok {
            log_failure("write stream data", at, u32::from(accepted), 0);
            slow_down();
        }
        card().write_at = at + 1;
        ok
    }
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        self.complete();
    }
}

pub fn write_stream_stop() -> bool {
    x(0xfd);
    x(0xff);
    let at = card().write_at;
    let ok = if wait_ready() {
        write_status("write stream status", at)
    } else {
        log_failure("write stream stop", at, 0, 0);
        false
    };
    end_transaction();
    if !ok {
        start();
    }
    ok
}
