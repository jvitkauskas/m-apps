// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
// Copyright (C) 2026 Henri David (hdavid), Optimist (github.com/w0ts/optimist)
//! FM-1 (AC791N / WL82) Wi-Fi MAC: JieLi's "wl30" core (MAC_CSR0 reads 0x00300100 on the FM-1) with Ralink's RT3070
//! register layout. MAC register r (Ralink's numbering: MAC_CSR0 0x1000, BBP_CSR_CFG 0x101C ...) is plain MMIO at
//! 0x30000 + r, as the JieLi AC79 SDK reaches it (wl_sdio.c wl30_mmc_io_rw_extended, Apache-2.0). Frames go by DMA
//! (0x30F00-0x30F1C), the Wi-Fi clock / reset block is at 0x14040-0x1405C. The register addresses and values are facts
//! read from JieLi's SDK (Apache-2.0: NOTICE), the code around them is Optimist's (docs/WIFI-HW-FACTS.md there: every
//! measurement). Used by fm1-base's own Wi-Fi stack (wifi_stack = "own": wifi/, device/wifi_own.c).
//!
//! The Wi-Fi side needs the radio started: the Bluetooth start-up (fm1_ble.rs `rf_init`, captured from stock V15)
//! already is JieLi's Wi-Fi radio quick start (33,357 of its 34,020 writes match), so Wi-Fi comes up on a radio BT has
//! started, with three more steps ([`up`]): the station's MAC settings, both RF paths on (0x14000 |= 3) and the
//! coexistence controller's tables (fm1_wifi_wlc.rs, 565 words to 0x14004), which made reception work.
//!
//! - [`up`]: the Wi-Fi side on (the radio started by BT first)
//! - [`set_channel`]: 1..13 (byte 0 of 0x1405C = channel + 101)
//! - [`rx`]: one received frame by the RX DMA: RXWI (16 B) then the 802.11 frame
//! - [`tx_prepare_rate`] / [`tx`]: one frame out by the TX DMA (a TXWI before it, at a rate: RATE_*)
//! - [`mac_rd`] / [`mac_wr`]: a MAC register; the key tables' places below (WCID_* ...)
//!
//! Taken here under MPL-2.0. Changed by m-apps: in Rust.
use super::fm1_cc::{rd, set, wr};
use super::fm1_time::{T4_CNT, TICKS_PER_US};
use super::fm1_wifi_wlc as wlc;
use core::sync::atomic::{Ordering, compiler_fence};
use kit::hw::{dma_address, sync_cpu};

/// MAC register r at MAC + r.
pub const MAC: u32 = 0x30000;
/// The frame DMA's status: bit 17 a frame waits, 20 TX done, 21 RX done.
const WDMA1: u32 = 0x30F04;
const RX_PEND: u32 = 1 << 17;
const RX_DONE: u32 = 1 << 21;
/// Written before / after each RX transfer.
const RX_CLR: u32 = 1 << 11;
/// RX DMA: the buffer ..
const RX_ADDR: u32 = 0x30F18;
/// .. and the byte count.
const RX_LEN: u32 = 0x30F1C;
/// MAC register written 2 before a frame is fetched (sdio_recv_int_hdl).
const RX_ACK: u32 = 0x324;
/// The SDK takes a length below 4089.
pub const RX_MAX: u32 = 4088;
const TX_ADDR: u32 = 0x30F10;
const TX_LEN: u32 = 0x30F14;
/// Written before / after each TX transfer.
const TX_CLR: u32 = 1 << 10;
const TX_DONE: u32 = 1 << 20;
/// Bytes before the 802.11 frame in a TX buffer: length, queue, TXWI.
pub const TX_HDR: usize = 24;
/// Bytes before the 802.11 frame in a received one.
pub const RXWI: usize = 16;

/// The frame DMA reads main RAM: a TX buffer placed 128 KiB or more above the SRAM base (0x01C20000..) went out as
/// garbage (the DMA address likely wraps there), so the buffers it moves must sit below DMA_TOP (measured on the FM-1:
/// below works, above did not; why is not proven). The kit's .dma region is there (kit/app.ld.S).
#[allow(dead_code)]
pub const DMA_TOP: u32 = 0x01C2_0000;

/// A buffer the frame DMA moves, in whole words.
#[repr(C, align(4))]
pub struct Buf<const N: usize>(pub [u8; N]);

impl<const N: usize> Buf<N> {
    pub const fn new() -> Buf<N> {
        Buf([0; N])
    }

    fn word(&self, i: usize) -> u32 {
        u32::from_ne_bytes([self.0[4 * i], self.0[4 * i + 1], self.0[4 * i + 2], self.0[4 * i + 3]])
    }

    fn set_word(&mut self, i: usize, v: u32) {
        self.0[4 * i..4 * i + 4].copy_from_slice(&v.to_ne_bytes());
    }

    fn address(&mut self) -> u32 {
        dma_address(self.0.as_mut_ptr())
    }
}

/// One frame out by the TX DMA, as the SDK's usb_submit_urb / wl30_mmc_io_rw_extended (addr 0) hand it over. buf: word
/// 0 = MPDU length + 22; word 1 = 4 (queue 2); a 16-byte Ralink TXWI (word 0 the rate: 0xB0, 1 Mb/s CCK; word 1 ACK |
/// BA window 13 << 2 | WCID << 8 | MPDU length << 16); then the 802.11 frame. A frame with the Protected bit set is
/// encrypted by the MAC with the WCID's key (CCMP; the IV / EIV from the WCID's table); a frame at a WCID with no key
/// goes out as it is. ack 1: the MAC waits for the receiver's ACK and retries (0 for beacons and group frames). WCID 0
/// is the MAC's drop entry: never used.
#[inline(always)]
pub fn tx_prepare_rate<const N: usize>(buf: &mut Buf<N>, mpdu_len: u32, wcid: u32, ack: u32, w0: u32) {
    buf.set_word(0, mpdu_len + 22);
    buf.set_word(1, 4);
    buf.set_word(2, w0);
    buf.set_word(3, (ack & 1) | 13 << 2 | (wcid & 0xFF) << 8 | (mpdu_len & 0xFFF) << 16);
    buf.set_word(4, 0);
    buf.set_word(5, 0);
}

/// TXWI word 0 (Ralink's _TXWI_STRUC, the SDK's debug info in wl_wifi_sta.a cmm_data.c: MCS [22:16], PHYMODE [31:30], 0
/// CCK, 1 OFDM; bits 4-7 the SDK's own 0xB0, kept): the CCK MCS 0..3 = 1, 2, 5.5, 11 Mb/s (long preamble), the OFDM
/// MCS 0..7 = 6 .. 54 Mb/s. JieLi's rate table (wl_usb_to_sdio.c mcs_mode_tab, wifi_set_tx_rate_control_tab) runs CCK
/// 1M, CCK 2M, CCK 5.5M, OFDM 6M, .. in that order.
pub const fn txwi(phy: u32, mcs: u32) -> u32 {
    0xB0 | (mcs & 0x7F) << 16 | (phy & 3) << 30
}
/// 0xB0: every frame until fm1-base's Wi-Fi tuning.
pub const RATE_CCK1: u32 = txwi(0, 0);

/// The frame tx_prepare_rate set up, out; 0, or -1 if the DMA did not finish within 10 ms (one frame at a time).
#[inline(always)]
pub fn tx<const N: usize>(buf: &mut Buf<N>) -> i32 {
    // The DMA reads the length word's bytes on, at most this buffer.
    assert!(buf.word(0) as usize + 8 <= N);
    set(WDMA1, TX_CLR);
    sync_cpu();
    wr(TX_ADDR, buf.address() + 4);
    wr(TX_LEN, buf.word(0) + 4);
    sync_cpu();
    let t0 = T4_CNT.read();
    while rd(WDMA1) & TX_DONE == 0 {
        if T4_CNT.read().wrapping_sub(t0) > 10000 * TICKS_PER_US {
            return -1;
        }
    }
    set(WDMA1, TX_CLR);
    0
}

/// A received frame (after the buffer's 4-byte length word): Ralink's RXWI, 16 bytes, then the 802.11 frame. RXWI word
/// 0: WCID [7:0] (0xFF: no station), MPDU length [27:16]; bytes 8-10 the RSSI per chain (signed dBm). Measured on the
/// FM-1 against beacons and an action frame. The MAC decrypts a CCMP frame for a keyed WCID and strips its header.
#[inline(always)]
pub fn rx_mpdu_len(f: &[u8]) -> u32 {
    (u32::from(f[2]) | u32::from(f[3]) << 8) & 0xFFF
}

#[inline(always)]
pub fn rx_rssi(f: &[u8]) -> i32 {
    i32::from(f[8] as i8)
}

#[allow(dead_code)]
#[inline(always)]
pub fn mac_rd(r: u32) -> u32 {
    rd(MAC + r)
}

#[inline(always)]
pub fn mac_wr(r: u32, v: u32) {
    wr(MAC + r, v)
}

// The MAC's address and key tables (Ralink's layout; the SDK's AsicUpdateWcidAttributeEntry, RTMPSetWcidSecurityInfo):
// a station is WCID 1 (its AP), an access point's clients WCID 1..N.
/// Our address: bytes 0-3, then 4-5 | 0x00FF0000.
pub const ADDR_OWN: u32 = 0x1008;
/// The BSSID: bytes 0-3, then 4-5.
pub const ADDR_BSSID: u32 = 0x1010;
/// Ralink's RX_FILTR_CFG (bit n set = drop that kind).
pub const RX_FILTER: u32 = 0x1400;
/// .. drop only frames with a bad CRC.
pub const RX_FILTER_ALL: u32 = 0x1;
/// .. Ralink's STANORMAL, the SDK's STA driver's (wl_wifi_sta.a, written in fm1_wifi_sta_init): dropped are bad CRC
/// (bit 0), PHY errors (1), unicast not to our address (2: 0x1008, all six bytes compared, 0x100C [23:16] 0xFF), a
/// wrong version (4), retried duplicates (7), and the control frames: CF-End+ACK, CF-End, ACK, CTS, RTS, PS-Poll, BA,
/// the rest (8-14, 16; the MAC takes the ACK of its own frames itself: the host never needs them). Kept: other
/// networks' frames (3 clear), multicast (5 clear: Link's group), broadcast (6 clear: ARP, DHCP), BAR (15).
pub const RX_FILTER_STA: u32 = 0x17F97;
// The receiver's highest AGC gain (FM1_WIFI_AGC_*, entry 44 of the BBP's window C) is JieLi's adj_agc_max_gain by the
// joined AP's signal; the start-up program writes 66 there, and nothing here changes it.
/// WCID w's peer address: bytes 0-3, then 4-5.
pub const fn wcid_addr(w: u32) -> u32 {
    0x1800 + 8 * w
}
/// Its pairwise key (16 B, then 16 B of MIC keys: 0 for CCMP).
pub const fn pkey(w: u32) -> u32 {
    0x4000 + 32 * w
}
/// Its TX IV and EIV: ((key id << 6 | 0x20 ExtIV) << 24) | PN.
pub const fn iveiv(w: u32) -> u32 {
    0x6000 + 8 * w
}
/// Its attribute: bit 0 the pairwise table, bits 1-3 the cipher.
pub const fn wcid_attr(w: u32) -> u32 {
    0x6800 + 4 * w
}
/// Shared (group) key i of BSS 0.
pub const fn skey(i: u32) -> u32 {
    0x6C00 + 32 * i
}
/// The shared keys' ciphers, 4 bits each.
pub const SKEY_MODE: u32 = 0x7000;
pub const CIPHER_CCMP: u32 = 4;

/// A 32-byte key entry: the key (16 B), then 0s.
#[inline(always)]
pub fn key_wr(r: u32, key: &[u8; 16]) {
    for i in 0..8 {
        let w = if i < 4 { u32::from_le_bytes([key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]]) } else { 0 };
        mac_wr(r + 4 * i as u32, w);
    }
}

/// An address pair of words.
#[inline(always)]
pub fn addr_wr(r: u32, mac: &[u8; 6], hi_or: u32) {
    mac_wr(r, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
    mac_wr(r + 4, (u32::from(mac[4]) | u32::from(mac[5]) << 8) | hi_or);
}

/// Byte 0 = channel + 101 (the SDK's wifi_set_channel); byte 2 the TX power (6).
const CHAN: u32 = 0x1405C;

#[inline(always)]
pub fn channel() -> u32 {
    (rd(CHAN) & 0xFF).wrapping_sub(101)
}

#[inline(always)]
pub fn set_channel(ch: u32) {
    wr(CHAN, (rd(CHAN) & !0xFF) | (ch.wrapping_add(101) & 0xFF));
}

/// The Ralink STA driver's MAC settings when the interface comes up (rt28xx_init / NICInitializeAsic /
/// RTMPEnableRxTx, after the radio start-up), in its order, as the SDK's wl_wifi_sta.a writes them in the emulator; the
/// WCID / key table clears left out.
static STA_INIT: [[u32; 2]; 31] = [
    [0x042C, 0xF8F0E8E0], [0x0430, 0x6F77D0C8], [0x1408, 0x0000013F], [0x140C, 0x00008003],
    [0x1400, 0x00017F97], [0x1350, 0x00001020], [0x1018, 0x00001F00], [0x040C, 0x1F3FBF9F],
    [0x134C, 0x47D01F0F], [0x1404, 0x00000013], [0x1364, 0x05740003], [0x1368, 0x05740003],
    [0x0408, 0x00F40006], [0x1370, 0x03F44084], [0x1374, 0x01744004], [0x1378, 0x03F44084],
    [0x136C, 0x01744004], [0x1340, 0x0000583F], [0x1344, 0x00092B20], [0x1380, 0x002400CA],
    [0x1608, 0x00000002], [0x1204, 0x00000003], [0x1114, 0x00000000], [0x1340, 0x0000583F],
    [0x1008, 0x00000000], [0x100C, 0x00FF0000], [0x1300, 0x00064300], [0x1304, 0x000A4700],
    [0x1308, 0x00043538], [0x130C, 0x0003212F], [0x1400, 0x00017F97],
];
/// Bit 0 BT, bit 1 Wi-Fi: the RF paths (the SDK's wifi_rf_on / wifi_rf_off).
const WL_EN: u32 = 0x14000;

/// The Wi-Fi side on, on a radio BT has started: the STA settings, then the SDK's wlc_main (both RF paths on, the
/// coexistence controller's tables).
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn up() {
    for &[r, v] in &STA_INIT {
        mac_wr(r, v);
    }
    set(WL_EN, 3);
    for i in 0..wlc::N {
        wr(wlc::PORT, wlc::word(i));
    }
}

/// True: the bit came up within ticks (TIMER4).
#[inline(always)]
fn dma_wait(bit: u32, ticks: u32) -> bool {
    let t0 = T4_CNT.read();
    while rd(WDMA1) & bit == 0 {
        if T4_CNT.read().wrapping_sub(t0) > ticks {
            return false;
        }
    }
    true
}

/// One waiting frame into buf (max + 8 bytes; max <= RX_MAX), as the SDK's wl30_mmc_io_rw_extended (addr 12): the
/// length word first (big endian), then length + 4 bytes from buf + 4. Returns the length, 0 if none waited, -1 if
/// the DMA did not finish within 10 ms or the length was out of range. No cache maintenance around it (the SDK has a
/// csync only, which is a fence for the compiler too).
#[inline(always)]
pub fn rx<const N: usize>(buf: &mut Buf<N>, max: u32) -> i32 {
    // What the DMA may write is inside the buffer.
    assert!(max as usize + 8 <= N);
    if rd(WDMA1) & RX_PEND == 0 {
        return 0;
    }
    mac_wr(RX_ACK, 2);
    set(WDMA1, RX_CLR);
    sync_cpu();
    wr(RX_ADDR, buf.address());
    wr(RX_LEN, 4);
    sync_cpu();
    if !dma_wait(RX_DONE, 10000 * TICKS_PER_US) {
        return -1;
    }
    set(WDMA1, RX_CLR);
    sync_cpu();
    let w = buf.word(0);
    let n = w.swap_bytes();
    if w == 0 || n > max || n > RX_MAX {
        return -1;
    }
    wr(RX_ADDR, buf.address() + 4);
    wr(RX_LEN, n + 4);
    sync_cpu();
    if !dma_wait(RX_DONE, 10000 * TICKS_PER_US) {
        return -1;
    }
    set(WDMA1, RX_CLR);
    // The frame is read after this, never before: a fence for the compiler alone, no instruction.
    compiler_fence(Ordering::SeqCst);
    n as i32
}
