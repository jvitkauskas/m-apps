// SPDX-License-Identifier: MPL-2.0
//! What the radio hears, read: no hardware here, so it builds on a computer too, for its fuzz test. Every length in a
//! frame is untrusted: these stay inside the slice given. Moved from net.c (MPL-2.0).
use crate::network::{OPEN, OTHER, WPA2};

/// A beacon or probe response's network: its name (None if it gave none), BSSID, channel and security (WPA2, OPEN or
/// OTHER, network.rs).
pub struct Heard<'a> {
    pub ssid: Option<&'a [u8]>,
    pub bssid: &'a [u8],
    pub channel: u32,
    pub security: u8,
}

const CCMP: [u8; 4] = [0x00, 0x0f, 0xac, 0x04];
const PSK: [u8; 4] = [0x00, 0x0f, 0xac, 0x02];

/// The 16 bits at p, little-endian.
fn le16(b: &[u8], p: usize) -> usize {
    usize::from(b[p]) | usize::from(b[p + 1]) << 8
}

/// m as a beacon or probe response, or None if it isn't one: its BSSID at 16, fixed fields to 36 (capabilities at 34),
/// then the elements. heard_on is the channel it was heard on, for one that names none, or one that can't be.
pub fn beacon(m: &[u8], heard_on: u32) -> Option<Heard<'_>> {
    let len = m.len();
    if len < 36 || (m[0] != 0x80 && m[0] != 0x50) {
        return None;
    }
    let mut h = Heard { ssid: None, bssid: &m[16..22], channel: heard_on, security: OTHER };
    let (privacy, mut rsn, mut wpa2) = (m[34] & 0x10 != 0, false, false);
    let mut at = 36;
    while at + 2 <= len {
        let (id, l) = (m[at], usize::from(m[at + 1]));
        if at + 2 + l > len {
            break;
        }
        let b = &m[at + 2..at + 2 + l];
        if id == 0 && l <= 32 {
            h.ssid = Some(b);
        } else if id == 3 && l == 1 {
            h.channel = u32::from(b[0]);
        } else if id == 48 && l >= 2 {
            // RSN: version 1, group CCMP, a CCMP pairwise and a PSK key management among those offered, PMF not
            // required (capabilities bit 6).
            rsn = true;
            if l >= 8 && b[0] == 1 && b[1] == 0 && b[2..6] == CCMP {
                let mut p = 6;
                let pairs = le16(b, p);
                let (mut pair_ok, mut key_ok) = (false, false);
                p += 2;
                let mut i = 0;
                while i < pairs && p + 4 <= l {
                    pair_ok |= b[p..p + 4] == CCMP;
                    i += 1;
                    p += 4;
                }
                if p + 2 <= l {
                    let keys = le16(b, p);
                    p += 2;
                    let mut i = 0;
                    while i < keys && p + 4 <= l {
                        key_ok |= b[p..p + 4] == PSK;
                        i += 1;
                        p += 4;
                    }
                }
                let pmf_required = p + 2 <= l && b[p] & 0x40 != 0;
                wpa2 = pair_ok && key_ok && !pmf_required;
            }
        }
        at += 2 + l;
    }
    h.security = if wpa2 {
        WPA2
    } else if !privacy && !rsn {
        OPEN
    } else {
        OTHER
    };
    // A channel said that can't be one: the one it was heard on.
    if !(1..=13).contains(&h.channel) {
        h.channel = heard_on;
    }
    Some(h)
}

/// A name of zeros, as a hidden network sends.
pub fn hidden(ssid: &[u8]) -> bool {
    ssid.iter().all(|&c| c == 0)
}

/// Whether an IPv4 packet's lengths fit the bytes received (from its EtherType on), and a UDP one's too: the IP layer
/// (wifi_ip.rs) trusts them. Anything but IPv4 is let through as it is. Too little to hold an EtherType doesn't fit
/// (the app never passes that: the C read past it).
pub fn lengths_fit(type_and_packet: &[u8]) -> bool {
    let [t0, t1, p @ ..] = type_and_packet else { return false };
    if *t0 != 0x08 || *t1 != 0x00 {
        return true;
    }
    let n = p.len();
    if n < 20 || p[0] >> 4 != 4 {
        return false;
    }
    let (header, total) = (usize::from(p[0] & 15) * 4, usize::from(p[2]) << 8 | usize::from(p[3]));
    if header < 20 || total < header || total > n {
        return false;
    }
    if p[9] != 17 {
        return true;
    }
    if total - header < 8 {
        return false;
    }
    let udp = usize::from(p[header + 4]) << 8 | usize::from(p[header + 5]);
    (8..=total - header).contains(&udp)
}
