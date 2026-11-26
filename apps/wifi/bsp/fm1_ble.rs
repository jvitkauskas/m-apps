// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
//! FM-1 (AC791N / WL82) radio bring-up, which the Wi-Fi side needs first, written from the clean-room fact sheet
//! docs/BLE-HW-FACTS.md (branch feat/ble-facts, 41b7374; §14-§17 from d907ce3): every register access names its section
//! there as "HW §n". m-apps keeps only what Wi-Fi uses of felucca-bsp's file: the BLE baseband, its link port and its
//! interrupts are left out (apps/wifi/bsp/README.md).
//!
//! - [`rf_init`]: the radio in stock's second-boot order (HW §16): the captured Wi-Fi front end with the stored trims
//!   (fm1_ble_rf.rs), clocks and power, the BT analog block (HW §5.1, §5.4); what stays TODO(hardware) is listed in
//!   fm1_ble_rf.rs
//! - [`rng32`]: the random-number source (HW §1 0x13B00/04)
//!
//! Taken here under MPL-2.0. Changed by m-apps: in Rust.
use super::ble_rf_tables::AGC;
use super::fm1_ble_rf::{Records, Rf, SECT_BT, SECT_DONE};
use super::fm1_cc::{clear, rd, set, wr};
use super::fm1_rf_cal::bt_trims;
use super::fm1_time::delay_us;

// ---- BT analog / configuration block (HW §2.2)
const BT_C00: u32 = 0x2FC00;
const BT_C04: u32 = 0x2FC04;
/// [9:0], [19:10] stored I / Q trims.
const BT_C08: u32 = 0x2FC08;
const BT_C0C: u32 = 0x2FC0C;
/// Bytes 0..2 stored trims.
const BT_C10: u32 = 0x2FC10;
const BT_C14: u32 = 0x2FC14;
const BT_C18: u32 = 0x2FC18;
const BT_C1C: u32 = 0x2FC1C;
const BT_C20: u32 = 0x2FC20;
const BT_C24: u32 = 0x2FC24;
const BT_C28: u32 = 0x2FC28;
/// BT block enable / config [I].
const BT_C40: u32 = 0x2FC40;
const BT_C48: u32 = 0x2FC48;
const BT_C78: u32 = 0x2FC78;
const BT_C7C: u32 = 0x2FC7C;
/// The PLL channel table (an SRAM address).
const BT_PLL_TBL: u32 = 0x2FC88;
const BT_C98: u32 = 0x2FC98;
const BT_C9C: u32 = 0x2FC9C;
const BT_CA0: u32 = 0x2FCA0;
const BT_AGC0: u32 = 0x2FD80;
/// 0x2FD84 .. 0x2FD94
const fn bt_agc(i: u32) -> u32 {
    0x2FD84 + 4 * i
}
/// BR/EDR baseband control (HW §5.1 step 4).
const BT_BRCON: u32 = 0x20000;
/// BT / RF enable (HW §5.1 step 3).
const WL_BTEN: u32 = 0x14000;
/// Bits 14-15: BT domain power / reset (HW §5.1).
const CLK_CON1_BT: u32 = 0x10010;
/// Random source (HW §1).
const RNG_LO: u32 = 0x13B00;
const RNG_HI: u32 = 0x13B04;
/// HW §2.2: the AGC table's write index (0).
const BT_AGC_IDX: u32 = 0x2FD98;
/// HW §2.2: its data port, 128 words.
const BT_AGC_DATA: u32 = 0x2FD9C;

/// Values the fact sheet does not settle, named so nobody mistakes them for measurements. HW §5.1 step 3: "a
/// 240-unit delay" and "delays between steps", unit and length unknown [M:s]; 100 us is a guess, generous for a power
/// switch.
const STEP_DELAY_US: u32 = 100;

/// HW §1
#[inline(always)]
pub fn rng32() -> u32 {
    rd(RNG_LO) ^ rd(RNG_HI).wrapping_mul(0x9E37_79B9)
}

/// The radio in stock's second-boot order (HW §16.1), with the stored trims (the data of VM 106, 107, 108, 187:
/// src/ble/ble_vm.c found them in the VM or in Optimist's copy; without them the caller never gets here, §15.4):
/// - HW §16.1 groups 2-13: the Wi-Fi front end the BT RF init runs first (HW §5.2), from the captured program
///   (fm1_ble_rf.rs, ble_rf_tables.rs), the trim fields byte by byte (§16.4), our own VCO scan (group 7);
/// - HW §5.1 steps 3, 4: the BT domain;
/// - HW §5.4 / §16.1 g. 14: the PLL channel table, the AGC table (captured) and configuration, the BT analog words.
///
/// The clock words of HW §16.1 group 1 (0x10010 bit 10 = felucca's UART's clock; 0x10008 bit 3 = the second core's
/// start) are not the radio's: left as the firmware set them.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn rf_init(rf: &mut Rf, rec: &Records) {
    // HW §16.1 groups 2-13
    rf.run(rec);
    // HW §16.1 group 14
    rf.section(SECT_BT);
    // HW §5.1 step 3: BT domain power / reset, then the BT / RF enable. Bits 14-15 cleared [M:s].
    clear(CLK_CON1_BT, 3 << 14);
    delay_us(STEP_DELAY_US);
    // -> 0x000C0081 in three steps [M:t]
    set(WL_BTEN, 1 << 7);
    delay_us(STEP_DELAY_US);
    wr(WL_BTEN, (rd(WL_BTEN) & !(0x1F << 16)) | 0x0C << 16);
    delay_us(STEP_DELAY_US);
    set(WL_BTEN, 1);
    delay_us(STEP_DELAY_US);
    // HW §5.1 step 4 [M:t]
    set(BT_C40, 1 << 10);
    wr(BT_BRCON, 0xC000_0000);
    wr(BT_C40, 0xFFFF);
    wr(BT_C40, 0xFCFD);
    wr(BT_C78, 0x0F);
    wr(BT_C78, 0x3FF);
    // (VM 110, the carrier offset -> PLL_COMP: V15 writes no 110 and a hand-made one changed nothing in the emulator,
    // HW §14.5; U14)
    // HW §5.4: the PLL channel table, then the AGC.
    for (i, e) in rf.pll_tbl.chunks_exact_mut(3).enumerate() {
        let i = i as u32;
        e.copy_from_slice(&[i | i << 8, 0, 0]);
    }
    wr(BT_PLL_TBL, kit::hw::dma_address(rf.pll_tbl.as_mut_ptr().cast()));
    // HW §2.2, §5.4: the AGC table (captured, §17).
    wr(BT_AGC_IDX, 0);
    for &v in &AGC {
        wr(BT_AGC_DATA, v);
    }
    // HW §5.4 AGC config [M:t]
    wr(BT_C20, 0);
    wr(BT_AGC0, 0x000F_0000);
    wr(bt_agc(0), 0x1872_BF14);
    wr(bt_agc(1), 0x1872_BF55);
    wr(bt_agc(2), 0x1872_BF00);
    wr(bt_agc(3), 0);
    wr(bt_agc(4), 0);
    wr(BT_AGC0, 0x000F_0001);
    set(BT_C7C, 1);
    // HW §2.2 (0xAD after the BR/EDR init, not done here)
    wr(BT_C48, 0xB9);
    // HW §2.2, §5.4: 0x2FC00 - 0x2FC28 [M:t]
    wr(BT_C00, 0x00FF_D144);
    wr(BT_C04, 0x4E14_3CDF);
    // The BT TX IQ / DC trims: bt_rf_analog_init's base values, with our calibration the compensation from the measured
    // record (fm1_rf_cal.rs, RF-CALIBRATION.md §5).
    let (mut c08, mut c10) = (0x0310_0000, 0x8000_8080);
    if rf.cal_on && rf.trimmed {
        bt_trims(&rf.x187, &mut c08, &mut c10);
    }
    wr(BT_C08, c08);
    wr(BT_C0C, 0x8080_8080);
    wr(BT_C10, c10);
    wr(BT_C14, 0x80);
    wr(BT_C18, 0);
    wr(BT_C1C, 0);
    wr(BT_C20, 0x3655_5537);
    wr(BT_C24, 0x0003_4041);
    wr(BT_C28, 0x0005_C000);
    // HW §2.2: constants [M:s]
    wr(BT_C98, 0x0236_0236);
    wr(BT_C9C, 0x3636_0008);
    wr(BT_CA0, 0x0000_3636);
    // HW §5.4 then initialises the BR/EDR baseband (0x2FC44, 0x2000C ... 0x20168) and the slot timer. Not done: a
    // BLE-only link layer probably needs neither (HW §10), but whether the engine needs the BT clock running is U12.
    rf.section(SECT_DONE);
}
