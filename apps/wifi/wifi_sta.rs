// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
// Copyright (C) 2026 Henri David (hdavid), Optimist (github.com/w0ts/optimist)
//! WPA2-PSK Station (IEEE 802.11i / 802.11-2020 §12.7): PTK derivation, 4-way handshake state machine (EAPOL-Key
//! M1-M4), GTK unwrapping, 802.11 Auth/Assoc framing, LLC/SNAP EAPOL encapsulation, and MAC key table installation.
//!
//! Taken here under MPL-2.0. Changed by m-apps: in Rust, on slices: what's received is a slice of what was received,
//! and what's built goes in the slice given. On a computer (the fuzzer) the MAC's writes go nowhere, as the C's fuzz
//! test had them (tests/fuzz/fake/fm1_wifi.h).
use crate::wifi_crypto::{aes_unwrap, hmac_sha1, prf};

/// felucca-bsp's MAC (apps/wifi/bsp/fm1_wifi.rs), or on a computer, its register places and writes done nowhere.
#[cfg(target_arch = "pi32v2")]
use crate::bsp::fm1_wifi as mac;
#[cfg(not(target_arch = "pi32v2"))]
mod mac {
    pub const ADDR_OWN: u32 = 0x1008;
    pub const ADDR_BSSID: u32 = 0x1010;
    pub const RX_FILTER: u32 = 0x1400;
    pub const RX_FILTER_ALL: u32 = 0x1;
    pub const fn wcid_addr(w: u32) -> u32 {
        0x1800 + 8 * w
    }
    pub const fn pkey(w: u32) -> u32 {
        0x4000 + 32 * w
    }
    pub const fn iveiv(w: u32) -> u32 {
        0x6000 + 8 * w
    }
    pub const fn wcid_attr(w: u32) -> u32 {
        0x6800 + 4 * w
    }
    pub const fn skey(i: u32) -> u32 {
        0x6C00 + 32 * i
    }
    pub const SKEY_MODE: u32 = 0x7000;
    pub const CIPHER_CCMP: u32 = 4;
    pub fn mac_wr(_r: u32, _v: u32) {}
    pub fn key_wr(_r: u32, _key: &[u8; 16]) {}
    pub fn addr_wr(_r: u32, _mac: &[u8; 6], _hi_or: u32) {}
    pub fn set_channel(_ch: u32) {}
}

pub const SSID_MAX: usize = 32;
pub const EAPOL_MAX: usize = 256;

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum State {
    Idle = 0,
    Auth,
    Assoc,
    WaitM1,
    WaitM3,
    Connected,
    Failed,
}

/// Standard WPA2-PSK RSN Information Element for Station (22 bytes).
static RSN_IE: [u8; 22] = [
    0x30, 0x14, // Tag 0x30 (RSN), length 20
    0x01, 0x00, // Version 1
    0x00, 0x0F, 0xAC, 0x04, // Group cipher: AES-CCMP
    0x01, 0x00, // Pairwise cipher count: 1
    0x00, 0x0F, 0xAC, 0x04, // Pairwise cipher: AES-CCMP
    0x01, 0x00, // AKM suite count: 1
    0x00, 0x0F, 0xAC, 0x02, // AKM: PSK (00-0F-AC-2)
    0x00, 0x00, // RSN capabilities
];

/// Random bytes for the SNonce (firmware: ble_hw_rand).
pub type Rand = fn(&mut [u8]);

pub struct Sta {
    pub state: State,
    /// Station MAC address (SPA).
    pub mac: [u8; 6],
    /// AP BSSID / MAC address (AA).
    pub bssid: [u8; 6],
    pub ssid: [u8; SSID_MAX],
    pub ssid_len: usize,
    /// Pairwise Master Key.
    pub pmk: [u8; 32],
    /// Station Nonce.
    pub snonce: [u8; 32],
    /// AP Nonce.
    pub anonce: [u8; 32],
    /// Derived PTK: the Key Confirmation Key (EAPOL MIC), the Key Encryption Key (AES unwrap), the Temporal Key (CCMP
    /// pairwise key), 16 bytes each.
    pub ptk: [u8; 48],
    /// Group Temporal Key.
    pub gtk: [u8; 16],
    /// GTK Key ID (1..3).
    pub gtk_id: u8,
    /// Last seen replay counter.
    pub replay_cnt: u64,
    /// Assigned Association ID.
    pub aid: u16,
    /// Transmit sequence counter.
    pub tx_seq: u16,
    pub rand: Option<Rand>,
}

fn kck(ptk: &[u8; 48]) -> &[u8; 16] {
    ptk.first_chunk().unwrap()
}

fn kek(ptk: &[u8; 48]) -> &[u8; 16] {
    ptk[16..].first_chunk().unwrap()
}

fn tk(ptk: &[u8; 48]) -> &[u8; 16] {
    ptk.last_chunk().unwrap()
}

pub fn derive_ptk(pmk: &[u8; 32], spa: &[u8; 6], aa: &[u8; 6], snonce: &[u8; 32], anonce: &[u8; 32]) -> [u8; 48] {
    let mut data = [0u8; 76];
    let (min_mac, max_mac) = if spa < aa { (spa, aa) } else { (aa, spa) };
    let (min_nonce, max_nonce) = if snonce < anonce { (snonce, anonce) } else { (anonce, snonce) };
    data[..6].copy_from_slice(min_mac);
    data[6..12].copy_from_slice(max_mac);
    data[12..44].copy_from_slice(min_nonce);
    data[44..].copy_from_slice(max_nonce);
    let mut ptk = [0u8; 48];
    prf(pmk, b"Pairwise key expansion", &data, &mut ptk);
    ptk
}

fn read_u64_be(p: &[u8]) -> u64 {
    u64::from_be_bytes(*p.first_chunk().unwrap())
}

fn write_u16_be(p: &mut [u8], v: u16) {
    p[..2].copy_from_slice(&v.to_be_bytes());
}

/// The MIC of an EAPOL frame, with its own MIC field taken as zeros; mic is left as it is for one too long.
fn compute_eapol_mic(kck: &[u8; 16], pkt: &[u8], mic: &mut [u8; 16]) {
    let mut tmp = [0u8; EAPOL_MAX];
    if pkt.len() > tmp.len() {
        return;
    }
    tmp[..pkt.len()].copy_from_slice(pkt);
    tmp[81..97].fill(0);
    let digest = hmac_sha1(kck, &tmp[..pkt.len()]);
    mic.copy_from_slice(&digest[..16]);
}

fn verify_eapol_mic(kck: &[u8; 16], pkt: &[u8]) -> bool {
    let mut expected = [0u8; 16];
    if pkt.len() > EAPOL_MAX {
        return false;
    }
    compute_eapol_mic(kck, pkt, &mut expected);
    pkt[81..97] == expected
}

impl Sta {
    pub const fn new(mac: [u8; 6], rand: Option<Rand>) -> Sta {
        Sta {
            state: State::Idle,
            mac,
            bssid: [0; 6],
            ssid: [0; SSID_MAX],
            ssid_len: 0,
            pmk: [0; 32],
            snonce: [0; 32],
            anonce: [0; 32],
            ptk: [0; 48],
            gtk: [0; 16],
            gtk_id: 0,
            replay_cnt: 0,
            aid: 0,
            tx_seq: 0,
            rand,
        }
    }

    /// A saved key.
    pub fn set_pmk(&mut self, pmk: &[u8; 32], ssid: &[u8]) {
        let n = ssid.len().min(SSID_MAX);
        self.ssid[..n].copy_from_slice(&ssid[..n]);
        self.ssid_len = n;
        // A saved network: its key, derived once.
        self.pmk = *pmk;
    }

    pub fn set_ap(&mut self, bssid: &[u8; 6]) {
        self.bssid = *bssid;
        self.state = State::WaitM1;
    }

    /// The SNonce: from the caller's random source (the firmware: the radio's, ble_hw_rand), never a predictable
    /// sequence.
    fn gen_snonce(&mut self) {
        if let Some(rand) = self.rand {
            rand(&mut self.snonce);
        }
    }

    fn parse_gtk_kde(&mut self, key_data: &[u8]) -> bool {
        let mut unwrapped = [0u8; 128];
        let n = key_data.len();
        if n < 24 || n > unwrapped.len() + 8 || n % 8 != 0 {
            return false;
        }
        if !aes_unwrap(kek(&self.ptk), key_data, &mut unwrapped) {
            return false;
        }
        let ulen = n - 8;
        let mut i = 0;
        while i + 6 < ulen {
            let (tag, tlen) = (unwrapped[i], usize::from(unwrapped[i + 1]));
            if i + 2 + tlen > ulen {
                break;
            }
            if tag == 0xDD && tlen >= 6 && unwrapped[i + 2..i + 6] == [0x00, 0x0F, 0xAC, 0x01] {
                self.gtk_id = unwrapped[i + 6] & 0x03;
                if tlen >= 22 {
                    self.gtk.copy_from_slice(&unwrapped[i + 8..i + 24]);
                }
                break;
            }
            i += 2 + tlen;
        }
        true
    }

    fn build_eapol_99(&self, replay: &[u8], key_info: u16, tx: &mut [u8]) {
        let tx = &mut tx[..99];
        tx.fill(0);
        // 802.1X-2001
        tx[0] = 0x01;
        // EAPOL-Key
        tx[1] = 0x03;
        // Body length
        write_u16_be(&mut tx[2..], 95);
        // RSN Key Descriptor
        tx[4] = 0x02;
        write_u16_be(&mut tx[5..], key_info);
        // Key Length
        write_u16_be(&mut tx[7..], 16);
        // Replay counter
        tx[9..17].copy_from_slice(&replay[..8]);
        let mut mic = [0u8; 16];
        compute_eapol_mic(kck(&self.ptk), tx, &mut mic);
        tx[81..97].copy_from_slice(&mic);
    }

    /// Installs the derived PTK and GTK into the RT3070 MAC hardware tables (0x1800, 0x4000, 0x6000, 0x6800, 0x6C00,
    /// 0x7000).
    pub fn install_hw_keys(&self) {
        // 1. Program AP BSSID into WCID entry 1 (0x1808).
        mac::addr_wr(mac::wcid_addr(1), &self.bssid, 0);
        // 2. Program Pairwise Key (TK) into Pairwise Key Table entry 1 (0x4020).
        mac::key_wr(mac::pkey(1), tk(&self.ptk));
        // 3. Program Group Key (GTK) into Shared Key Table (0x6C00 + gtk_id * 32).
        if self.gtk_id < 4 {
            mac::key_wr(mac::skey(u32::from(self.gtk_id)), &self.gtk);
            // Set Shared Key Mode (0x7000): cipher 4 (AES-CCMP) for this key.
            mac::mac_wr(mac::SKEY_MODE, mac::CIPHER_CCMP << (u32::from(self.gtk_id) * 4));
        }
        // 4. WCID Attribute Table (0x6800 + Wcid*4 = 0x6804), VERIFIED against the SDK's AsicUpdateWcidAttributeEntry
        // (cmm_asic.c): reg = (Wcid<<2)|0x6800; value = (KeyTabFlag&1) | ((cipher&7)<<1) | ((cipher>>3)<<10) | bss
        // bits. BUG FIX: KeyTabFlag (bit 0) selects the key table - 1 = PAIRWISE table (0x4000, where we put the TK), 0
        // = SHARED table (0x6c00). WPAInstallPairwiseKey (cmm_wpa.c) passes KeyTabFlag=1 for a pairwise key. We had 0,
        // so the MAC looked in the shared table for WCID 1's TX key and could not encrypt. AES-CCMP (cipher 4), BSS 0,
        // pairwise: value = (4<<1) | 1 = 0x09.
        mac::mac_wr(mac::wcid_attr(1), mac::CIPHER_CCMP << 1 | 1);
        // 5. WCID TX IV/EIV (0x6000 + Wcid*8 = 0x6008 / 0x600C), VERIFIED against the SDK's RTMPSetWcidSecurityInfo +
        // AsicUpdateWCIDIVEIV: for AES-CCMP the IV MUST carry the ExtIV bit (0x20) in the KeyID octet, else the MAC
        // emits a CCMP header with ExtIV=0 and the AP drops the frame (this was the bug: we wrote 0, so rx_packets
        // stayed at the 2 EAPOL frames). Vendor value = ((KeyID<<6 | 0x20) << 24) | 1 = 0x20000001 for KeyID 0; EIV =
        // 0.
        mac::mac_wr(mac::iveiv(1), 0x2000_0001);
        mac::mac_wr(mac::iveiv(1) + 4, 0);
    }

    /// Feeds a received EAPOL-Key packet. If a response packet (M2 or M4) is produced, returns 1 and writes its length
    /// into tx_len; returns 0 if consumed with no response, -1 on error.
    pub fn rx_eapol(&mut self, pkt: &[u8], tx: &mut [u8], tx_len: &mut u32) -> i32 {
        if pkt.len() < 99 {
            return -1;
        }
        // Check EAPOL Type = 3 (EAPOL-Key).
        if pkt[1] != 0x03 {
            return -1;
        }
        let body_len = usize::from(u16::from_be_bytes([pkt[2], pkt[3]]));
        if body_len + 4 > pkt.len() {
            return -1;
        }
        // 2 = RSN (802.11i)
        if pkt[4] != 0x02 {
            return -1;
        }
        let key_info = u16::from_be_bytes([pkt[5], pkt[6]]);
        let replay = read_u64_be(&pkt[9..]);
        let key_data_len = usize::from(u16::from_be_bytes([pkt[97], pkt[98]]));
        // (The EAPOL frame itself: any padding after it is not MIC'd.)
        let pkt = &pkt[..4 + body_len];
        // (Key data past the frame, or a frame too short to hold the fields above: refused.)
        if 99 + key_data_len > pkt.len() {
            return -1;
        }
        let key_data = &pkt[99..99 + key_data_len];

        // Message 1 or Message 3 (Pairwise) vs Group Message 1.
        if key_info & 1 << 3 != 0 {
            // Message 1 from AP: Key ACK=1, MIC=0.
            if key_info & 1 << 7 != 0 && key_info & 1 << 8 == 0 {
                self.replay_cnt = replay;
                self.anonce.copy_from_slice(&pkt[17..49]);
                // Generate SNonce and derive PTK.
                self.gen_snonce();
                self.ptk = derive_ptk(&self.pmk, &self.mac, &self.bssid, &self.snonce, &self.anonce);
                // Build Message 2.
                let tx = &mut tx[..121];
                tx.fill(0);
                // 802.1X-2001
                tx[0] = 0x01;
                // EAPOL-Key
                tx[1] = 0x03;
                // Body length
                write_u16_be(&mut tx[2..], 95 + RSN_IE.len() as u16);
                // RSN Key Descriptor
                tx[4] = 0x02;
                // Key Info: Version 2 (HMAC-SHA1/AES), Key Type Pairwise, Key MIC
                write_u16_be(&mut tx[5..], 0x010A);
                // Key Length
                write_u16_be(&mut tx[7..], 16);
                // Replay counter
                tx[9..17].copy_from_slice(&pkt[9..17]);
                // SNonce
                tx[17..49].copy_from_slice(&self.snonce);
                // Key Data: Station RSN IE
                write_u16_be(&mut tx[97..], RSN_IE.len() as u16);
                tx[99..].copy_from_slice(&RSN_IE);
                // Compute MIC
                let mut mic = [0u8; 16];
                compute_eapol_mic(kck(&self.ptk), tx, &mut mic);
                tx[81..97].copy_from_slice(&mic);
                *tx_len = 121;
                self.state = State::WaitM3;
                return 1;
            }

            // Message 3 from AP: Key ACK=1, MIC=1.
            if key_info & 1 << 7 != 0 && key_info & 1 << 8 != 0 {
                if self.state != State::WaitM3 || replay <= self.replay_cnt {
                    return -1;
                }
                if !verify_eapol_mic(kck(&self.ptk), pkt) || pkt[17..49] != self.anonce {
                    self.state = State::Failed;
                    return -1;
                }
                // If Key Data is encrypted, unwrap GTK.
                if key_info & 1 << 12 != 0 && key_data_len >= 24 && !self.parse_gtk_kde(key_data) {
                    self.state = State::Failed;
                    return -1;
                }
                self.replay_cnt = replay;
                self.build_eapol_99(&pkt[9..], 0x030A, tx);
                *tx_len = 99;
                // Install hardware keys.
                self.install_hw_keys();
                self.state = State::Connected;
                return 1;
            }
        } else {
            // Group Key Handshake Message 1 from AP: Key Type=0, Key ACK=1, Key MIC=1, Secure=1.
            if key_info & 1 << 7 != 0 && key_info & 1 << 8 != 0 && key_info & 1 << 9 != 0 {
                if self.state != State::Connected || replay <= self.replay_cnt {
                    return -1;
                }
                if !verify_eapol_mic(kck(&self.ptk), pkt) {
                    return -1;
                }
                self.replay_cnt = replay;
                // Unwrap GTK from Key Data.
                if key_info & 1 << 12 != 0 && key_data_len >= 24 {
                    if !self.parse_gtk_kde(key_data) {
                        return -1;
                    }
                    self.install_hw_keys();
                }
                self.build_eapol_99(&pkt[9..], 0x0302, tx);
                *tx_len = 99;
                return 1;
            }
        }
        0
    }

    fn build_mgmt_hdr(&mut self, fc0: u8, tx: &mut [u8]) {
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        tx[..24].fill(0);
        tx[0] = fc0;
        // Duration
        tx[2] = 0x3A;
        tx[3] = 0x01;
        // Addr 1: DA (AP BSSID)
        tx[4..10].copy_from_slice(&self.bssid);
        // Addr 2: SA (Station MAC)
        tx[10..16].copy_from_slice(&self.mac);
        // Addr 3: BSSID
        tx[16..22].copy_from_slice(&self.bssid);
        // Sequence Control
        tx[22] = (seq << 4) as u8;
        tx[23] = (seq >> 4) as u8;
    }

    fn parse_mgmt_resp(&self, frame: &[u8], fc0: u8) -> bool {
        frame.len() >= 30 && frame[0] & 0xFC == fc0 && frame[4..10] == self.mac && frame[10..16] == self.bssid
    }

    /// 802.11 Authentication Frame: builds Open System Auth (Seq 1) frame. Returns frame byte count.
    pub fn build_auth(&mut self, tx: &mut [u8]) -> u32 {
        self.build_mgmt_hdr(0xB0, tx);
        // Auth body: Algorithm Open System (0), Seq 1, Status 0.
        tx[24..30].copy_from_slice(&[0, 0, 1, 0, 0, 0]);
        30
    }

    /// Parses 802.11 Auth Response from AP (Seq 2, Status 0).
    pub fn parse_auth_resp(&self, frame: &[u8]) -> bool {
        // Body: Alg 0, Seq 2, Status 0.
        self.parse_mgmt_resp(frame, 0xB0) && frame[24..30] == [0, 0, 2, 0, 0, 0]
    }

    /// Deauthentication, reason 3 (the sending station is leaving): the AP forgets us, so a later join starts clean.
    pub fn build_deauth(&mut self, tx: &mut [u8]) -> u32 {
        self.build_mgmt_hdr(0xC0, tx);
        tx[24] = 0x03;
        tx[25] = 0x00;
        26
    }

    /// 802.11 Association Request Frame: builds Assoc Req with SSID, Rates, Ext Rates, RSN IE. Returns frame byte
    /// count.
    pub fn build_assoc_req(&mut self, tx: &mut [u8]) -> u32 {
        const RATES: [u8; 10] = [0x01, 0x08, 0x82, 0x84, 0x8B, 0x96, 0x0C, 0x12, 0x18, 0x24];
        const EXT_RATES: [u8; 6] = [0x32, 0x04, 0x30, 0x48, 0x60, 0x6C];
        self.build_mgmt_hdr(0x00, tx);
        let mut off = 24;
        let mut put = |b: &[u8]| {
            tx[off..off + b.len()].copy_from_slice(b);
            off += b.len();
        };
        // Fixed Parameters: Capability Info (0x0431): ESS, Short Preamble, Short Slot, Privacy; Listen Interval (10).
        put(&[0x31, 0x04, 0x0A, 0x00]);
        // Tag 0: SSID
        put(&[0x00, self.ssid_len as u8]);
        put(&self.ssid[..self.ssid_len]);
        // Tag 1: Supported Rates (1, 2, 5.5, 11, 6, 9, 12, 18 Mbps)
        put(&RATES);
        // Tag 50 (0x32): Extended Supported Rates (24, 36, 48, 54 Mbps)
        put(&EXT_RATES);
        // Tag 48 (0x30): RSN IE (22 bytes)
        put(&RSN_IE);
        off as u32
    }

    /// Parses 802.11 Assoc Response from AP (Status 0, records AID).
    pub fn parse_assoc_resp(&mut self, frame: &[u8]) -> bool {
        if !self.parse_mgmt_resp(frame, 0x10) {
            return false;
        }
        // Fixed parameters: Capability (24..25), Status (26..27), AID (28..29).
        if u16::from_le_bytes([frame[26], frame[27]]) != 0 {
            return false;
        }
        self.aid = u16::from_le_bytes([frame[28], frame[29]]) & 0x3FFF;
        true
    }

    /// Encapsulates an EAPOL packet in an 802.11 Data frame with LLC/SNAP header (AA-AA-03-00-00-00-88-8E). Returns
    /// frame byte count.
    pub fn encap_eapol(&mut self, eapol: &[u8], tx: &mut [u8]) -> u32 {
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        // 802.11 Data header (24 bytes): ToDS=1, FromDS=0.
        tx[..24].fill(0);
        // Type: Data, Subtype: Data
        tx[0] = 0x08;
        // ToDS = 1
        tx[1] = 0x01;
        // Addr 1: RA / BSSID
        tx[4..10].copy_from_slice(&self.bssid);
        // Addr 2: TA / SA
        tx[10..16].copy_from_slice(&self.mac);
        // Addr 3: DA
        tx[16..22].copy_from_slice(&self.bssid);
        tx[22] = (seq << 4) as u8;
        tx[23] = (seq >> 4) as u8;
        // LLC/SNAP Header (8 bytes): AA-AA-03-00-00-00-88-8E
        tx[24..32].copy_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8E]);
        // EAPOL payload
        tx[32..32 + eapol.len()].copy_from_slice(eapol);
        32 + eapol.len() as u32
    }

    /// Processes an incoming 802.11 frame (Auth Resp, Assoc Resp, or EAPOL Data). If an EAPOL response (M2/M4 wrapped
    /// in 802.11 Data + LLC/SNAP) is produced, returns 1 and sets tx_len; returns 0 if consumed, -1 on error.
    pub fn rx_frame(&mut self, frame: &[u8], tx: &mut [u8], tx_len: &mut u32) -> i32 {
        if frame.len() < 24 {
            return -1;
        }
        if self.state == State::Auth {
            if self.parse_auth_resp(frame) {
                self.state = State::Assoc;
                return 0;
            }
            return -1;
        }
        if self.state == State::Assoc {
            if self.parse_assoc_resp(frame) {
                self.state = State::WaitM1;
                return 0;
            }
            return -1;
        }
        // Data frame: check for LLC/SNAP 802.1X EAPOL.
        if frame[0] & 0x0C != 0x08 {
            return -1;
        }
        // Check QoS Data bit.
        let hdr_len = if frame[0] & 0x80 != 0 { 26 } else { 24 };
        if frame.len() < hdr_len + 8 {
            return -1;
        }
        // Verify LLC/SNAP header (AA-AA-03-00-00-00-88-8E).
        if frame[hdr_len..hdr_len + 8] != [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8E] {
            return -1;
        }
        let eapol = &frame[hdr_len + 8..];
        let mut resp = [0u8; EAPOL_MAX];
        let mut resp_len = 0;
        let res = self.rx_eapol(eapol, &mut resp, &mut resp_len);
        if res == 1 {
            *tx_len = self.encap_eapol(&resp[..resp_len as usize], tx);
            return 1;
        }
        res
    }
}

/// The MAC as it is at boot, before a join: WCID 1 has no cipher and no key, its IV is clear. A key left from the last
/// join would stay in use for the frames of the next one (the MAC encrypts what it has a key for), which made a re-join
/// fail after a join that had worked.
pub fn clear_hw_keys() {
    // WCID 1 attribute: no cipher, the shared table.
    mac::mac_wr(mac::wcid_attr(1), 0);
    // IV, EIV
    mac::mac_wr(mac::iveiv(1), 0);
    mac::mac_wr(mac::iveiv(1) + 4, 0);
    // The pairwise key's 16 bytes.
    for i in 0..4 {
        mac::mac_wr(mac::pkey(1) + 4 * i, 0);
    }
    // The shared key mode.
    mac::mac_wr(mac::SKEY_MODE, 0);
}

/// Programs Station MAC (0x1008/0x100C), Target BSSID (0x1010/0x1014), and Radio Channel (0x1405C).
pub fn set_mac_regs(sta_mac: &[u8; 6], bssid: &[u8; 6], channel: u32) {
    // Our address
    mac::addr_wr(mac::ADDR_OWN, sta_mac, 0x00FF_0000);
    // The BSSID
    mac::addr_wr(mac::ADDR_BSSID, bssid, 0);
    mac::set_channel(channel);
    mac::mac_wr(mac::RX_FILTER, mac::RX_FILTER_ALL);
}
