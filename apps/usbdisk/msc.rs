// SPDX-License-Identifier: MIT
//! A USB mass storage device on USB0, bulk-only, polled, serving the card one sector at a time. A write has reached the
//! card before the host is told it succeeded.
//!
//! Bulk-only mass storage on USB0's MUSB controller. The controller's register sequences are those of m-bootloader's
//! usb_midi (rust/src/usb_midi.rs), whose comments say what they're from. Its accesses have to be kept apart, or it
//! babbles on the bus.
use kit::hw::{Reg, bit, dma_address, sync_cpu};
use kit::platform::{hal_delay_us, settle};
use kit::{Global, app, card};

const CON0: Reg = Reg::at::<0x11800>();
const CON1: Reg = Reg::at::<0x11804>();
const EP0_COUNT: Reg = Reg::at::<0x11808>();
const EP1_COUNT: Reg = EP0_COUNT.add(4);
const EP0_ADDRESS: Reg = Reg::at::<0x11818>();
const EP1_TX_ADDRESS: Reg = Reg::at::<0x1181c>();
/// Endpoints 1 to 3: eight bytes apart.
const EP1_RX_ADDRESS: Reg = Reg::at::<0x11820>();
const EP4_RX_ADDRESS: Reg = Reg::at::<0x1183c>();
const IO_CON0: Reg = Reg::at::<0x51000>();
const CLOCK_CON1: Reg = Reg::at::<0x10010>();
// The MUSB's registers.
const FADDR: u32 = 0;
const POWER: u32 = 1;
const INT_TX1: u32 = 2;
const INT_USB: u32 = 6;
const INT_TX1_ENABLE: u32 = 7;
const INT_TX2_ENABLE: u32 = 8;
const INT_RX1_ENABLE: u32 = 9;
const INT_RX2_ENABLE: u32 = 10;
const INT_USB_ENABLE: u32 = 11;
const INDEX: u32 = 14;
const TX_MAX: u32 = 16;
const CSR0: u32 = 17;
const TX_CSR1: u32 = 17;
const TX_CSR2: u32 = 18;
const RX_MAX: u32 = 19;
const RX_CSR1: u32 = 20;
const RX_CSR2: u32 = 21;
const RX_COUNT1: u32 = 22;
const RX_COUNT2: u32 = 23;
const WAIT_LIMIT: u32 = 20000;
const PACKET: u32 = 64;
const EP_IN: u32 = 0x81;
const EP_OUT: u32 = 0x01;
const SECTOR: u32 = 512;
/// One read at a time near the end of the card: reading ahead up to its end upsets some cards (kit/card.rs).
const NEAR_THE_END: u32 = 256;

static DEVICE_DESCRIPTOR: [u8; 18] = [18, 1, 0x10, 0x01, 0, 0, 0, PACKET as u8, 0x09, 0x12, 0x03, 0x00, 0x00, 0x01, 0, 1, 2, 1];
#[rustfmt::skip]
static CONFIG_DESCRIPTOR: [u8; 32] = [
    9, 2, 32, 0, 1, 1, 0, 0x80, 50,
    9, 4, 0, 0, 2, 8, 6, 0x50, 0,                  // mass storage, SCSI, bulk-only
    7, 5, EP_IN as u8, 2, PACKET as u8, 0, 0,
    7, 5, EP_OUT as u8, 2, PACKET as u8, 0, 0,
];
static LANGUAGE_STRING: [u8; 4] = [4, 3, 0x09, 0x04];

/// A name as a string descriptor: UTF-16.
const fn string<const N: usize>(text: &[u8]) -> [u8; N] {
    assert!(N == 2 + 2 * text.len());
    let mut d = [0; N];
    d[0] = N as u8;
    d[1] = 3;
    let mut i = 0;
    while i < text.len() {
        d[2 + 2 * i] = text[i];
        i += 1;
    }
    d
}

// The names a computer shows, by the board it's on.
const FM1_NAME: &[u8] = b"FM-1 USB DISK";
const SMK37_NAME: &[u8] = b"SMK-37 USB DISK";
static FM1_STRING: [u8; 2 + 2 * FM1_NAME.len()] = string(FM1_NAME);
static SMK37_STRING: [u8; 2 + 2 * SMK37_NAME.len()] = string(SMK37_NAME);
const SERIAL: &[u8] = b"000000000001";
static SERIAL_STRING: [u8; 2 + 2 * SERIAL.len()] = string(SERIAL);

/// The answer to an inquiry, with the product's 16 bytes, padded with spaces.
const fn inquiry(product: &[u8]) -> Buffer<36> {
    const HEAD: [u8; 16] = [0, 0x80, 0x02, 0x02, 31, 0, 0, 0, b'M', b'-', b'A', b'P', b'P', b'S', b' ', b' '];
    let mut d = [b' '; 36];
    let mut i = 0;
    while i < 16 {
        d[i] = HEAD[i];
        if i < product.len() {
            d[16 + i] = product[i];
        }
        i += 1;
    }
    d[32] = b'1';
    d[33] = b'.';
    d[34] = b'0';
    Buffer(d)
}
// The controller's DMA reads them, so they're aligned as the buffers are.
static FM1_INQUIRY: Buffer<36> = inquiry(FM1_NAME);
static SMK37_INQUIRY: Buffer<36> = inquiry(SMK37_NAME);
static ZERO: [u8; 2] = [0; 2];
static HALTED: [u8; 2] = [1, 0];
/// The configuration, as a byte to send.
static CONFIGURATIONS: [u8; 2] = [0, 1];

fn smk() -> bool {
    app::board() == 2
}

/// What the controller's DMA reads and writes: aligned for it.
#[repr(align(4))]
pub struct Buffer<const N: usize>(pub [u8; N]);

impl<const N: usize> Buffer<N> {
    /// A byte the controller wrote: read where it is, each time.
    fn received(&self, i: usize) -> u8 {
        // Safety: a byte of this buffer, read through a reference to it.
        unsafe { (&raw const self.0[i]).read_volatile() }
    }

    /// The first `M` bytes the controller wrote, read where they are.
    fn received_copy<const M: usize>(&self) -> [u8; M] {
        let mut out = [0; M];
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.received(i);
        }
        out
    }

    fn address(&mut self) -> u32 {
        dma_address(self.0.as_mut_ptr())
    }
}

/// A static's address for the controller's DMA, which only reads it.
fn read_address(data: &'static [u8]) -> u32 {
    dma_address(data.as_ptr().cast_mut())
}

#[derive(Clone, Copy)]
pub struct Stats {
    pub sectors_read: u32,
    pub sectors_written: u32,
    pub errors: u32,
}

/// The controller's side, and one command's.
struct Io {
    // The controller's DMA writes past a full packet when receiving, so these have room to spare.
    ep0: Buffer<{ PACKET as usize + 4 }>,
    command_block: Buffer<{ PACKET as usize + 4 }>,
    reply: Buffer<32>,
    status_block: Buffer<16>,
    /// What is left of the answer being sent on endpoint 0. (None, not an empty slice, which isn't all zeros: one
    /// byte that isn't zero would put all of this in the image.)
    ep0_data: Option<&'static [u8]>,
    total_sectors: u32,
    pending_address: u8,
    configuration: u8,
    sense_key: u8,
    sense_code: u8,
    address_pending: bool,
    ep0_sending: bool,
    zero_end: bool,
    stalled_in: bool,
    stalled_out: bool,
    ejected: bool,
    aborted: bool,
    needs_reset: bool,
    stats: Stats,
    /// The controller's selected endpoint, so that the data path doesn't reselect it for every packet. 0xff for none,
    /// set by start() so that all of this stays zeros.
    selected: u32,
    tx_status: u32,
    // One command's state.
    tag: u32,
    expected: u32,
    moved: u32,
    to_host: bool,
}

type Sector = Buffer<{ SECTOR as usize + 4 }>;

/// The device: the controller's side, and the two sector buffers that the card and USB take turns with.
pub struct Msc {
    io: Io,
    sectors: [Sector; 2],
}

static MSC: Global<Msc> = Global::new(Msc {
    io: Io {
        ep0: Buffer([0; PACKET as usize + 4]),
        command_block: Buffer([0; PACKET as usize + 4]),
        reply: Buffer([0; 32]),
        status_block: Buffer([0; 16]),
        ep0_data: None,
        total_sectors: 0,
        pending_address: 0,
        configuration: 0,
        sense_key: 0,
        sense_code: 0,
        address_pending: false,
        ep0_sending: false,
        zero_end: false,
        stalled_in: false,
        stalled_out: false,
        ejected: false,
        aborted: false,
        needs_reset: false,
        stats: Stats { sectors_read: 0, sectors_written: 0, errors: 0 },
        selected: 0,
        tx_status: 0,
        tag: 0,
        expected: 0,
        moved: 0,
        to_host: false,
    },
    sectors: [Buffer([0; SECTOR as usize + 4]), Buffer([0; SECTOR as usize + 4])],
});

/// The device, which the app takes once.
///
/// # Safety
/// Called once: this is the only reference to it.
pub unsafe fn claim() -> &'static mut Msc {
    unsafe { MSC.get() }
}

/// Waits until what the controller sent to memory can be read.
#[inline(always)]
fn memory_sync() {
    #[cfg(target_arch = "pi32v2")]
    // Safety: an instruction that waits for the DMA's writes; it's a fence for the compiler too.
    unsafe {
        core::arch::asm!("ssync", options(nostack, preserves_flags))
    }
}

/// Accesses a few cycles apart make the controller babble on the bus, and the host then turns the port off. So they're
/// kept apart, out of line.
#[inline(never)]
fn sie_start(command: u32) {
    settle(40);
    CON1.write(command);
    sync_cpu();
}

fn sie_write(r: u32, v: u32) {
    if CON0.read() & 4 == 0 {
        return;
    }
    sie_start(r << 8 & 0xff_ff00 | v & 0xff);
    let mut n = WAIT_LIMIT;
    while n != 0 && CON1.read() & 0x8000 == 0 {
        n -= 1;
    }
}

fn sie_read(r: u32) -> u32 {
    if CON0.read() & 4 == 0 {
        return 0;
    }
    sie_start(r << 8 & 0xff_bf00 | 0x4000);
    let mut n = WAIT_LIMIT;
    while n != 0 {
        if CON1.read() & 0x8000 != 0 {
            return CON1.read() & 0xff;
        }
        n -= 1;
    }
    0
}

fn descriptor(value: u32) -> Option<&'static [u8]> {
    match (value >> 8, value & 255) {
        (1, _) => Some(&DEVICE_DESCRIPTOR),
        (2, _) => Some(&CONFIG_DESCRIPTOR),
        (3, 0) => Some(&LANGUAGE_STRING),
        (3, 1) => Some(if smk() { &SMK37_STRING } else { &FM1_STRING }),
        (3, 2) => Some(&SERIAL_STRING),
        _ => None,
    }
}

/// The first 512 bytes of a sector buffer: the rest is room for the controller's DMA.
fn whole(buffer: &mut Sector) -> &mut [u8; 512] {
    buffer.0.first_chunk_mut().unwrap()
}

fn address(sector: &mut [u8; 512]) -> u32 {
    dma_address(sector.as_mut_ptr())
}

impl Io {
    fn select_endpoint(&mut self, n: u32) {
        if self.selected != n {
            self.selected = n;
            sie_write(INDEX, n);
        }
    }

    fn ep0_chunk(&mut self) {
        let data = self.ep0_data.unwrap_or_default();
        let n = data.len().min(PACKET as usize);
        self.ep0.0[..n].copy_from_slice(&data[..n]);
        sync_cpu();
        EP0_ADDRESS.write(self.ep0.address());
        EP0_COUNT.write(n as u32);
        let left = &data[n..];
        self.ep0_data = Some(left);
        let last = left.is_empty() && !(n == PACKET as usize && self.zero_end);
        if left.is_empty() && n == PACKET as usize {
            self.zero_end = false;
        }
        self.select_endpoint(0);
        sie_write(CSR0, if last { 0x0a } else { 0x02 });
        self.ep0_sending = !last;
    }

    fn ep0_send(&mut self, data: &'static [u8], asked: u32) {
        let size = data.len().min(asked as usize);
        self.ep0_data = Some(&data[..size]);
        self.zero_end = (size as u32) < asked && size as u32 % PACKET == 0;
        self.select_endpoint(0);
        sie_write(CSR0, 0x40);
        self.ep0_chunk();
    }

    /// Ready for the next packet, which the controller's DMA will put at `to`.
    fn rx_arm(&mut self, to: u32) {
        EP1_RX_ADDRESS.write(to);
        self.select_endpoint(1);
        let mut csr = sie_read(RX_CSR1) | sie_read(RX_CSR2) << 8;
        csr = (csr & !0x164) | 0x10;
        sie_write(RX_CSR1, csr & 0xff);
        sie_write(RX_CSR2, csr >> 8);
    }

    fn rx_arm_command(&mut self) {
        let to = self.command_block.address();
        self.rx_arm(to);
    }

    fn endpoints_reset(&mut self) {
        EP1_TX_ADDRESS.write(self.reply.address());
        EP1_RX_ADDRESS.write(self.command_block.address());
        self.select_endpoint(1);
        sie_write(TX_MAX, 0xff);
        sie_write(TX_CSR1, 0x48);
        sie_write(TX_CSR2, 0);
        sie_write(RX_MAX, 0xff);
        sie_write(RX_CSR1, 0x90);
        sie_write(RX_CSR2, 0);
        self.stalled_in = false;
        self.stalled_out = false;
    }

    fn ep1_configure(&mut self) {
        self.endpoints_reset();
        sie_write(INT_RX1_ENABLE, 0x02);
        CON0.clear(bit(20));
    }

    fn stall_in(&mut self) {
        self.select_endpoint(1);
        sie_write(TX_CSR1, 0x10);
        self.stalled_in = true;
    }

    fn stall_out(&mut self) {
        self.select_endpoint(1);
        sie_write(RX_CSR1, 0x20);
        self.stalled_out = true;
    }

    fn clear_halt(&mut self, endpoint: u32) {
        self.select_endpoint(1);
        if endpoint == EP_IN {
            sie_write(TX_CSR1, 0x48);
            self.stalled_in = false;
        } else if endpoint == EP_OUT {
            sie_write(RX_CSR1, 0x90);
            self.stalled_out = false;
        }
    }

    fn ep0_service(&mut self) {
        if self.address_pending {
            sie_write(FADDR, u32::from(self.pending_address));
            self.address_pending = false;
        }
        self.select_endpoint(0);
        let csr = sie_read(CSR0);
        // We sent a stall.
        if csr & 0x04 != 0 {
            sie_write(CSR0, 0);
            self.ep0_sending = false;
            return;
        }
        // The host abandoned the transfer.
        if csr & 0x10 != 0 {
            sie_write(CSR0, 0x80);
            self.ep0_sending = false;
        }
        if self.ep0_sending {
            if csr & 0x02 == 0 {
                self.ep0_chunk();
            }
            return;
        }
        if csr & 0x01 == 0 {
            return;
        }
        memory_sync();
        let setup: [u8; 8] = self.ep0.received_copy();
        let value = u32::from(setup[2]) | u32::from(setup[3]) << 8;
        let index = u32::from(setup[4]);
        let asked = u32::from(setup[6]) | u32::from(setup[7]) << 8;
        let stall = 'request: {
            match u32::from(setup[0]) << 8 | u32::from(setup[1]) {
                // Set address.
                0x0005 => {
                    self.pending_address = setup[2] & 0x7f;
                    self.address_pending = true;
                }
                // Get descriptor.
                0x8006 => {
                    let Some(d) = descriptor(value) else {
                        break 'request true;
                    };
                    if asked != 0 {
                        self.ep0_send(d, asked);
                        return;
                    }
                }
                // Set configuration.
                0x0009 => {
                    if setup[2] > 1 {
                        break 'request true;
                    }
                    self.configuration = setup[2];
                    if self.configuration != 0 {
                        self.ep1_configure();
                    }
                    self.aborted = true;
                }
                0x8008 => {
                    let c = usize::from(self.configuration);
                    self.ep0_send(&CONFIGURATIONS[c..c + 1], asked);
                    return;
                }
                0x8000 | 0x8100 => {
                    self.ep0_send(&ZERO, asked);
                    return;
                }
                // An endpoint's status.
                0x8200 => {
                    let halted = (index == EP_IN && self.stalled_in) || (index == EP_OUT && self.stalled_out);
                    self.ep0_send(if halted { &HALTED } else { &ZERO }, asked);
                    return;
                }
                // Clear an endpoint's halt.
                0x0201 => {
                    if !self.needs_reset {
                        self.clear_halt(index);
                    }
                }
                // Set an endpoint's halt.
                0x0203 => {
                    if index == EP_IN {
                        self.stall_in();
                    } else if index == EP_OUT {
                        self.stall_out();
                    }
                }
                // The highest logical unit.
                0xa1fe => {
                    self.ep0_send(&ZERO[..1], asked);
                    return;
                }
                // Bulk-only reset.
                0x21ff => {
                    self.needs_reset = false;
                    self.aborted = true;
                    self.endpoints_reset();
                }
                0x0102 | 0x0302 | 0x000b | 0x010b => {}
                _ => break 'request true,
            }
            false
        };
        self.select_endpoint(0);
        sie_write(CSR0, if stall { 0x60 } else { 0x48 });
    }

    fn bus_poll(&mut self) {
        // A frame's start is pending: acknowledge it.
        if CON0.read() & bit(13) != 0 {
            CON0.set(bit(12));
        }
        let bus = sie_read(INT_USB);
        let tx = sie_read(INT_TX1);
        // Bus reset.
        if bus & 0x04 != 0 {
            self.selected = 0xff;
            sie_write(FADDR, 0);
            self.configuration = 0;
            self.ep0_sending = false;
            self.address_pending = false;
            self.needs_reset = false;
            self.aborted = true;
            EP0_ADDRESS.write(self.ep0.address());
            sie_write(INT_USB_ENABLE, 0x07);
            sie_write(INT_TX1_ENABLE, 0x01);
            sie_write(INT_RX1_ENABLE, 0);
        }
        if tx & 0x01 != 0 {
            self.ep0_service();
        }
    }

    /// Waits for the IN endpoint to be free. False if the transfer was given up, by a reset or a lost host.
    fn tx_wait(&mut self) -> bool {
        // Sending would clear a stall the host hasn't.
        if self.stalled_in {
            return false;
        }
        let mut n = 0u32;
        while !self.aborted && self.configuration != 0 {
            self.select_endpoint(1);
            let csr = sie_read(TX_CSR1);
            if csr & 0x01 == 0 {
                if csr & 0x80 != 0 {
                    sie_write(TX_CSR1, csr & !0x80);
                }
                self.tx_status = csr & !0x80;
                return true;
            }
            if n & 15 == 15 {
                self.bus_poll();
            }
            if n & 0xfff == 0xfff {
                app::feed();
            }
            n = n.wrapping_add(1);
        }
        false
    }

    /// Sends n bytes from `data`, an address the controller's DMA reads.
    fn tx_send(&mut self, data: u32, n: u32) -> bool {
        if !self.tx_wait() {
            return false;
        }
        sync_cpu();
        EP1_TX_ADDRESS.write(data);
        EP1_COUNT.write(n);
        sie_write(TX_CSR1, self.tx_status | 0x01);
        true
    }

    /// Waits for a packet, and returns its size, or None if the transfer was given up.
    fn rx_wait(&mut self) -> Option<u32> {
        let mut n = 0u32;
        while !self.aborted && self.configuration != 0 {
            self.select_endpoint(1);
            if sie_read(RX_CSR1) & 1 != 0 {
                let size = sie_read(RX_COUNT1) | sie_read(RX_COUNT2) << 8;
                memory_sync();
                return Some(size.min(PACKET));
            }
            if n & 15 == 15 {
                self.bus_poll();
            }
            if n & 0xfff == 0xfff {
                app::feed();
            }
            n = n.wrapping_add(1);
        }
        None
    }

    fn fail(&mut self, key: u8, code: u8) {
        self.sense_key = key;
        self.sense_code = code;
        // The rest are hosts trying what isn't supported, which is normal.
        if key == 3 {
            self.stats.errors += 1;
        }
    }

    /// The end of the data: if the host expected more than a short packet tells it, it gets a stall.
    fn finish_in(&mut self, last_packet: u32) {
        if self.moved < self.expected && (self.moved == 0 || last_packet == PACKET) {
            self.stall_in();
        }
    }

    /// Sends `size` bytes from `data`, an address the controller's DMA reads, or as many as the host expects.
    fn send_reply(&mut self, data: u32, size: u32) -> bool {
        let size = size.min(self.expected);
        let (mut sent, mut last) = (0, 0);
        while sent < size {
            last = (size - sent).min(PACKET);
            if !self.tx_send(data + sent, last) {
                return false;
            }
            sent += last;
        }
        self.moved = sent;
        self.finish_in(last);
        true
    }

    fn send_own_reply(&mut self, size: u32) {
        let data = self.reply.address();
        self.send_reply(data, size);
    }

    fn send_sector(&mut self, data: u32) -> bool {
        let mut at = 0;
        while at < SECTOR {
            if !self.tx_send(data + at, PACKET) {
                return false;
            }
            at += PACKET;
        }
        true
    }

    fn read_singly(&mut self, sector: &mut [u8; 512], first: u32, count: u32) {
        for s in 0..count {
            // The last packet may still be going out of this buffer.
            if !self.tx_wait() {
                return;
            }
            if !card::read(first + s, sector) {
                self.fail(3, 0x11);
                break;
            }
            self.stats.sectors_read += 1;
            if !self.send_sector(address(sector)) {
                return;
            }
            self.moved += SECTOR;
            app::feed();
        }
        self.finish_in(PACKET);
    }

    /// The card and USB can work at once, so while one sector goes out, DMA fetches the next into the other buffer.
    fn read_stream(&mut self, sectors: &mut [Sector; 2], first: u32, count: u32) {
        let [a, b] = sectors;
        let (mut out, mut next) = (whole(a), whole(b));
        if !card::stream_read(out) {
            card::stream_stop();
            self.read_singly(out, first, count);
            return;
        }
        let mut streaming = true;
        for s in 0..count {
            let more = s + 1 < count;
            // The last packet may still be going out of the buffer this overwrites.
            if !self.tx_wait() {
                if streaming {
                    card::stream_stop();
                }
                return;
            }
            let mut reading = None;
            if more && streaming {
                // Safety: finished below, or dropped on the way out, never leaked.
                reading = unsafe { card::stream_begin(next) };
                if reading.is_none() {
                    card::stream_stop();
                    streaming = false;
                }
            }
            let started = reading.is_some();
            self.stats.sectors_read += 1;
            let sent = self.send_sector(address(out));
            // Finished here either way, which ends the stream's hold on `next`.
            let good = reading.is_none_or(card::Reading::finish);
            if !good {
                card::stream_stop();
                streaming = false;
            }
            if !sent {
                if streaming {
                    card::stream_stop();
                }
                return;
            }
            self.moved += SECTOR;
            app::feed();
            if more && (!started || !good) && !card::read(first + s + 1, next) {
                self.fail(3, 0x11);
                self.finish_in(PACKET);
                return;
            }
            core::mem::swap(&mut out, &mut next);
        }
        if streaming {
            card::stream_stop();
        }
        self.finish_in(PACKET);
    }

    fn read_sectors(&mut self, sectors: &mut [Sector; 2], first: u32, count: u32) {
        if count > 1 && self.total_sectors - (first + count) >= NEAR_THE_END && card::stream_start(first) {
            self.read_stream(sectors, first, count);
        } else {
            self.read_singly(whole(&mut sectors[0]), first, count);
        }
    }

    /// A sector from the host, into `to`, an address the controller's DMA writes. False, having dealt with it, if it
    /// won't come.
    fn receive_sector(&mut self, to: u32) -> bool {
        let mut at = 0;
        while at < SECTOR {
            self.rx_arm(to + at);
            let Some(got) = self.rx_wait() else {
                return false;
            };
            at += got;
            self.moved += got;
            if got < PACKET {
                // A short packet inside a sector.
                self.fail(5, 0x24);
                self.stall_out();
                return false;
            }
        }
        true
    }

    fn write_failed(&mut self) {
        self.fail(3, 0x0c);
        // With all the data in, the host has nothing to send, so it wouldn't find a stall and clear it. After a reset,
        // the command is gone, and a stall would only get in the next one's way.
        if self.moved < self.expected && !self.aborted {
            self.stall_out();
        }
    }

    fn write_singly(&mut self, sector: &mut [u8; 512], first: u32, count: u32) {
        for s in 0..count {
            if !self.receive_sector(address(sector)) {
                return;
            }
            if !card::write(first + s, sector) {
                self.write_failed();
                return;
            }
            self.stats.sectors_written += 1;
            app::feed();
        }
    }

    /// While one sector goes to the card, DMA receives the next. The host only hears of success after the card's
    /// status.
    ///
    /// Each turn sends the sector that came last to the card, receives the next into the other buffer meanwhile, then
    /// finishes the first, so that a buffer is the card's only while its sector goes out.
    fn write_stream(&mut self, sectors: &mut [Sector; 2], first: u32, count: u32) {
        let [a, b] = sectors;
        let (mut this, mut other) = (whole(a), whole(b));
        let mut got = self.receive_sector(address(this));
        let mut s = 0;
        loop {
            if !got {
                if !card::write_stream_stop() && self.sense_key == 0 {
                    self.write_failed();
                }
                return;
            }
            // Safety: finished below, never leaked.
            let writing = unsafe { card::write_stream_begin(this) };
            app::feed();
            if s + 1 == count {
                let last = writing.finish();
                if last {
                    self.stats.sectors_written += 1;
                }
                if !card::write_stream_stop() || !last {
                    self.write_failed();
                }
                return;
            }
            got = self.receive_sector(address(other));
            if !writing.finish() {
                // A failed stop leaves the stream's earlier sectors unconfirmed, and they're gone from here, so the host
                // has to hear about it and write them again. Otherwise this one can be retried, as it's still here.
                if !card::write_stream_stop() {
                    self.write_failed();
                    return;
                }
                if !card::write(first + s, this) {
                    self.write_failed();
                    return;
                }
                self.stats.sectors_written += 1;
                if got && !card::write(first + s + 1, other) {
                    self.write_failed();
                    return;
                }
                if got {
                    self.stats.sectors_written += 1;
                    self.write_singly(this, first + s + 2, count - s - 2);
                }
                return;
            }
            self.stats.sectors_written += 1;
            core::mem::swap(&mut this, &mut other);
            s += 1;
        }
    }

    fn write_sectors(&mut self, sectors: &mut [Sector; 2], first: u32, count: u32) {
        if count > 1 && self.total_sectors - (first + count) >= NEAR_THE_END && card::write_stream_start(first) {
            self.write_stream(sectors, first, count);
        } else {
            self.write_singly(whole(&mut sectors[0]), first, count);
        }
    }

    fn no_data_expected(&mut self) {
        if self.expected != 0 {
            self.fail(5, 0x20);
            if self.to_host {
                self.stall_in();
            } else {
                self.stall_out();
            }
        }
    }

    /// Nothing to send, for a command that failed: the host gets a stall if it expected data.
    fn refuse(&mut self) {
        if self.to_host {
            self.moved = 0;
            self.finish_in(0);
        } else if self.expected != 0 {
            self.stall_out();
        }
    }

    fn command_run(&mut self, sectors: &mut [Sector; 2], c: &[u8; 16]) {
        let (asked_key, asked_code) = (self.sense_key, self.sense_code);
        if c[0] != 0x03 {
            self.sense_key = 0;
            self.sense_code = 0;
        }
        match c[0] {
            // Test unit ready.
            0x00 => {
                if self.ejected {
                    self.fail(2, 0x3a);
                }
                self.no_data_expected();
            }
            // Request sense.
            0x03 => {
                let r = &mut self.reply.0;
                r[..18].fill(0);
                r[0] = 0x70;
                r[2] = asked_key;
                r[7] = 10;
                r[12] = asked_code;
                self.sense_key = 0;
                self.sense_code = 0;
                self.send_own_reply(18);
            }
            // Inquiry.
            0x12 => {
                if c[1] & 1 != 0 {
                    self.fail(5, 0x24);
                    self.moved = 0;
                    self.finish_in(0);
                } else {
                    let inquiry = if smk() { &SMK37_INQUIRY } else { &FM1_INQUIRY };
                    self.send_reply(read_address(&inquiry.0), 36);
                }
            }
            // Mode sense (6).
            0x1a => {
                self.reply.0[..4].copy_from_slice(&[3, 0, 0, 0]);
                self.send_own_reply(4);
            }
            // Mode sense (10).
            0x5a => {
                self.reply.0[..8].fill(0);
                self.reply.0[1] = 6;
                self.send_own_reply(8);
            }
            // Start stop unit.
            0x1b => {
                if c[4] & 3 == 2 {
                    self.ejected = true;
                }
                self.no_data_expected();
            }
            // Prevent or allow removal: we don't hold a disk in. Verify. Synchronize cache: there's no cache here.
            0x1e | 0x2f | 0x35 => self.no_data_expected(),
            // Read format capacities.
            0x23 => {
                let r = &mut self.reply.0;
                r[..12].fill(0);
                r[3] = 8;
                r[4..8].copy_from_slice(&self.total_sectors.to_be_bytes());
                r[8] = 2;
                r[10] = (SECTOR >> 8) as u8;
                self.send_own_reply(12);
            }
            // Read capacity (10).
            0x25 => {
                if self.ejected {
                    self.fail(2, 0x3a);
                    self.moved = 0;
                    self.finish_in(0);
                } else {
                    self.reply.0[..4].copy_from_slice(&(self.total_sectors - 1).to_be_bytes());
                    self.reply.0[4..8].copy_from_slice(&SECTOR.to_be_bytes());
                    self.send_own_reply(8);
                }
            }
            // Read (10), write (10).
            0x28 | 0x2a => {
                let first = u32::from_be_bytes([c[2], c[3], c[4], c[5]]);
                let count = u32::from(c[7]) << 8 | u32::from(c[8]);
                let write = c[0] == 0x2a;
                if self.ejected {
                    self.fail(2, 0x3a);
                } else if first > self.total_sectors || count > self.total_sectors - first {
                    self.fail(5, 0x21);
                } else if self.expected != count * SECTOR || self.to_host == write {
                    self.fail(5, 0x24);
                }
                if self.sense_key != 0 {
                    self.refuse();
                } else if write {
                    self.write_sectors(sectors, first, count);
                } else {
                    self.read_sectors(sectors, first, count);
                }
            }
            _ => {
                self.fail(5, 0x20);
                self.refuse();
            }
        }
    }

    /// Waits out a stall the host has to clear.
    fn stall_cleared(&mut self) -> bool {
        let mut n = 0u32;
        while (self.stalled_in || self.stalled_out) && !self.aborted && self.configuration != 0 {
            self.bus_poll();
            if n & 0xff == 0xff {
                app::feed();
            }
            n = n.wrapping_add(1);
        }
        !self.aborted && self.configuration != 0
    }

    /// A command block, as it came.
    fn command_handle(&mut self, sectors: &mut [Sector; 2], block: &[u8; 31]) {
        self.tag = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        self.expected = u32::from_le_bytes([block[8], block[9], block[10], block[11]]);
        self.to_host = block[12] & 0x80 != 0;
        self.moved = 0;
        let mut c = [0u8; 16];
        c.copy_from_slice(&block[15..31]);
        // The next packet after this one is either the data, or this endpoint's next command.
        if self.to_host || self.expected == 0 {
            self.rx_arm_command();
        }
        self.aborted = false;
        self.command_run(sectors, &c);
        if self.aborted || !self.stall_cleared() {
            return;
        }
        if !self.to_host && self.expected != 0 {
            self.rx_arm_command();
        }
        let s = &mut self.status_block.0;
        s[0..4].copy_from_slice(&0x5342_5355u32.to_le_bytes());
        s[4..8].copy_from_slice(&self.tag.to_le_bytes());
        s[8..12].copy_from_slice(&(self.expected - self.moved).to_le_bytes());
        s[12] = if self.sense_key != 0 { 1 } else { 0 };
        let data = self.status_block.address();
        self.tx_send(data, 13);
        app::feed();
    }
}

impl Msc {
    pub fn poll(&mut self) {
        let Msc { io, sectors } = self;
        io.bus_poll();
        if io.configuration == 0 || io.needs_reset {
            return;
        }
        io.select_endpoint(1);
        if sie_read(RX_CSR1) & 1 == 0 {
            return;
        }
        let size = sie_read(RX_COUNT1) | sie_read(RX_COUNT2) << 8;
        memory_sync();
        let block: [u8; 31] = io.command_block.received_copy();
        if size != 31
            || u32::from_le_bytes([block[0], block[1], block[2], block[3]]) != 0x4342_5355
            || block[13] & 15 != 0
            || block[14] == 0
            || block[14] > 16
        {
            // A bad command block: bulk-only says stall both until the host resets the device.
            io.needs_reset = true;
            io.stats.errors += 1;
            io.rx_arm_command();
            io.stall_in();
            io.stall_out();
            return;
        }
        io.command_handle(sectors, &block);
    }

    pub fn configured(&self) -> bool {
        self.io.configuration != 0
    }

    /// The host ejected the disk.
    pub fn ejected(&self) -> bool {
        self.io.ejected
    }

    pub fn stats(&self) -> Stats {
        self.io.stats
    }

    pub fn start(&mut self, sectors: u32) {
        let io = &mut self.io;
        io.total_sectors = sectors;
        io.selected = 0xff;
        CON0.write(0);
        CON1.write(0);
        IO_CON0.write(0x0c);
        hal_delay_us(25000);
        // USB clock: PLL 48 MHz.
        CLOCK_CON1.clear(3);
        let ep0 = io.ep0.address();
        EP0_ADDRESS.write(ep0);
        for n in 0..3 {
            EP1_RX_ADDRESS.add(8 * n).write(ep0);
        }
        EP4_RX_ADDRESS.write(ep0);
        IO_CON0.set(bit(10) | bit(9));
        IO_CON0.clear(bit(11));
        IO_CON0.set(bit(12));
        CON0.clear(0x4035);
        CON1.write(0);
        CON0.set(1);
        IO_CON0.clear(0xf0);
        // D+ pull-up: on the bus.
        IO_CON0.set(0x40);
        CON0.set(0x3c);
        sie_write(POWER, 0x60);
        sie_write(INT_USB_ENABLE, 0x07);
        sie_write(INT_TX1_ENABLE, 0x01);
        sie_write(INT_TX2_ENABLE, 0);
        sie_write(INT_RX1_ENABLE, 0);
        sie_write(INT_RX2_ENABLE, 0);
        CON0.clear(0x1f << 19);
        io.configuration = 0;
    }

    pub fn stop(&mut self) {
        CON0.write(0);
        IO_CON0.write(0x0c);
    }
}
