// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
//! FM-1 (AC791N / WL82) radio start-up, the part before the BT block: the shared Wi-Fi front end that the BT RF init
//! runs first (docs/BLE-HW-FACTS.md §5.2, §16; branch feat/ble-facts d907ce3, "HW §n" below). Used by fm1_ble.rs.
//!
//! The long constant parts (§16.1 groups 2-6, 8-13: about 34,000 register words on a second boot) are a small program
//! in ble_rf_tables.rs, captured by tools/ble_rf_capture.py from the stock V15 firmware in the emulator and committed.
//! This file runs it, in boot-2 order:
//!
//! - register write: §16.1 groups 2, 3, 5, 6, 8, 9, 12 (0x11900-0x11964, 0x14040-0x1405C, the MAC window); bit 14 of
//!   0x11900 kept as found (§16.2: a periodic routine's bit, not a trim)
//! - BBP window write / read: §16.3: entry address, data, commit 1 / address, commit 2, the read-back register; direct
//!   BBP registers; the port 0x3101C (§5.2: bit 17 start, bit 16 read)
//! - RF-die LUT words: §5.2 SPI port 0x14028 / 0x1402C, the five-high five-low kick (§16.1 group 11)
//! - trim field: §14.5, §16.4: the next write takes a field from VM 106 / 107 / 108 / 187 (the bytes given to
//!   [`Rf::run`]); a read-modify-write field goes into the last read-back
//! - delay: the gaps of 20 us or more in stock's trace
//! - VCO scan: §16.1 group 7: our own search ([`Rf::vco_scan`] below), not stock's
//! - replay: §16.1 group 10: the BBP reload, the same as the first load (group 4)
//! - skipped: §16.2: the window-D read-back loop (DC / IQ [I]), TODO(hardware)
//! - section marker: where a §16.1 group starts (found by register pattern: ble_rf_capture.py sections()): 2 analog
//!   word, 3 radio config, 4 BBP first load, 5 crystal trim (VM 106), 6 analog init, 7 VCO scan, 8 post-scan set-up,
//!   10 BBP second phase, 11 RF-die LUT, 13 VM 108; then fm1_ble.rs `rf_init` marks 14 (the BT block) and 15 (done).
//!   felucca's console 'bletrim' prints the last one entered and the set: on a unit where the start-up stops, the group
//!   it stopped in
//!
//! TODO(hardware), the unknowns in HW §12 and §16.2:
//! - the window-D read-back loop (§16.2, 3,537 transactions in stock's boot 2): left out; its results depend on
//!   read-backs the emulator returns as 0 (U17). §18.3 step 3, the read-back build: write a window entry and read it
//!   back through 0xD3 (a read-back console command still to write, HW §18), then decide how to measure and write
//!   those entries;
//! - the RF-die LUT words a stored trim changes (VM 107 -> entries 0xE0-0xFF of both words, VM 187 bytes 48-63, the
//!   scan -> word-1 entries 0x00-0x7F): written as captured (the emulator's calibration), their mapping unknown. §16.5
//!   row 1: after the scan, read the LUT back (SPI command 0x6);
//! - the VCO band and the final 0x11934-0x11940: the scan below finds an in-range band, but stock's final writes
//!   (constants of the capture) are kept; compare with the emulator's on hardware (§18.3 step 3);
//! - the BT TX trims (0x2FC08, 0x2FC10: §16.2, live from BBP read-backs): fm1_ble.rs keeps stock's first values;
//! - delays: stock's gaps in the emulator, not a datasheet's settle times.
//!
//! Taken here under MPL-2.0. Changed by m-apps: in Rust, its state one struct ([`Rf`]) where the C had statics.
use super::ble_rf_tables::{ADDR, BBP_FLAGS, FIELDS, NFIELDS, PROG};
use super::fm1_cc::{clear, rd, set, wr};
use super::fm1_irq::{icfg, irq_off, irq_on};
use super::fm1_rf_cal::{Cal, rfd_w0, rfd_w1};
use super::fm1_time::delay_us;

/// HW §5.2: bit17 start, bit16 read, [15:8] reg, [7:0] data.
const BBP_PORT: u32 = 0x3101C;
/// HW §5.2: [22:20] busy, 0x10000 | address << 8 | command.
const RFSPI_CTL: u32 = 0x14028;
const RFSPI_DATA: u32 = 0x1402C;
/// Written 0 while waiting (HW §5.2).
const RFSPI_WAIT: u32 = 0x14034;
/// HW §16.2: bit 14 kept.
pub const WL_A00: u32 = 0x11900;
/// HW §16.1 group 7: bit 24 pulsed per step.
const WL_VCO_EN: u32 = 0x11934;
/// [25:19] the band; bits 16, 18 pulsed per step.
const WL_VCO_BAND: u32 = 0x11938;
const WL_VCO_FINE: u32 = 0x1193C;
/// Bit 28: start a comparator measurement.
const WL_CAL_START: u32 = 0x11968;
/// Strobe 1 x 8 then 0; read: bit 17 low, bit 18 high.
const WL_CAL_STROBE: u32 = 0x11978;

// Values the sheet does not settle, named so nobody mistakes them for measurements.
/// The BBP start bit polled to 0 (stock reads it once; the bound is ours).
const BBP_POLLS: u32 = 1000;
/// HW §5.2 busy [22:20]; the bound is ours.
const RFSPI_POLLS: u32 = 1000;
/// Stock's scan used bands 0..63 (§16.1 group 7, 0x11938 [25:19]).
const VCO_BANDS: u32 = 64;
/// Between the step's pulse edges (stock: ~2 us in the emulator) [I].
const VCO_EDGE_US: u32 = 2;
/// Before a measurement (stock: ~45 us in the emulator) [I].
const VCO_SETTLE_US: u32 = 50;

/// §16.1 groups run as one burst: decoded first (trim fields applied), then written by burst_run with the interrupts
/// off, so nothing reads flash between the group's first and last write. Group 5, the 0x11930 ramp (1, 3, 7, 0xF,
/// 0x1F, 0x5F, 0x25F, then the VM 106 trims): on hardware, rf_init hung in group 5 in exactly the builds where a flash
/// data-cache line of ble_rf_prog began right after the write of 3 (ble_rf_prog at 8 mod 32: 1f0ab85, 3a153c3), and
/// ran in the builds where the first line began after 0x5F (24 mod 32: 1cc6e04, 510e616), the code identical. [I] the
/// ramp's first steps leave the flash path (XIP) unusable until it is complete, so a flash access there never returns
/// and the watchdog resets. A group that holds anything but register writes and trim fields, or more than BURST_MAX
/// writes, runs op by op as before (rf_burst 0 in 'bletrim'). (A RAM app runs from RAM, but the burst is kept as it was.)
const BURST_GROUPS: u32 = 1 << 5;
/// Group 5 has 16 writes (§16.1: 14 + 2).
const BURST_MAX: usize = 32;

/// What the start-up did (the calibration reads its BBP timeouts). Kept as felucca's console reads it, though only
/// the BBP timeouts are read here.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct Stat {
    pub ops: u32,
    pub bbp_timeouts: u32,
    pub spi_timeouts: u32,
    pub lut_words: u32,
    pub trims: u32,
    pub skipped: u32,
    pub delay_us: u32,
    /// Bit g: §16.1 group g entered (section markers).
    pub sections: u32,
    /// The last comparator word.
    pub scan_result: u32,
    /// Ops when the last group was entered.
    pub group_op: u32,
    pub ran: u8,
    pub scan_found: u8,
    pub scan_band: u8,
    pub scan_steps: u8,
    pub bad_op: u8,
    /// Writes of the last burst group (BURST_GROUPS); 0: none ran.
    pub burst: u8,
    /// The last group entered (0: not started).
    pub section: u8,
}

/// The BT block (fm1_ble.rs `rf_init`, HW §16.1 group 14).
pub const SECT_BT: u32 = 14;
pub const SECT_DONE: u32 = 15;

/// The Wi-Fi IQ entries the replay writes last (RF-CALIBRATION.md §4.1: window D 0x04-0x09 the TX gains, phases and
/// DCs, 0x61 / 0x62 their low bits; D' 0x09-0x0C the RX gains and phases): the Wi-Fi PHY's alone (the BT side takes its
/// compensation from the record into 0x2FC08 / 0x2FC10 at the start, fm1_ble.rs, and never reads these). Kept as
/// written, for fm1-base's fm1_rf_cal_wifi_iq to put another record's fields in after the start (m-apps doesn't).
/// Keeping them changes no write.
const WIQ_N: usize = 12;
const WIQ_W: [u8; WIQ_N] = [2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3];
const WIQ_E: [u8; WIQ_N] = [0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x61, 0x62, 0x09, 0x0A, 0x0B, 0x0C];

/// The radio's start-up, its calibration and what they found: the C's statics, all zeros until it runs.
pub struct Rf {
    pub stat: Stat,
    /// Set by the firmware: measure (true), or the stored 187 (false).
    pub cal_on: bool,
    /// What was measured (the log reads it).
    pub rfc: Cal,
    /// The record built from it.
    pub x187: [u8; 68],
    pub scanned: bool,
    pub trimmed: bool,
    pub scan_rc: i32,
    pub trim_rc: i32,
    #[allow(dead_code)]
    wiq: [u8; WIQ_N],
    /// Bit k: entry k was written by the replay.
    #[allow(dead_code)]
    wiq_mask: u16,
    /// HW §2.4: 81 entries x 3 words, one per 1 MHz channel; stock V15's form {i | i << 8, 0, 0} [M:d], meaning [I].
    /// The BT block is given its address, so it stays where it is, in the app's RAM, as the C's static did.
    pub pll_tbl: [u32; 81 * 3],
}

/// The data of VM 106, 107, 108, 187, in that order.
pub type Records<'a> = [&'a [u8]; 4];

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn u32le(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

/// §14.5 / §16.4
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn field(v: u32, f: u8, rec: &Records) -> u32 {
    let d = &FIELDS[4 * usize::from(f)..4 * usize::from(f) + 4];
    let x = u32::from(rec[usize::from(d[0] & 3)][usize::from(d[1])]);
    let (s, w) = (u32::from(d[2]), u32::from(d[3] & 0x7F));
    // VM 108: a non-zero byte replaces the default.
    if d[3] & 0x80 != 0 {
        return if x != 0 { x } else { v };
    }
    let m = ((1u32 << w) - 1) << s;
    (v & !m) | (x << s & m)
}

/// The rest of the replay with measured values: a 187 field's write gets the record's true fields (the captured ones
/// are 1-2-bit slices found by perturbation).
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn is187(trim: &[u8]) -> bool {
    trim.iter().any(|&t| FIELDS[4 * usize::from(t)] & 3 == 3)
}

/// The burst's writes, from RAM: no flash fetch, no flash data (a and v are on the caller's stack) between the first
/// write and the last. In .ram_text, as the C had it, which a RAM app's linker script keeps with the rest.
// Safety: the slot's linker script puts .ram_text with the rest of the code.
#[unsafe(link_section = ".ram_text")]
#[inline(never)]
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn burst_run(a: &[u32], v: &[u32]) {
    for (&a, &v) in a.iter().zip(v) {
        wr(a, v);
    }
}

impl Rf {
    pub const fn new() -> Rf {
        Rf {
            stat: Stat {
                ops: 0,
                bbp_timeouts: 0,
                spi_timeouts: 0,
                lut_words: 0,
                trims: 0,
                skipped: 0,
                delay_us: 0,
                sections: 0,
                scan_result: 0,
                group_op: 0,
                ran: 0,
                scan_found: 0,
                scan_band: 0,
                scan_steps: 0,
                bad_op: 0,
                burst: 0,
                section: 0,
            },
            cal_on: false,
            rfc: Cal::new(),
            x187: [0; 68],
            scanned: false,
            trimmed: false,
            scan_rc: 0,
            trim_rc: 0,
            wiq: [0; WIQ_N],
            wiq_mask: 0,
            pll_tbl: [0; 81 * 3],
        }
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn section(&mut self, g: u32) {
        self.stat.section = g as u8;
        self.stat.group_op = self.stat.ops;
        self.stat.sections |= 1 << (g & 31);
    }

    /// One BBP transaction -> the port's [7:0].
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn bbp(&mut self, reg: u32, data: u32, read: u32) -> u32 {
        let c = BBP_FLAGS | read << 16 | reg << 8 | data;
        wr(BBP_PORT, c);
        // HW §5.2: the command, then with the start bit.
        wr(BBP_PORT, c | 1 << 17);
        let mut i = 0;
        while i < BBP_POLLS && rd(BBP_PORT) & 1 << 17 != 0 {
            i += 1;
        }
        if i == BBP_POLLS {
            self.stat.bbp_timeouts += 1;
        }
        rd(BBP_PORT) & 0xFF
    }

    /// The BBP windows (HW §16.3): window w's commit, address, data, read-back = 0xC8 + 4w .. 0xCB + 4w.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn win_wr(&mut self, w: u32, entry: u32, v: u32) {
        self.bbp(0xC9 + 4 * w, entry, 0);
        self.bbp(0xCA + 4 * w, v & 0xFF, 0);
        self.bbp(0xC8 + 4 * w, 1, 0);
    }

    /// [I] that it returns the entry: U17.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn win_rd(&mut self, w: u32, entry: u32) -> u32 {
        self.bbp(0xC9 + 4 * w, entry, 0);
        self.bbp(0xC8 + 4 * w, 2, 0);
        self.bbp(0xCB + 4 * w, 0, 1)
    }

    /// HW §5.2: one RF-die SPI command.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn rfspi(&mut self, addr: u32, cmd: u32, data: u32) {
        let mut i = 0;
        while i < RFSPI_POLLS && rd(RFSPI_CTL) >> 20 & 7 != 0 {
            wr(RFSPI_WAIT, 0);
            i += 1;
        }
        if i == RFSPI_POLLS {
            self.stat.spi_timeouts += 1;
        }
        wr(RFSPI_DATA, data);
        wr(RFSPI_CTL, 0x10000 | (addr & 0xFF) << 8 | (cmd & 0xF));
        // The kick: bit 4 five times high, five times low.
        for _ in 0..5 {
            set(RFSPI_CTL, 0x10);
        }
        for _ in 0..5 {
            clear(RFSPI_CTL, 0x10);
        }
    }

    // Our own calibration (fm1_rf_cal.rs, JieLi's, Apache-2.0): with cal_on set before run(), the replay measures this
    // unit's radio instead of taking a stored record 187: the stock VCO scan at group 7's scan op, the trim before the
    // first write that carries a 187 field (after group 8's PLL set-up), then every 187 field, the RF-die table's banks
    // (word 1, entries 0-127) and RX DC (word 0, 128-223) from what it measured. The x187 given to run() is ignored
    // then. docs/bt/RF-CALIBRATION.md.

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn wiq_keep(&mut self, w: u32, entry: u32, v: u32) {
        for k in 0..WIQ_N {
            if u32::from(WIQ_W[k]) == w && u32::from(WIQ_E[k]) == entry {
                self.wiq[k] = v as u8;
                self.wiq_mask |= 1 << k;
            }
        }
    }

    /// The first 187 field: measure now.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn before_187(&mut self) {
        if self.cal_on && !self.trimmed {
            self.trimmed = true;
            self.trim_rc = self.cal_trim();
        }
    }

    /// An RF-die word, patched.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn lut(&self, entry: u32, cmd: u32, w: u32) -> u32 {
        if !self.cal_on {
            return w;
        }
        if cmd == 0xD && entry < 128 && self.scanned {
            return rfd_w1(&self.rfc, entry, w);
        }
        if cmd == 0xE && (128..224).contains(&entry) && self.trimmed {
            return rfd_w0(entry, w, &self.x187);
        }
        w
    }

    /// §16.1 group 7, our own search (not stock's): with the fine code stock starts from, find a band whose comparator
    /// reads neither low (bit 17) nor high (bit 18): bisection over VCO_BANDS, a higher band when low. The step: the
    /// band into 0x11938 [25:19], 0x11938 bits 16 / 18 and 0x11934 bit 24 pulsed low then high, 0x11968 bit 28, eight
    /// strobes of 0x11978 then 0, the comparator in 0x11978 (the registers and the order as stock's boot-2 trace shows
    /// them: BLE-STACK.md §8.1). Its result is reported only: stock's final band and fine code follow as constants
    /// (TODO(hardware) above).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn vco_scan(&mut self, fine0: u32) {
        let (mut lo, mut hi) = (0u32, VCO_BANDS - 1);
        wr(WL_VCO_FINE, fine0);
        self.stat.scan_found = 0;
        self.stat.scan_steps = 0;
        while lo <= hi {
            let b = (lo + hi) / 2;
            wr(WL_VCO_BAND, (rd(WL_VCO_BAND) & !(0x7F << 19)) | b << 19);
            clear(WL_VCO_BAND, 1 << 16);
            clear(WL_VCO_BAND, 1 << 18);
            clear(WL_VCO_EN, 1 << 24);
            delay_us(VCO_EDGE_US);
            set(WL_VCO_BAND, 1 << 16);
            delay_us(VCO_EDGE_US);
            set(WL_VCO_EN, 1 << 24);
            set(WL_VCO_BAND, 1 << 18);
            delay_us(VCO_SETTLE_US);
            set(WL_CAL_START, 1 << 28);
            for _ in 0..8 {
                wr(WL_CAL_STROBE, 1);
            }
            wr(WL_CAL_STROBE, 0);
            let r = rd(WL_CAL_STROBE);
            self.stat.scan_steps = self.stat.scan_steps.wrapping_add(1);
            self.stat.scan_result = r;
            self.stat.scan_band = b as u8;
            if r & 1 << 17 != 0 {
                // Low: a higher band.
                lo = b + 1;
            } else if r & 1 << 18 != 0 {
                if b == 0 {
                    break;
                }
                // High: a lower band.
                hi = b - 1;
            } else {
                self.stat.scan_found = 1;
                break;
            }
        }
    }

    /// A group of BURST_GROUPS, from its first op at pc: decode its register writes and trim fields up to the next
    /// section marker, then write them in one burst with the interrupts off (icfg bit 9 is the global enable that cli /
    /// sti clear and set: the emulator's model [I]). -> the pc after the group, or pc itself when the group holds
    /// anything else (the caller then runs it op by op).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn burst(&mut self, pc: usize, end: usize, rec: &Records) -> usize {
        let p = &PROG;
        let (mut a, mut v) = ([0u32; BURST_MAX], [0u32; BURST_MAX]);
        let (mut n, mut ops, mut trims, mut q) = (0usize, 0u32, 0u32, pc);
        let mut trim = [0u8; 8];
        let mut nt = 0;
        while q < end && p[q] != 0x90 {
            if p[q] < 0x80 {
                // (0x11900 needs a read: not in a burst.)
                if n == BURST_MAX || q + 5 > end || ADDR[usize::from(p[q])] == WL_A00 {
                    return pc;
                }
                a[n] = ADDR[usize::from(p[q])];
                v[n] = u32le(&p[q + 1..]);
                for &t in &trim[..nt] {
                    v[n] = field(v[n], t, rec);
                }
                n += 1;
                nt = 0;
                q += 5;
            } else if p[q] == 0x8B && q + 3 <= end && nt < trim.len() && p[q + 1] < NFIELDS {
                // (A read-back field: only window writes use it.)
                trim[nt] = p[q + 1];
                nt += 1;
                trims += 1;
                q += 3;
            } else {
                return pc;
            }
            ops += 1;
        }
        // A trim field for an op outside the group.
        if nt != 0 {
            return pc;
        }
        let ie = icfg() & 0x200;
        irq_off();
        burst_run(&a[..n], &v[..n]);
        if ie != 0 {
            irq_on();
        }
        self.stat.ops += ops;
        self.stat.trims += trims;
        self.stat.burst = n as u8;
        q
    }

    /// One op more, counted (each write of a packed op counts as the op it replaced).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn tick(&mut self) {
        self.stat.ops += 1;
    }

    /// The packed ops (capture format 3, tools/ble_rf_pack.py), write for write the format-2 ops they replace: OP_LUT2
    /// 0x91: cmd A, cmd B, first entry, count - 1, nA, nB, the deltas (4 bytes each), then count pairs of delta indices
    /// (A's word, then B's, for each entry; a word is its command's last + the delta, from 0) -> the bytes used.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn lut2(&mut self, pc: usize) -> usize {
        let p = &PROG[pc..];
        let (n, na) = (usize::from(p[4]) + 1, usize::from(p[5]));
        let (da, (mut wa, mut wb)) = (7, (0u32, 0u32));
        let db = da + 4 * na;
        let ix = db + 4 * usize::from(p[6]);
        let (first, cmd_a, cmd_b) = (u32::from(p[3]), u32::from(p[1]), u32::from(p[2]));
        for i in 0..n {
            if i != 0 {
                self.tick();
            }
            let entry = first + i as u32;
            wa = wa.wrapping_add(u32le(&p[da + 4 * usize::from(p[ix + 2 * i])..]));
            self.rfspi(entry, cmd_a, self.lut(entry, cmd_a, wa));
            self.tick();
            wb = wb.wrapping_add(u32le(&p[db + 4 * usize::from(p[ix + 2 * i + 1])..]));
            self.rfspi(entry, cmd_b, self.lut(entry, cmd_b, wb));
        }
        self.stat.lut_words += 2 * n as u32;
        ix + 2 * n
    }

    /// OP_WWG 0x92: window, n, n entries, records r, then r x n values: the window writes of the n entries, r times.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn wwg(&mut self, pc: usize) -> usize {
        let p = &PROG[pc..];
        let n = usize::from(p[2]);
        let r = usize::from(p[3 + n]);
        let mut v = 4 + n;
        for k in 0..r {
            for j in 0..n {
                if k | j != 0 {
                    self.tick();
                }
                self.win_wr(u32::from(p[1] & 3), u32::from(p[3 + j]), u32::from(p[v]));
                v += 1;
            }
        }
        v
    }

    /// A write's value with its trim fields: with our calibration, a 187 field's write gets the record's own fields
    /// through `cal` (and any others as captured).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn trimmed_value(&mut self, mut v: u32, trim: &[u8], rec: &Records, cal: impl Fn(u32, &[u8; 68]) -> u32) -> u32 {
        if self.cal_on && is187(trim) {
            self.before_187();
            for &t in trim {
                if FIELDS[4 * usize::from(t)] & 3 != 3 {
                    v = field(v, t, rec);
                }
            }
            cal(v, &self.x187)
        } else {
            for &t in trim {
                v = field(v, t, rec);
            }
            v
        }
    }

    /// Runs PROG[pc, end) (one level of replay); rec = the data of VM 106, 107, 108, 187.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn exec(&mut self, mut pc: usize, end: usize, rec: &Records, depth: u32) {
        let p = &PROG;
        let mut trim = [0u8; 8];
        let (mut rmw, mut nt, mut last) = (false, 0usize, 0u32);
        while pc < end {
            let op = p[pc];
            self.tick();
            if op < 0x80 {
                // A register write.
                let a = ADDR[usize::from(op)];
                let mut v = u32le(&p[pc + 1..]);
                if a == WL_A00 {
                    v = (v & !(1 << 14)) | (rd(a) & 1 << 14);
                }
                v = self.trimmed_value(v, &trim[..nt], rec, |v, x187| super::fm1_rf_cal::cal_reg(a, v, x187));
                if self.cal_on && self.scanned && a == WL_VCO_BAND {
                    // Our scan's bank, not the capture's.
                    v = (v & !(0x7F << 19)) | u32::from(self.rfc.bank121) << 19;
                }
                wr(a, v);
                nt = 0;
                rmw = false;
                pc += 5;
            } else if op & 0xFC == 0x80 {
                // A window write.
                let (w, entry) = (u32::from(op & 3), u32::from(p[pc + 1]));
                let v = if rmw { last } else { u32::from(p[pc + 2]) };
                let v = self.trimmed_value(v, &trim[..nt], rec, |v, x187| super::fm1_rf_cal::cal_win(w, entry, v, x187));
                self.win_wr(w, entry, v);
                self.wiq_keep(w, entry, v);
                nt = 0;
                rmw = false;
                pc += 3;
            } else if op & 0xFC == 0x84 {
                // A window read.
                last = self.win_rd(u32::from(op & 3), u32::from(p[pc + 1]));
                pc += 2;
            } else if op == 0x88 {
                self.bbp(u32::from(p[pc + 1]), u32::from(p[pc + 2]), 0);
                pc += 3;
            } else if op == 0x89 {
                last = self.bbp(u32::from(p[pc + 1]), 0, 1);
                pc += 2;
            } else if op == 0x8A {
                // LUT words: command, first entry, count, words.
                let (cmd, first, n) = (u32::from(p[pc + 1]), u32::from(p[pc + 2]), usize::from(p[pc + 3]));
                for i in 0..n {
                    let entry = first + i as u32;
                    self.rfspi(entry, cmd, self.lut(entry, cmd, u32le(&p[pc + 4 + 4 * i..])));
                }
                self.stat.lut_words += n as u32;
                pc += 4 + 4 * n;
            } else if op == 0x8B {
                // A trim field for the next write.
                if nt < trim.len() && p[pc + 1] < NFIELDS {
                    trim[nt] = p[pc + 1];
                    nt += 1;
                } else {
                    self.stat.bad_op = op;
                }
                rmw |= p[pc + 2] & 1 != 0;
                self.stat.trims += 1;
                pc += 3;
            } else if op == 0x8C {
                let n = u32::from(p[pc + 1]) | u32::from(p[pc + 2]) << 8;
                delay_us(n);
                self.stat.delay_us += n;
                pc += 3;
            } else if op == 0x8D {
                if self.cal_on {
                    // The stock scan (fm1_rf_cal.rs).
                    self.scan_rc = self.cal_scan();
                    self.scanned = true;
                } else {
                    self.vco_scan(u32le(&p[pc + 1..]));
                }
                pc += 5;
            } else if op == 0x8E {
                let a = usize::from(p[pc + 1]) | usize::from(p[pc + 2]) << 8;
                let n = usize::from(p[pc + 3]) | usize::from(p[pc + 4]) << 8;
                if depth == 0 && a + n <= PROG.len() {
                    self.exec(a, a + n, rec, 1);
                } else {
                    self.stat.bad_op = op;
                }
                pc += 5;
            } else if op == 0x8F {
                // TODO(hardware): the read-back loop.
                self.stat.skipped += u32::from(p[pc + 1]) | u32::from(p[pc + 2]) << 8;
                pc += 3;
            } else if op == 0x91 {
                // The LUT words of two commands (packed).
                pc += self.lut2(pc);
            } else if op == 0x92 {
                // Window writes repeating n entries (packed).
                pc += self.wwg(pc);
                nt = 0;
                rmw = false;
            } else if op == 0x90 {
                // A section marker (§16.1 group).
                let g = p[pc + 1];
                self.section(u32::from(g));
                pc += 2;
                if g < 32 && BURST_GROUPS >> g & 1 != 0 {
                    // (Unchanged: run op by op below.)
                    pc = self.burst(pc, end, rec);
                }
            } else {
                // 0xFF: the end (anything else: stop there).
                if op != 0xFF {
                    self.stat.bad_op = op;
                }
                return;
            }
        }
    }

    /// HW §16.1 groups 2-13 with the stored trims (the data of VM 106, 107, 108, 187, in that order).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn run(&mut self, rec: &Records) {
        self.exec(0, PROG.len(), rec, 0);
        if self.cal_on && self.trimmed {
            // The attenuators as JieLi's trim leaves them (its last rx_tx_cal_attenuation(0)); the replay's words are
            // a start without a trim (measured against the SDK build).
            super::fm1_rf_cal::cal_att(0);
        }
        self.stat.ran = 1;
    }
}
