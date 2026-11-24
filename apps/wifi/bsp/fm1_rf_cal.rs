// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 Zhuhai Jieli Technology Co., Ltd.
// Copyright (C) 2026 Charles Vestal
//! FM-1 (AC791N / WL82): the 2.4 GHz radio's own calibration, run at every boot before the BLE radio starts, so a unit
//! with no stored calibration (no SDK VM, no record 187) still gets a working radio. It follows JieLi's calibration in
//! wl_rf_common.a (wf_rf_cal.c, wf_rf_init.c, wf_phy_mac_lib.c) from the AC79 SDK,
//! https://gitee.com/Jieli-Tech/fw-AC79_AIoT_SDK (cpu/wl82/liba/), Apache-2.0.
//!
//! Changed from the original (Apache-2.0 section 4(b)): rewritten in C for felucca-bsp from the library's code, with
//! timeouts added on every hardware wait and the Wi-Fi-only steps left out (RX IQ, the 187 save, the prints). Changed
//! again by m-apps: in Rust, from that C (fm1_rf_cal.h), its steps methods of the start-up's state (fm1_ble_rf.rs
//! `Rf`), and without FM1_RF_CAL_FULL's steps, which the C built only on request and m-apps never did.
//!
//! The spec is docs/bt/RF-CALIBRATION.md ("§n" below). It uses fm1_ble_rf.rs's helpers: `bbp` (one BBP port
//! transaction), `win_wr` / `win_rd` (the windows), and the registers and the timer.
//!
//! What the replay (fm1_ble_rf.rs) calls, in boot order:
//! - `cal_scan`: where its VCO scan op is (§16.1 group 7): the VCO bank scan as wfa_pll_bank_scan(121) does it (§3.2).
//!   Keeps the bank table for the RF-die patch (§6).
//! - `cal_trim`: after group 8's PLL set-up for 2442 MHz, before the first write that carries a 187 field:
//!   wf_rf_trim without RX IQ (§4), then the 187 record built from what it measured (§9 item 5).
//! - [`cal_reg`], [`cal_win`]: instead of the captured 187 fields: a register write with the record's fields put in at
//!   their true widths (quick_start_wifi_trim's writes, §4.5); the same for a window write.
//! - [`rfd_w0`], [`rfd_w1`]: the RF-die table: word 0 of entries 128-223 (RX DC, §6) and word 1 of entries 0-127 (the
//!   VCO bank of each entry's frequency code, §6, through [`bank`]).
//! - [`bt_trims`]: the BT modem's TX IQ / DC compensation from the record (§5).
//!
//! Where the library's code and the spec differ, the code wins (all checked against the IR, 2026-10-26):
//! - the BBP port (§3.5): the library's bbp_set polls bit 17 before its two writes, and bbp_rd does not poll at all;
//!   `bbp` polls after each command. The words written and their order are the same, so `bbp` is used; it only never
//!   starts a command while one is running.
//! - wf_rx_tune_test (§4.1b): bit 0 of 0x11968 is not cleared with [9:1]; the codes are written into [9:1] with the mask
//!   0x3FE. The result is a second wlrf_get_(1) after the done bit, not the word that showed it.
//! - the midbank search (§3.2 step 2) sets fbds 73 once, before the first mid.
//! - get_wfa_pll_bank (§3.2 step 4) also compares limit[47] (= top) for i = 47.
//! - the RF-die word-0 patch (§6) puts in rx_dc_tune[g][1] << 22 unmasked (16 bits shifted: only its low 10 stay).
//!
//! Library behaviour we take as bugs, and what this file does:
//! - get_wfa_pll_midbank: when no mid shows a change, it returns the last comparator result | 15, which is 15 (the scan
//!   then walks banks 15 down to 225 & 127). Here: top TOP_FALLBACK (63), and the scan returns E_MIDBANK after
//!   finishing.
//! - txiq_tune_soft steps 5 and 6 (FM1_RF_CAL_FULL only, not built here): the best gain offset (-423..423) and phase
//!   (-200..200) are kept in a signed byte, so one beyond +-127 wraps (and the wrapped gain offset picks the wrong side).
//!   felucca's C keeps their full value.
//! - the library's waits (the RC tune engine, the estimator) have no timeout: here each is bounded by TIMER4 and
//!   counted; a step that times out stops, puts back its neutral values and the trim returns its code.
//!
//! Interrupts: the SDK runs all of this with interrupts on (§8); so does this file. Nothing else may drive the radio
//! meanwhile (the trim transmits into its own receiver).
use super::fm1_ble_rf::Rf;
use super::fm1_cc::{clear, rd, rmw, set, wr};
use super::fm1_time::{TICKS_PER_US, delay_us, ticks};

/// The RC tune engine, a run (§4.1b; "well under a millisecond" [L]).
const TUNE_US: u32 = 5000;
/// The estimator of one metric read (§4.6; its length unknown, R2).
const DEM_US: u32 = 5000;
/// The scan's top when no mid bank shows a change (see the bugs above).
const TOP_FALLBACK: u32 = 63;

pub const OK: i32 = 0;
/// The scan found no mid bank; it ran from TOP_FALLBACK.
const E_MIDBANK: i32 = -1;
/// The RC tune engine never finished: the filter registers left as found.
const E_FILTER: i32 = -2;
/// An estimator read timed out in the TX LO search: txlo 64 / 64.
const E_TXLO: i32 = -3;
/// The same in RX DC: the rest of the steps 0 (0x200).
const E_RXDC: i32 = -5;

/// What the calibration measured: the log reads some, felucca's console the rest.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct Cal {
    // The scan (§3.2).
    /// wfa_pll_bank_limit: [i] the fbds where bank top - i starts; [47] = top.
    pub limit: [u8; 48],
    /// mid | 15 (our unit: 0x3F).
    pub top: u8,
    /// The mid bank found (` 2394 bank: 37`); 0 when none.
    pub mid: u8,
    pub mid_found: bool,
    /// The bank set with fbds 121 (`setbak:33`).
    pub bank121: u8,
    // The trim (§4).
    /// The RC tune codes for 6, 10, 20 MHz (§4.1b).
    pub flt: [u8; 3],
    /// The TX LO search's best (§4.2 step 3).
    pub txlo_i: u8,
    pub txlo_q: u8,
    /// (Internal: an estimator timeout stops the running step.)
    abort: bool,
    /// Per gain step, I then Q: bit 9 = non-negative, [8:0] magnitude (§4.3).
    pub rx_dc: [[u16; 2]; 4],
    /// What is in window D: I gain, I phase, Q gain, Q phase, I DC, Q DC.
    pub tx: [i32; 6],
    // Microseconds per step (TIMER4).
    pub us_scan: u32,
    pub us_filter: u32,
    pub us_txlo: u32,
    pub us_txiq: u32,
    pub us_rxdc: u32,
    pub us_total: u32,
    // Failures.
    /// RC tune engine / estimator timeouts.
    pub to_filter: u32,
    pub to_dem: u32,
    /// `bbp`'s timeouts during the trim.
    pub bbp_timeouts: u32,
    /// Estimator reads made by the trim.
    pub dem_reads: u32,
    /// The trim's first error (E_*).
    pub err: i32,
}

impl Cal {
    pub const fn new() -> Cal {
        Cal {
            limit: [0; 48],
            top: 0,
            mid: 0,
            mid_found: false,
            bank121: 0,
            flt: [0; 3],
            txlo_i: 0,
            txlo_q: 0,
            abort: false,
            rx_dc: [[0; 2]; 4],
            tx: [0; 6],
            us_scan: 0,
            us_filter: 0,
            us_txlo: 0,
            us_txiq: 0,
            us_rxdc: 0,
            us_total: 0,
            to_filter: 0,
            to_dem: 0,
            bbp_timeouts: 0,
            dem_reads: 0,
            err: 0,
        }
    }
}

// ---- small register helpers ----

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn bit(a: u32, bit: u32, on: u32) {
    rmw(a, 1 << bit, (on & 1) << bit);
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn us_since(t0: u32) -> u32 {
    ticks().wrapping_sub(t0) / TICKS_PER_US
}

// The windows by the library's names (§3.5, HW §16.3).
/// "cca"
const C: u32 = 0;
/// "tx_backend"
const D: u32 = 2;
/// "rx_frontend"
const DP: u32 = 3;

/// The record's fields, little-endian (§4.1).
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn u32le(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn put32(p: &mut [u8], v: u32) {
    p[..4].copy_from_slice(&v.to_le_bytes());
}

/// A 10-bit field as the library reads it back (ashr 22).
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn sext10(v: u32) -> i32 {
    let v = v & 0x3FF;
    if v & 0x200 != 0 { v as i32 - 0x400 } else { v as i32 }
}

// ---- the analog side (wf_rf_init.c) ----

/// wlrf_get_: the analog read port (§3.2): 0x11968 [31:28] = sel, 0x11978 written 1 eight times then 0, read back.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn get(sel: u32) -> u32 {
    wr(0x11968, (rd(0x11968) & 0x0FFF_FFFF) | sel << 28);
    for _ in 0..8 {
        wr(0x11978, 1);
    }
    wr(0x11978, 0);
    rd(0x11978)
}

// wf_rf_en_set (§3.6): a group of enables, each set to one of six flags, in the library's order. Entry: (register -
// 0x11900) / 4 << 8 | bit << 3 | flag. Flags: A = receive | RX cal | TX cal | RX DC cal, RX = receive, R = receive |
// RX cal | RX DC cal, T = transmit | RX cal | TX cal, TXC = TX cal, RXC = RX cal.
const F_A: u16 = 0;
const F_RX: u16 = 1;
const F_R: u16 = 2;
const F_T: u16 = 3;
const F_TXC: u16 = 4;
const F_RXC: u16 = 5;
const fn en(off: u16, bit: u16, f: u16) -> u16 {
    off / 4 << 8 | bit << 3 | f
}
static EN_TAB: [u16; 29] = [
    en(0x00, 23, F_A), en(0x04, 15, F_RX), en(0x0C, 4, F_R),
    en(0x0C, 5, F_R), en(0x0C, 20, F_R), en(0x10, 12, F_A),
    en(0x10, 27, F_A), en(0x14, 11, F_A), en(0x14, 18, F_A),
    en(0x18, 0, F_A), en(0x18, 1, F_A), en(0x14, 26, F_A),
    en(0x08, 6, F_T), en(0x18, 19, F_T), en(0x1C, 0, F_T),
    en(0x1C, 4, F_T), en(0x20, 9, F_T), en(0x24, 5, F_T),
    en(0x24, 7, F_T), en(0x24, 8, F_T), en(0x24, 27, F_T),
    en(0x28, 12, F_T), en(0x28, 26, F_T), en(0x38, 26, F_R),
    en(0x38, 27, F_T), en(0x10, 13, F_TXC), en(0x2C, 14, F_TXC),
    en(0x2C, 11, F_RXC), en(0x08, 29, F_RXC),
];

/// mode: 0 receive (and any other value), 1 transmit, 2 TX cal, 3 RX cal, 4 RX DC cal, 5 none (synthesizer only).
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn en_set(mode: u32) {
    let (rx, tx, txc, rxc, rxdc) =
        (u32::from(mode == 0 || mode > 5), u32::from(mode == 1), u32::from(mode == 2), u32::from(mode == 3), u32::from(mode == 4));
    let mut f = [0u32; 6];
    f[usize::from(F_A)] = rxc | rx | txc | rxdc;
    f[usize::from(F_RX)] = rx;
    f[usize::from(F_R)] = rxc | rx | rxdc;
    f[usize::from(F_T)] = rxc | tx | txc;
    f[usize::from(F_TXC)] = txc;
    f[usize::from(F_RXC)] = rxc;
    for &e in &EN_TAB {
        let e = u32::from(e);
        bit(0x11900 + 4 * (e >> 8), e >> 3 & 31, f[(e & 7) as usize]);
    }
}

/// rx_flt_vga_gain (wf_rf_init.c): the RX filter / VGA gain steps, 4 bytes each: byte 0 -> 0x11910 [24:22], byte 1 ->
/// 0x11910 [26:25], byte 2 -> 0x11914 [17:15] (bits 0-2) and [14:12] (bits 4-6), byte 3 -> 0x11914 [7:5].
static VGA: [[u8; 4]; 42] = [
    [1, 0, 0x10, 0], [1, 0, 0x11, 0], [1, 0, 0x12, 0], [1, 0, 0x13, 0], [1, 0, 0x14, 0], [1, 0, 0x15, 0],
    [1, 0, 0x20, 1], [1, 0, 0x21, 1], [1, 0, 0x22, 1], [1, 0, 0x23, 1], [1, 0, 0x24, 1], [1, 0, 0x25, 1],
    [1, 1, 0x20, 2], [1, 1, 0x21, 2], [1, 1, 0x22, 2], [1, 1, 0x23, 2], [1, 1, 0x24, 2], [1, 1, 0x25, 2],
    [2, 1, 0x20, 3], [2, 1, 0x21, 3], [2, 1, 0x22, 3], [2, 1, 0x23, 3], [2, 1, 0x24, 3], [2, 1, 0x25, 3],
    [2, 1, 0x40, 4], [2, 1, 0x41, 4], [2, 1, 0x42, 4], [2, 1, 0x43, 4], [2, 1, 0x44, 4], [2, 1, 0x45, 4],
    [2, 2, 0x40, 5], [2, 2, 0x41, 5], [2, 2, 0x42, 5], [2, 2, 0x43, 5], [2, 2, 0x44, 5], [2, 2, 0x45, 5],
    [4, 2, 0x40, 6], [4, 2, 0x41, 6], [4, 2, 0x42, 6], [4, 2, 0x43, 6], [4, 2, 0x44, 6], [4, 2, 0x45, 6],
];

/// wfrf_flt_vga_gain: one bit or field at a time, as the library writes them.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn vga_gain(g: u32) {
    let v = VGA[(g as usize).min(41)].map(u32::from);
    bit(0x11910, 22, v[0]);
    bit(0x11910, 23, v[0] >> 1);
    bit(0x11910, 24, v[0] >> 2);
    bit(0x11910, 25, v[1]);
    bit(0x11910, 26, v[1] >> 1);
    rmw(0x11914, 7 << 15, v[2] << 15);
    bit(0x11914, 12, v[2] >> 4);
    bit(0x11914, 13, v[2] >> 5);
    bit(0x11914, 14, v[2] >> 6);
    rmw(0x11914, 7 << 5, v[3] << 5);
}

/// wfrf_hp_open / _close: 0x11914 [2:0] = 1, bits 3 and 4 set / all cleared.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn hp_open() {
    rmw(0x11914, 7, 1);
    set(0x11914, 1 << 3);
    set(0x11914, 1 << 4);
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn hp_close() {
    clear(0x11914, 7);
    clear(0x11914, 1 << 3);
    clear(0x11914, 1 << 4);
}

/// rx_tx_cal_attenuation (§3.6): the loop-back attenuators; t = 1 for the TX trim, 0 after it.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn cal_att(t: u32) {
    clear(0x11920, 1 << 20);
    set(0x11920, 1 << 21);
    if t == 1 {
        clear(0x11920, 1 << 22);
        set(0x11920, 1 << 23);
    } else {
        set(0x11920, 1 << 22);
        clear(0x11920, 1 << 23);
    }
    rmw(0x11924, 7 << 28, 5 << 28);
    set(0x11928, 3 << 13);
    rmw(0x11928, 3 << 27, (if t == 1 { 3 } else { 1 }) << 27);
    rmw(0x11910, 0xF << 8, 9 << 8);
}

// ---- the PLL and the VCO bank scan (wf_rf_cal.c, §3.2) ----

/// set_pll_fbds
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn fbds(fbds: u32) {
    rmw(0x1193C, 0xFF << 5, fbds << 5);
}

/// set_pll_cap
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn cap(cap: u32) {
    rmw(0x11938, 0x7F << 19, (cap & 0x7F) << 19);
}

/// set_pll_rn_en: restart the loop, then 100 us to settle.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn rn_en() {
    clear(0x11938, 1 << 16);
    clear(0x11938, 1 << 18);
    clear(0x11934, 1 << 24);
    delay_us(5);
    set(0x11938, 1 << 16);
    delay_us(5);
    set(0x11934, 1 << 24);
    set(0x11938, 1 << 18);
    delay_us(100);
}

/// get_pll_vc_res: the control-voltage detector, bit 18 -> 1, bit 17 -> 2.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn vc_res() -> u32 {
    let r = get(1);
    (r >> 18 & 1) | (r >> 16 & 2)
}

/// open_ / close_rf_pll_vc_det
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn vc_det(on: u32) {
    bit(0x11940, 24, on);
}

/// get_wfa_pll_bank: the bank for an fbds code from the scan's table.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn bank(c: &Cal, fbds: u32) -> u32 {
    let f = fbds & 0xFF;
    for i in 1..48 {
        if u32::from(c.limit[i]) > f {
            return u32::from(c.limit[47].wrapping_sub((i - 1) as u8));
        }
    }
    u32::from(c.limit[47].wrapping_sub(47))
}

// ---- the PHY (wf_phy_mac_lib.c, §3.5) ----

/// MAC register 0x1004 (the MAC at 0x30000).
const MAC_1004: u32 = 0x31004;

/// RX DC encoded: the analog RX DC trims in 0x11908, I [15:7] with bit 16 = non-negative, Q [25:17] with bit 26 -> the
/// encoded value: v | 0x200 when v >= 0, else -v (the library does not mask it). set_anl_rxdc_i / _q.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn anl_rxdc(shift: u32, sbit: u32, dc: i32) -> u32 {
    let d = i32::from(dc as i16);
    if d >= 0 {
        rmw(0x11908, 0x1FF << shift, (d as u32) << shift);
        set(0x11908, 1 << sbit);
        return u32::from((d | 0x200) as u16);
    }
    rmw(0x11908, 0x1FF << shift, ((-d) as u32) << shift);
    clear(0x11908, 1 << sbit);
    u32::from((-d) as u16)
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn anl_rxdc_i(dc: i32) -> u32 {
    anl_rxdc(7, 16, dc)
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn anl_rxdc_q(dc: i32) -> u32 {
    anl_rxdc(17, 26, dc)
}

/// txlo_cali_set / txlo_calq_set: the TX LO leakage trims, 0x1191C [14:8] and 0x11920 [6:0].
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn txlo_i(v: u32) {
    rmw(0x1191C, 0x7F << 8, (v & 0x7F) << 8);
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn txlo_q(v: u32) {
    rmw(0x11920, 0x7F, v & 0x7F);
}

const TX_IG: usize = 0;
const TX_IP: usize = 1;
const TX_QG: usize = 2;
const TX_QP: usize = 3;
const TX_IDC: usize = 4;
const TX_QDC: usize = 5;

/// CRC-16/XMODEM (the library's CRC16 over bytes 0-63, §4.1).
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn crc16(p: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in p {
        crc ^= u32::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1 ^ 0x1021) & 0xFFFF } else { (crc << 1) & 0xFFFF };
        }
    }
    crc
}

/// The 187 record (§4.1) from what the trim left: TX values as written to window D (exact: we wrote them), the TX LO,
/// RX DC and filter registers read back as rd_tx_iq_set / rd_rx_iq_set / rd_bw_set do, RX IQ neutral (not measured:
/// Wi-Fi only), rx_dc_tune as measured, then the check.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn record(c: &Cal, x187: &mut [u8; 68]) {
    x187.fill(0);
    for k in 0..6 {
        // Gains unsigned 10 bits, the rest signed.
        let r = c.tx[k] as u32 & 0x3FF;
        put32(&mut x187[4 * k..], if k == TX_IG || k == TX_QG { r } else { sext10(r) as u32 });
    }
    x187[24] = (rd(0x1191C) >> 8 & 0x7F) as u8;
    x187[25] = (rd(0x11920) & 0x7F) as u8;
    // dig_rx_i_gain, _q_gain; phases 0.
    x187[26] = 255;
    x187[27] = 255;
    let r = rd(0x11908);
    let mag = r >> 7 & 0x1FF;
    put32(&mut x187[32..], if r & 1 << 16 != 0 { mag } else { 0u32.wrapping_sub(mag) });
    let mag = r >> 17 & 0x1FF;
    put32(&mut x187[36..], if r & 1 << 26 != 0 { mag } else { 0u32.wrapping_sub(mag) });
    put32(&mut x187[40..], rd(0x1195C));
    put32(&mut x187[44..], rd(0x11960));
    for k in 0..8 {
        let v = c.rx_dc[k >> 1][k & 1];
        x187[48 + 2 * k] = v as u8;
        x187[49 + 2 * k] = (v >> 8) as u8;
    }
    // (0 would read as "no record" to the SDK: 1 in 65536.)
    let r = crc16(&x187[..64]);
    x187[64] = r as u8;
    x187[65] = (r >> 8) as u8;
}

impl Rf {
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn c_wr(&mut self, e: u32, v: u32) {
        self.win_wr(C, e, v)
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn c_rd(&mut self, e: u32) -> u32 {
        self.win_rd(C, e)
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn d_wr(&mut self, e: u32, v: u32) {
        self.win_wr(D, e, v)
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn d_rd(&mut self, e: u32) -> u32 {
        self.win_rd(D, e)
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn dp_wr(&mut self, e: u32, v: u32) {
        self.win_wr(DP, e, v)
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn dp_rd(&mut self, e: u32) -> u32 {
        self.win_rd(DP, e)
    }

    /// get_wfa_pll_midbank: at fbds 73, the first mid (7, 23, ..., 119) whose neighbours mid - 7 and mid + 8 read
    /// otherwise than mid itself -> mid | 15.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn midbank(&mut self) -> u32 {
        fbds(73);
        let mut mid = 7;
        while mid < 120 {
            cap(mid);
            rn_en();
            let m = vc_res();
            cap(mid - 7);
            rn_en();
            let l = vc_res();
            cap(mid + 8);
            rn_en();
            let h = vc_res();
            if h != m || l != m {
                self.rfc.mid = mid as u8;
                self.rfc.mid_found = true;
                return mid | 15;
            }
            mid += 16;
        }
        // (The library: the last result | 15 = 15.)
        self.rfc.mid = 0;
        self.rfc.mid_found = false;
        TOP_FALLBACK
    }

    /// wfa_pll_bank_scan(121) (§3.2 steps 1-5): for each bank from top down, step fbds up from the last "below" (2)
    /// until "above" (1) or past 171; the bank's limit is (the previous bank's "above" + this one's last "below" - 3) /
    /// 2. Then fbds 121 with its bank. The comparator needs no wait (the loop restart has its delays), so nothing here
    /// can time out. -> 0, or E_MIDBANK (the scan still ran, from the fallback top).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn cal_scan(&mut self) -> i32 {
        let t0 = ticks();
        let (mut prev_h, mut lo) = (73u32, 73u32);
        vc_det(1);
        let top = self.midbank();
        self.rfc.top = top as u8;
        self.rfc.limit[47] = top as u8;
        for i in 0..47 {
            cap(top.wrapping_sub(i) & 0xFF);
            let mut f = lo;
            let mut cur_lo = lo;
            let h;
            loop {
                fbds(f);
                rn_en();
                let r = vc_res();
                if r == 1 {
                    h = f;
                    break;
                }
                if r == 2 {
                    cur_lo = f;
                }
                if f + 1 > 171 {
                    // (After a 2 at 171, cur_lo is 171 too.)
                    h = 171;
                    break;
                }
                f += 1;
            }
            self.rfc.limit[i as usize] = ((prev_h + cur_lo - 3) / 2) as u8;
            prev_h = h;
            lo = cur_lo;
        }
        fbds(121);
        self.rfc.bank121 = bank(&self.rfc, 121) as u8;
        cap(u32::from(self.rfc.bank121));
        vc_det(0);
        self.rfc.us_scan = us_since(t0);
        if self.rfc.mid_found { OK } else { E_MIDBANK }
    }

    /// wf_phy_rst
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn phy_rst(&mut self) {
        wr(MAC_1004, 12);
        self.bbp(0x16, 0, 0);
        self.bbp(0x15, 0x31, 0);
        self.bbp(0x15, 0x30, 0);
        self.bbp(0x19, 0, 0);
    }

    /// wf_phy_loop_always_test: TX into RX.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn loop_test(&mut self) {
        self.phy_rst();
        wr(MAC_1004, 12);
        self.bbp(0x19, 0x18, 0);
    }

    /// confg_cca_for_trim
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn cca_for_trim(&mut self) {
        let v = self.c_rd(0x40) & 0x7F;
        self.c_wr(0x40, v);
        let v = self.c_rd(0x41) & 0x7F;
        self.c_wr(0x41, v);
        self.c_wr(0x28, 0);
        self.c_wr(0x27, 5);
        let v = self.bbp(0x04, 0, 1) & !0x18;
        self.bbp(0x04, v, 0);
    }

    /// dem_filter_dc (§4.6): start the estimator, wait for it, read the result for sel: 0, 3: I^2 + Q^2 (16-bit I at
    /// D' 0x81:0x82, Q at 0x84:0x85); 1: the 24-bit I mean (0x81-0x83); 2: the Q mean (0x8A-0x8C); other: I^2 + Q^2
    /// from 0x87:0x88 and 0x8A:0x8B. A timeout sets abort and returns 0.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn dem(&mut self, sel: u32) -> i32 {
        self.rfc.dem_reads += 1;
        self.dp_wr(0x15, 1);
        self.dp_wr(0x16, 1);
        self.dp_wr(0x15, 3);
        self.dp_wr(0x16, 3);
        let t0 = ticks();
        while self.dp_rd(0x8D) & 3 != 0 {
            if ticks().wrapping_sub(t0) > DEM_US * TICKS_PER_US {
                self.rfc.to_dem += 1;
                self.rfc.abort = true;
                return 0;
            }
        }
        if sel == 1 || sel == 2 {
            let a = if sel == 1 { 0x81 } else { 0x8A };
            let hi = self.dp_rd(a);
            let mid = self.dp_rd(a + 1);
            let b = self.dp_rd(a + 2);
            return (hi << 24 | mid << 16 | b << 8) as i32 / 256;
        }
        let a = if sel == 0 || sel == 3 { 0x81 } else { 0x87 };
        let b = if sel == 0 || sel == 3 { 0x84 } else { 0x8A };
        let hi = self.dp_rd(a);
        let i = i32::from((hi << 8 | self.dp_rd(a + 1)) as i16);
        let hi = self.dp_rd(b);
        let q = i32::from((hi << 8 | self.dp_rd(b + 1)) as i16);
        // (As the library: a full-scale pair wraps.)
        ((i * i) as u32).wrapping_add((q * q) as u32) as i32
    }

    /// rd_adc_peak: the ADC peak over 1 ms.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn adc_peak(&mut self) -> u32 {
        self.dp_wr(0x17, 0x82);
        delay_us(1000);
        let p = self.dp_rd(0x80);
        self.dp_wr(0x17, 2);
        p
    }

    /// adc_pp_tune(0): raise the VGA gain step until the peak (the second of two reads) is above 47, at most step 36.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn adc_pp_tune(&mut self) -> u32 {
        let mut g = 0;
        vga_gain(0);
        self.adc_peak();
        let mut p = self.adc_peak();
        while p <= 47 && g <= 35 {
            g += 1;
            vga_gain(g);
            self.adc_peak();
            p = self.adc_peak();
        }
        vga_gain(g);
        g
    }

    /// set_dig_tx_i_gain .. set_dig_tx_q_dc: k = 0 I gain, 1 I phase, 2 Q gain, 3 Q phase, 4 I DC, 5 Q DC. The 10-bit
    /// value's [9:2] into D 0x04 + k, its [1:0] into a pair of D 0x61 (k 0-3) or 0x62 (k 4-5), read-modify-write.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn tx_set(&mut self, k: usize, v: i32) {
        let (e, s, u) = (if k < 4 { 0x61 } else { 0x62 }, 2 * (k as u32 & 3), v as u32);
        self.d_wr(0x04 + k as u32, u >> 2 & 0xFF);
        let r = self.d_rd(e);
        self.d_wr(e, (r & !(3 << s)) | (u & 3) << s);
        self.rfc.tx[k] = v;
    }

    // ---- the trim (§4) ----

    /// wf_rx_tune_test (§4.1b): the RC tuning engine three times (6, 10, 20 MHz), the codes into the filter registers.
    /// A run that does not finish stops the step before any filter register is written.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn rx_tune(&mut self) -> i32 {
        const CODE: [u32; 3] = [0x1FC, 0x122, 0x0A0];
        rmw(0x11904, 0xF << 8, 8 << 8);
        set(0x11950, 1 << 27);
        for (k, &code) in CODE.iter().enumerate() {
            rmw(0x11968, 0x3FE, code);
            rmw(0x11968, 0x7F << 11, 15 << 11);
            clear(0x11968, 0x1F << 18);
            clear(0x11968, 1 << 23);
            // Start.
            set(0x11968, 1);
            let t0 = ticks();
            while get(1) & 0x20 == 0 {
                if ticks().wrapping_sub(t0) > TUNE_US * TICKS_PER_US {
                    self.rfc.to_filter += 1;
                    clear(0x11968, 1);
                    clear(0x11950, 1 << 27);
                    return E_FILTER;
                }
            }
            self.rfc.flt[k] = (get(1) >> 8) as u8;
            clear(0x11968, 1);
        }
        let (r6, r10, r20) = (u32::from(self.rfc.flt[0]), u32::from(self.rfc.flt[1]), u32::from(self.rfc.flt[2]));
        clear(0x11950, 1 << 27);
        rmw(0x11920, 0xFF << 10, r10 << 10);
        rmw(0x11910, 0xFF << 14, r10 << 14);
        set(0x1195C, 0xFF);
        rmw(0x1195C, 0xFF << 8, r6 << 8);
        rmw(0x1195C, 0xFF << 16, r10 << 16);
        rmw(0x1195C, 0xFF << 24, r20 << 24);
        set(0x11960, 0xFF);
        rmw(0x11960, 0xFF << 8, r6 << 8);
        rmw(0x11960, 0xFF << 16, r10 << 16);
        rmw(0x11960, 0xFF << 24, r20 << 24);
        OK
    }

    /// The TX LO search (§4.2 step 3): best of 0..127 for one trim, starting from 64 at metric(0); only a strictly
    /// smaller metric wins -> the best, or -1 on an estimator timeout.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn txlo_search(&mut self, put: fn(u32)) -> i32 {
        let mut b = 64;
        put(64);
        let mut best = self.dem(0);
        let mut i = 0;
        while i < 128 && !self.rfc.abort {
            put(i);
            let m = self.dem(0);
            if m < best {
                best = m;
                b = i;
            }
            i += 1;
        }
        if self.rfc.abort {
            return -1;
        }
        put(b);
        b as i32
    }

    /// Gains 1023, phases and DCs 0.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn tx_neutral(&mut self) {
        self.tx_set(TX_IG, 1023);
        self.tx_set(TX_QG, 1023);
        self.tx_set(TX_IP, 0);
        self.tx_set(TX_QP, 0);
        self.tx_set(TX_IDC, 0);
        self.tx_set(TX_QDC, 0);
    }

    /// txiq_tune_soft (§4.2): the loop-back set-up, then the TX LO search (steps 1-3). (FM1_RF_CAL_FULL's TX DC, gain
    /// balance and phase, steps 4-6, aren't built.)
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn txiq_tune(&mut self) -> i32 {
        let t0 = ticks();
        self.phy_rst();
        self.c_wr(0x15, 0);
        self.c_wr(0x14, 0);
        self.cca_for_trim();
        self.loop_test();
        // A test tone [M].
        self.d_wr(0x00, 3);
        self.d_wr(0x5F, 16);
        self.d_wr(0x01, 20);
        self.d_wr(0x02, 0);
        self.d_wr(0x03, 0);
        self.dp_wr(0x00, 62);
        self.dp_wr(0x13, 30);
        self.dp_wr(0x14, 56);
        self.dp_wr(0x14, 52);
        self.tx_neutral();
        txlo_i(64);
        txlo_q(64);
        self.adc_pp_tune();
        self.rfc.abort = false;
        let mut r = self.txlo_search(txlo_i);
        if r >= 0 {
            self.rfc.txlo_i = r as u8;
            r = self.txlo_search(txlo_q);
            self.rfc.txlo_q = r as u8;
        }
        self.rfc.us_txlo = us_since(t0);
        if r < 0 {
            txlo_i(64);
            txlo_q(64);
            self.rfc.txlo_i = 64;
            self.rfc.txlo_q = 64;
            return E_TXLO;
        }
        OK
    }

    /// rxdc_tune_soft (§4.3): for each of four front-end gain steps, a signed successive approximation of the analog
    /// RX DC trims against the I and Q means: twelve rounds, step 256 halving down to 1; the guard is checked before
    /// the step, so a trim stays within +-510. The last step's trims stay in 0x11908.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn rxdc_tune(&mut self) -> i32 {
        // 0x1190C [7:6], 0x11910 [11:8], 0x11910 [6:0]
        const GS: [[u32; 3]; 4] = [[0, 0, 127], [1, 0, 127], [1, 3, 69], [1, 9, 14]];
        let t0 = ticks();
        self.phy_rst();
        self.c_wr(0x15, 0);
        self.c_wr(0x14, 0);
        self.cca_for_trim();
        self.loop_test();
        self.d_wr(0x00, 3);
        self.d_wr(0x5F, 16);
        self.dp_wr(0x00, 62);
        self.dp_wr(0x1B, 56);
        self.dp_wr(0x08, 0xC0);
        self.dp_wr(0x13, 0);
        self.dp_wr(0x14, 56);
        self.dp_wr(0x14, 52);
        vga_gain(40);
        self.rfc.abort = false;
        let mut g = 0;
        while g < 4 && !self.rfc.abort {
            rmw(0x1190C, 3 << 6, GS[g][0] << 6);
            rmw(0x11910, 0xF << 8, GS[g][1] << 8);
            rmw(0x11910, 0x7F, GS[g][2]);
            anl_rxdc_i(0);
            anl_rxdc_q(0);
            let (mut bi, mut bq, mut s) = (0i32, 0i32, 256i32);
            let mut j = 0;
            while j < 12 && !self.rfc.abort {
                let m = self.dem(1);
                if self.rfc.abort {
                    break;
                }
                if m > 0 && bi > -510 {
                    bi -= s;
                } else if m < 0 && bi < 510 {
                    bi += s;
                }
                anl_rxdc_i(bi);
                let m = self.dem(2);
                if m > 0 && bq > -510 {
                    bq -= s;
                } else if m < 0 && bq < 510 {
                    bq += s;
                }
                anl_rxdc_q(bq);
                s = if s > 1 { s >> 1 } else { 1 };
                j += 1;
            }
            if self.rfc.abort {
                break;
            }
            self.rfc.rx_dc[g][0] = anl_rxdc_i(bi) as u16;
            self.rfc.rx_dc[g][1] = anl_rxdc_q(bq) as u16;
            g += 1;
        }
        self.rfc.us_rxdc = us_since(t0);
        if self.rfc.abort {
            anl_rxdc_i(0);
            anl_rxdc_q(0);
            for dc in &mut self.rfc.rx_dc[g..] {
                *dc = [0x200, 0x200];
            }
            return E_RXDC;
        }
        OK
    }

    /// wf_rf_trim without RX IQ (§4, the SDK's order): the RX filter, TX cal mode and attenuation, TX LO, RX DC in
    /// receive mode with bit 15 of 0x11904 cleared, then RX cal mode and attenuation as the library leaves them
    /// (wf_rf_init's wf_rf_en_set(0) after the trim is the replay's). Then x187 is filled. A step that times out puts
    /// back its neutral values and the rest still runs. -> 0, or the first step's E_*.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn cal_trim(&mut self) -> i32 {
        let (t0, bbp0) = (ticks(), self.stat.bbp_timeouts);
        let c = &mut self.rfc;
        c.err = OK;
        c.dem_reads = 0;
        c.abort = false;
        c.txlo_i = 64;
        c.txlo_q = 64;
        c.rx_dc = [[0x200; 2]; 4];
        c.tx[TX_IG] = 1023;
        c.tx[TX_QG] = 1023;
        c.tx[TX_IP] = 0;
        c.tx[TX_QP] = 0;
        c.tx[TX_IDC] = 0;
        c.tx[TX_QDC] = 0;
        c.us_txiq = 0;

        let t = ticks();
        let r = self.rx_tune();
        self.rfc.us_filter = us_since(t);
        if r != OK && self.rfc.err == OK {
            self.rfc.err = r;
        }
        en_set(2);
        cal_att(1);
        let r = self.txiq_tune();
        if r != OK && self.rfc.err == OK {
            self.rfc.err = r;
        }
        hp_close();
        en_set(0);
        clear(0x11904, 1 << 15);
        let r = self.rxdc_tune();
        if r != OK && self.rfc.err == OK {
            self.rfc.err = r;
        }
        hp_open();
        en_set(3);
        cal_att(0);

        record(&self.rfc, &mut self.x187);
        self.rfc.bbp_timeouts = self.stat.bbp_timeouts.wrapping_sub(bbp0);
        self.rfc.us_total = us_since(t0);
        self.rfc.err
    }
}

// ---- the apply side: the replay's writes with the record's fields (quick_start_wifi_trim, §4.5) ----

/// The 0x11908 field pair as set_anl_rxdc_i / _q put a signed value in.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn anl_field(v: u32, shift: u32, sbit: u32, dc: i32) -> u32 {
    let d = i32::from(dc as i16);
    let v = v & !(0x1FF << shift);
    if d >= 0 {
        return v | (d as u32 & 0x1FF) << shift | 1 << sbit;
    }
    (v | ((-d) as u32 & 0x1FF) << shift) & !(1 << sbit)
}

/// A register write of the replay, with the record's fields at their true widths: 0x1191C [14:8] txlo_cali, 0x11920
/// [6:0] txlo_calq and [17:10] flt_bw_1 byte 2, 0x11910 [21:14] flt_bw_1 byte 2, 0x11908 anl_rxdc_i ([15:7], bit 16)
/// and anl_rxdc_q ([25:17], bit 26), 0x1195C / 0x11960 the whole flt_bw_1 / _2. Other addresses: v unchanged. (The
/// quick start also sets 0x11904 [11:8] = 8, a constant the replay carries.)
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn cal_reg(addr: u32, v: u32, x187: &[u8; 68]) -> u32 {
    let bw2 = u32::from(x187[42]);
    match addr {
        0x1191C => (v & !(0x7F << 8)) | u32::from(x187[24] & 0x7F) << 8,
        0x11920 => (v & !(0x7F | 0xFF << 10)) | u32::from(x187[25] & 0x7F) | bw2 << 10,
        0x11910 => (v & !(0xFF << 14)) | bw2 << 14,
        0x11908 => {
            let v = anl_field(v, 7, 16, u32le(&x187[32..]) as i32);
            anl_field(v, 17, 26, u32le(&x187[36..]) as i32)
        }
        0x1195C => u32le(&x187[40..]),
        0x11960 => u32le(&x187[44..]),
        _ => v,
    }
}

/// A window write of the replay, the same: D (w 2) entries 0x04-0x09 = the six TX values >> 2, 0x61 = their low bit
/// pairs (I gain [1:0], I phase [3:2], Q gain [5:4], Q phase [7:6]), 0x62 [1:0] / [3:2] = the DCs' (bits 7-4 of v
/// kept); D' (w 3) 0x09 / 0x0B = RX I / Q gain, 0x0A / 0x0C = RX I / Q phase. Others: v unchanged.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn cal_win(w: u32, entry: u32, v: u32, x187: &[u8; 68]) -> u32 {
    if w == D {
        if (0x04..=0x09).contains(&entry) {
            return u32le(&x187[4 * (entry as usize - 4)..]) >> 2 & 0xFF;
        }
        if entry == 0x61 {
            let mut v = 0;
            for k in 0..4 {
                v |= (u32le(&x187[4 * k..]) & 3) << (2 * k);
            }
            return v;
        }
        if entry == 0x62 {
            return (v & !0xF) | (u32le(&x187[16..]) & 3) | (u32le(&x187[20..]) & 3) << 2;
        }
    } else if w == DP {
        match entry {
            0x09 => return u32::from(x187[26]),
            0x0B => return u32::from(x187[27]),
            0x0A => return u32::from(x187[28]),
            0x0C => return u32::from(x187[29]),
            _ => {}
        }
    }
    v
}

/// The RF-die table (wl_rfd_actl_ram_init, §6): word 0 of entries 128-223 keeps [11:0] and takes rx_dc_tune[g]: I in
/// [21:12], Q << 22; g = 0 for 128-132, 1 for 133-179, 2 for 180-185, 3 for 186-223. Others: w0 unchanged.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn rfd_w0(entry: u32, w0: u32, x187: &[u8; 68]) -> u32 {
    if !(128..=223).contains(&entry) {
        return w0;
    }
    let g = if entry < 133 {
        0
    } else if entry < 180 {
        1
    } else if entry < 186 {
        2
    } else {
        3
    };
    let i = u32::from(x187[48 + 4 * g]) | u32::from(x187[49 + 4 * g]) << 8;
    let q = u32::from(x187[50 + 4 * g]) | u32::from(x187[51 + 4 * g]) << 8;
    (w0 & 0xFFF) | (i & 0x3FF) << 12 | q << 22
}

/// Word 1 of entries 0-127: the bank of the entry's fbds code (word 1 [28:21]) << 9. Others: w1 unchanged.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn rfd_w1(c: &Cal, entry: u32, w1: u32) -> u32 {
    if entry >= 128 {
        return w1;
    }
    // (The SDK's table has [15:9] zero and ORs the bank in; the replay's words carry the emulator's bank there:
    // replaced, not ORed.)
    (w1 & !(0x7F << 9)) | bank(c, w1 >> 21 & 0xFF) << 9
}

// ---- the BT TX trims (keep_tx_iq_set and analog.c's update_mdm_txiq_cmp, §5) ----

/// From a 187 record: CMPUK1 / 2 = I / Q gain >> 3, CMPUK3 = (I phase + Q phase) / 8, OFSI / OFSQ = I / Q DC / 4
/// (divisions toward zero). Read-modify-write of the caller's values (bt_rf_analog_init's base: 0x2FC08 = 0x03100000,
/// 0x2FC10 = 0x80008080): 0x2FC10 [7:0] CMPUK1, [15:8] CMPUK2, [23:16] -CMPUK3; 0x2FC08 [9:0] OFSI, [19:10] OFSQ. Our
/// unit (` BT_TX_IQ: 127 123 -3`, ` BT_OF: 0 -1`): 0x80037B7F, 0x031FFC00.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn bt_trims(x187: &[u8; 68], w2fc08: &mut u32, w2fc10: &mut u32) {
    let k1 = u32le(&x187[0..]) >> 3 & 0xFF;
    let k2 = u32le(&x187[8..]) >> 3 & 0xFF;
    let k3 = (u32le(&x187[4..]) as i32).wrapping_add(u32le(&x187[12..]) as i32) / 8;
    let ofsi = u32le(&x187[16..]) as i32 / 4;
    let ofsq = u32le(&x187[20..]) as i32 / 4;
    let mut v = *w2fc10;
    v = (v & !0xFF) | k1;
    v = (v & !0xFF00) | k2 << 8;
    v = (v & !0xFF_0000) | (0u32.wrapping_sub(k3 as u32 & 0xFF) & 0xFF) << 16;
    *w2fc10 = v;
    let mut v = *w2fc08;
    v = (v & !0x3FF) | (ofsi as u32 & 0x3FF);
    v = (v & !(0x3FF << 10)) | (ofsq as u32 & 0x3FF) << 10;
    *w2fc08 = v;
}
