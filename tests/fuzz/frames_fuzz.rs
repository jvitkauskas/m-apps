// SPDX-License-Identifier: MIT
//! libFuzzer: what the radio hears, on any bytes, as apps/wifi/net.rs hands it on: a beacon or probe response
//! (frames.rs), a data frame through the length check to the IP layer (wifi_ip.rs: ARP, DHCP, UDP), and a WPA2 key
//! message or a management response to the station (wifi_sta.rs). Nothing may read outside the frame (a slice's bounds
//! check panics). The first byte picks the IP layer's and the station's states. Off the unit, the station's writes to
//! the MAC go nowhere (wifi_sta.rs's `mac`).
#![no_main]
#[macro_use]
#[path = "target.rs"]
mod target;
#[allow(dead_code)]
#[path = "../../apps/wifi/frames.rs"]
mod frames;
#[allow(dead_code)]
#[path = "../../apps/wifi/network.rs"]
mod network;
#[allow(dead_code)]
#[path = "../../apps/wifi/wifi_crypto.rs"]
mod wifi_crypto;
#[allow(dead_code)]
#[path = "../../apps/wifi/wifi_ip.rs"]
mod wifi_ip;
#[allow(dead_code)]
#[path = "../../apps/wifi/wifi_sta.rs"]
mod wifi_sta;
use wifi_ip::{Dhcp, WifiIp};
use wifi_sta::{Sta, State};

const MAC: [u8; 6] = [2, 0x11, 0x22, 0x33, 0x44, 0x55];
const AP: [u8; 6] = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60];

fn zeros(out: &mut [u8]) {
    out.fill(0);
}

fn udp_rx(_ctx: usize, _now_us: i64, _src_ip: u32, _src_port: u16, _dst_ip: u32, _dst_port: u16, p: &[u8]) {
    std::hint::black_box(p.iter().fold(0u8, |sum, &b| sum ^ b));
}

/// Whether s lies within f.
fn inside(s: &[u8], f: &[u8]) -> bool {
    let (at, start) = (s.as_ptr() as usize, f.as_ptr() as usize);
    at >= start && s.len() <= f.len() && at - start <= f.len() - s.len()
}

fuzz!(|data| {
    let [state, f @ ..] = data else { return };
    if data.len() > 2048 {
        return;
    }
    let state = usize::from(*state);
    let mut tx = [0u8; 2048];
    let mut tx_len = 0u32;

    if let Some(h) = frames::beacon(f, 6) {
        assert!((1..=13).contains(&h.channel) && h.security <= network::OTHER);
        assert!(h.bssid.len() == 6 && inside(h.bssid, f));
        if let Some(ssid) = h.ssid {
            assert!(ssid.len() <= 32 && inside(ssid, f));
            frames::hidden(ssid);
        }
    }

    // net.rs's on_data
    let mut ip = Box::new(WifiIp::new(MAC, AP, Some(zeros)));
    ip.dhcp_xid = 0x1234_5678;
    ip.dhcp_state = [Dhcp::Idle, Dhcp::Discovering, Dhcp::Requesting, Dhcp::Bound, Dhcp::Failed][state % 5];
    ip.ip = if state & 8 != 0 { 0xc0a8_0105 } else { 0 };
    ip.udp_rx = Some(udp_rx);
    ip.bind(5353);
    if f.len() >= 32 && f[0] & 0x0c == 0x08 {
        let header = if f[0] & 0x80 != 0 { 26 } else { 24 };
        if f.len() >= header + 8 && ip.early_filter(f) && frames::lengths_fit(&f[header + 6..]) {
            ip.rx_frame(f, 0, &mut tx, &mut tx_len);
        }
    }

    // the join's on_join: a station waiting for the first or third key message
    let mut sta = Box::new(Sta::new(MAC, Some(zeros)));
    let mut pmk = [0u8; 32];
    pmk[..3].copy_from_slice(&[1, 2, 3]);
    sta.set_pmk(&pmk, b"fuzz");
    sta.set_ap(&AP);
    // Every state a received frame can find it in: waiting for the authentication or association response, for the
    // first or third key message, or joined (a group key change). (A third message with a MIC that holds needs the
    // handshake's own keys, which random bytes don't reach.)
    sta.state = [State::Auth, State::Assoc, State::WaitM1, State::WaitM3, State::Connected][(state >> 4) % 5];
    sta.rx_frame(f, &mut tx, &mut tx_len);
});
