// SPDX-License-Identifier: MPL-2.0
//! WiFi for an app, polled: the radio (bsp/, from fm1-base), a scan, a WPA2-PSK join and DHCP (Optimist's own stack,
//! wifi_sta, wifi_crypto, wifi_ip). The join follows w0ts/fm1-base's device/wifi_own.c (GPL-3.0-only OR MPL-2.0, taken
//! here under MPL-2.0): Copyright (C) 2026 Henri David (hdavid), Optimist (github.com/w0ts/optimist).
//!
//! Open and WPA2-PSK networks can be joined (network.rs says what's heard), nothing else, WPA3-only among them.
use crate::bsp::fm1_ble::{self, rng32};
use crate::bsp::fm1_ble_rf::Rf;
use crate::bsp::fm1_cc::{rd, wr};
use crate::bsp::fm1_time::{self, TICKS_PER_US, ticks};
use crate::bsp::fm1_wifi::{self as wifi, Buf};
use crate::frames;
use crate::network::{Ap, Joined, OPEN, OTHER, STEP_ADDRESS, STEP_FINDING, STEP_JOINING, STEP_KEY, STEP_RADIO, STEPS};
use crate::sha1hw;
use crate::wifi_crypto::PmkJob;
use crate::wifi_ip::WifiIp;
use crate::wifi_sta::{self, Sta, State as Station};
use kit::hw::sync_cpu;
use kit::log::Hex;
use kit::platform::{flash_init, flash_uid};
use kit::{Global, app, log};

const LISTEN_MS: u32 = 150;
const FIND_PASSES: u32 = 3;
const STEP_MS: u32 = 400;
const STEP_TRIES: u32 = 6;
const ATTEMPTS: u32 = 3;
const HANDSHAKE_MS: u32 = 6000;
const DHCP_MS: u32 = 15000;
const RX_MOST: u32 = 1600;

const RX_BYTES: usize = 4 * ((RX_MOST as usize + 8) / 4 + 1);
const TX_BYTES: usize = wifi::TX_HDR + 1600;
type Tx = Buf<TX_BYTES>;

/// The radio's DMA reaches only RAM below 0x01C20000: the kit's .dma region (kit/app.ld.S).
struct Dma {
    rx: Buf<RX_BYTES>,
    tx: Tx,
}

// Safety: a NOLOAD section of the slot's linker script, for buffers alone: nothing there is code or the kit's.
#[unsafe(link_section = ".dma")]
static DMA: Global<Dma> = Global::new(Dma { rx: Buf::new(), tx: Buf::new() });

/// The SDK board's constant records, as Optimist passes them; 187 is measured (cal_on).
static X106: [u8; 2] = [0x0B, 0x0B];
static X107: [u8; 7] = [1, 7, 4, 7, 11, 1, 7];
static X108: [u8; 20] = [0; 20];
static X187: [u8; 68] = [0; 68];

// What the radio's start-up changes outside the MAC, as it was found, for off(): the RF / BT enables (0x14000) and
// 0x14028, the Wi-Fi clock / reset block (0x14040-0x1405C) and the radio's analog settings (0x11900-0x11968).
const WL_EN: u32 = 0x14000;
const WL_28: u32 = 0x14028;
const WL_CLOCK: u32 = 0x14040;
const WL_CLOCK_N: usize = 8;
const WL_ANALOG: u32 = 0x11900;
const WL_ANALOG_N: usize = 27;

struct FoundRegs {
    en: u32,
    w28: u32,
    clock: [u32; WL_CLOCK_N],
    analog: [u32; WL_ANALOG_N],
}

/// What went in and out, for the log when an address doesn't come.
#[derive(Clone, Copy)]
struct Count {
    frames: u32,
    data: u32,
    ours: u32,
    ip: u32,
    sent: u32,
    send_errors: u32,
}

const NO_COUNT: Count = Count { frames: 0, data: 0, ours: 0, ip: 0, sent: 0, send_errors: 0 };

/// The station's state, all zeros until it starts.
pub struct State {
    ip: WifiIp,
    sta: Sta,
    job: PmkJob,
    pmk: [u8; 32],
    mac: [u8; 6],
    started: bool,
    joined: bool,
    open_net: bool,
    found_regs: FoundRegs,
    t_last: u32,
    t_us: i64,
    count: Count,
    rf: Rf,
    /// The network being joined, as heard.
    found: Ap,
    /// Probe responses naming it: whether asking by name is heard.
    answers: u32,
    probe_seq: u16,
}

static STATE: Global<State> = Global::new(State {
    ip: WifiIp::new([0; 6], [0; 6], None),
    sta: Sta::new([0; 6], None),
    job: PmkJob::new(),
    pmk: [0; 32],
    mac: [0; 6],
    started: false,
    joined: false,
    open_net: false,
    found_regs: FoundRegs { en: 0, w28: 0, clock: [0; WL_CLOCK_N], analog: [0; WL_ANALOG_N] },
    t_last: 0,
    t_us: 0,
    count: NO_COUNT,
    rf: Rf::new(),
    found: Ap::new(),
    answers: 0,
    probe_seq: 0,
});

/// The radio, which the app takes once.
pub struct Radio {
    s: &'static mut State,
    dma: &'static mut Dma,
}

impl State {
    fn now_us(&mut self) -> i64 {
        let d = ticks().wrapping_sub(self.t_last) / TICKS_PER_US;
        self.t_last = self.t_last.wrapping_add(d * TICKS_PER_US);
        self.t_us += i64::from(d);
        self.t_us
    }

    fn now_ms(&mut self) -> u32 {
        (self.now_us() / 1000) as u32
    }
}

fn rand_bytes(out: &mut [u8]) {
    for (i, o) in out.iter_mut().enumerate() {
        *o = (rng32() ^ ticks() >> (i & 7)) as u8;
    }
}

fn frame_out(tx: &mut Tx) -> &mut [u8] {
    &mut tx.0[wifi::TX_HDR..]
}

/// The frame at frame_out(), len bytes, out at 1 Mb/s; ack: the MAC waits for the receiver's ACK, and retries.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn transmit(count: &mut Count, tx: &mut Tx, len: u32, ack: u32) {
    if len == 0 {
        return;
    }
    wifi::tx_prepare_rate(tx, len, 1, ack, wifi::RATE_CCK1);
    if wifi::tx(tx) == 0 {
        count.sent += 1;
    } else {
        count.send_errors += 1;
    }
}

fn send(count: &mut Count, tx: &mut Tx, len: u32) {
    transmit(count, tx, len, 1);
}

// ---- the networks heard ----

fn on_scan(list: &mut [Ap], scan_count: &mut usize, m: &[u8], rssi: i8) {
    let Some(h) = frames::beacon(m, wifi::channel()) else { return };
    let Some(ssid) = h.ssid else { return };
    if frames::hidden(ssid) {
        return;
    }
    let ap = match list[..*scan_count].iter().position(|a| a.ssid() == ssid) {
        Some(i) => &mut list[i],
        None => {
            if *scan_count == list.len() {
                return;
            }
            let ap = &mut list[*scan_count];
            *scan_count += 1;
            *ap = Ap::new();
            ap.ssid[..ssid.len()].copy_from_slice(ssid);
            ap.rssi = -128;
            ap
        }
    };
    if rssi > ap.rssi {
        ap.rssi = rssi;
        ap.bssid.copy_from_slice(h.bssid);
        ap.channel = h.channel as u8;
        ap.security = h.security;
    }
}

// ---- joining ----

fn on_find(s: &mut State, want: &[u8], m: &[u8], rssi: i8) {
    let Some(h) = frames::beacon(m, wifi::channel()) else { return };
    if h.ssid.unwrap_or(&[]) != want {
        return;
    }
    s.answers += u32::from(m[0] == 0x50);
    if rssi > s.found.rssi {
        s.found.rssi = rssi;
        s.found.bssid.copy_from_slice(h.bssid);
        s.found.channel = h.channel as u8;
        s.found.security = h.security;
    }
}

fn on_join(s: &mut State, tx: &mut Tx, f: &[u8]) {
    let mut tx_len = 0;
    if s.sta.state != Station::Connected && s.sta.rx_frame(f, frame_out(tx), &mut tx_len) == 1 && tx_len != 0 {
        send(&mut s.count, tx, tx_len);
    }
}

fn on_data(s: &mut State, tx: &mut Tx, f: &[u8]) {
    let mut tx_len = 0;
    let len = f.len();
    if len < 32 || f[0] & 0x0C != 0x08 {
        return;
    }
    s.count.data += 1;
    let header = if f[0] & 0x80 != 0 { 26 } else { 24 };
    if len < header + 8 || !s.ip.early_filter(f) || !frames::lengths_fit(&f[header + 6..]) {
        return;
    }
    s.count.ours += 1;
    // EAPOL: a new group key.
    if f[header + 6] == 0x88 && f[header + 7] == 0x8E {
        if s.sta.rx_frame(f, frame_out(tx), &mut tx_len) == 1 {
            send(&mut s.count, tx, tx_len);
        }
        return;
    }
    let now = s.now_us();
    let got = s.ip.rx_frame(f, now, frame_out(tx), &mut tx_len);
    if got >= 0 {
        s.count.ip += 1;
    }
    if got == 1 {
        send(&mut s.count, tx, tx_len);
    }
}

fn build_auth(s: &mut State, b: &mut [u8]) -> u32 {
    s.sta.build_auth(b)
}

/// The station's association request, which is WPA2's: for an open network, without its RSN element, which it puts
/// last, and without the Privacy capability.
fn build_assoc(s: &mut State, b: &mut [u8]) -> u32 {
    let mut n = s.sta.build_assoc_req(b);
    if s.open_net && n >= 24 + 4 + 22 && b[n as usize - 22] == 0x30 {
        b[24] &= !0x10;
        n -= 22;
    }
    n
}

impl Radio {
    /// # Safety
    /// Called once: there's one radio, and this is the only reference to its state and its buffers.
    pub unsafe fn claim() -> Radio {
        // Safety: as the caller has said, these are the only references to them.
        unsafe { Radio { s: STATE.get(), dma: DMA.get() } }
    }

    /// Up to max frames waiting, each to f as its 802.11 frame, without the radio's header, with its RSSI.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    fn receive(&mut self, max: u32, f: &mut dyn FnMut(&mut State, &mut Tx, &[u8], i8)) {
        let Dma { rx, tx } = &mut *self.dma;
        for _ in 0..max {
            let n = wifi::rx(rx, RX_MOST);
            if n <= 0 {
                return;
            }
            let fr = &rx.0[4..];
            let mpdu = wifi::rx_mpdu_len(fr) as usize;
            self.s.count.frames += 1;
            if mpdu + wifi::RXWI <= n as usize {
                f(self.s, tx, &fr[wifi::RXWI..wifi::RXWI + mpdu], wifi::rx_rssi(fr) as i8);
            }
        }
    }

    fn send(&mut self, build: impl FnOnce(&mut State, &mut [u8]) -> u32) {
        let len = build(self.s, frame_out(&mut self.dma.tx));
        send(&mut self.s.count, &mut self.dma.tx, len);
    }

    fn start(&mut self) {
        let s = &mut *self.s;
        if s.started {
            return;
        }
        fm1_time::init();
        s.t_last = ticks();
        // A locally administered address of the unit's own, from its flash's unique ID.
        let mut uid = [0u8; 8];
        if flash_init(1, 1) {
            flash_uid(&mut uid);
        }
        for i in 0..6 {
            s.mac[i] = uid[i] ^ uid[(i + 6) & 7] ^ 0x5a;
        }
        s.mac[0] = (s.mac[0] & 0xfc) | 0x02;
        s.found_regs.en = rd(WL_EN);
        s.found_regs.w28 = rd(WL_28);
        for (i, r) in s.found_regs.clock.iter_mut().enumerate() {
            *r = rd(WL_CLOCK + 4 * i as u32);
        }
        for (i, r) in s.found_regs.analog.iter_mut().enumerate() {
            *r = rd(WL_ANALOG + 4 * i as u32);
        }
        s.rf.cal_on = true;
        let t0 = ticks();
        fm1_ble::rf_init(&mut s.rf, &[&X106, &X107, &X108, &X187]);
        wifi::up();
        log!("radio up in ", ticks().wrapping_sub(t0) / TICKS_PER_US / 1000, " ms, calibration ", s.rf.scan_rc, " ",
            s.rf.trim_rc);
        s.started = true;
    }

    fn listen(&mut self, channel: u32, f: &mut dyn FnMut(&mut State, &mut Tx, &[u8], i8)) {
        wifi::set_channel(channel);
        let t0 = self.s.now_ms();
        while self.s.now_ms().wrapping_sub(t0) < LISTEN_MS {
            self.receive(24, f);
        }
        app::feed();
    }

    /// The networks heard on channels 1 to 13 into list, a name once (its strongest), strongest first, hidden ones
    /// left out: how many. progress is told after each channel, of 13. Starts the radio the first time.
    pub fn scan(&mut self, list: &mut [Ap], mut progress: impl FnMut(u32, u32)) -> usize {
        self.start();
        self.leave();
        wifi::mac_wr(wifi::RX_FILTER, wifi::RX_FILTER_ALL);
        let mut count = 0;
        for ch in 1..=13 {
            self.listen(ch, &mut |_, _, m, rssi| on_scan(list, &mut count, m, rssi));
            progress(ch, 13);
        }
        for i in 1..count {
            let mut j = i;
            while j != 0 && list[j].rssi > list[j - 1].rssi {
                list.swap(j, j - 1);
                j -= 1;
            }
        }
        log!("scan: ", count, " networks");
        count
    }

    /// A probe request naming the network: one that doesn't say its name in its beacons (a hidden one) answers it
    /// with a probe response that does. Broadcast, so nothing acknowledges it.
    fn probe(&mut self, ssid: &[u8]) {
        const RATES: [u8; 16] = [1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 50, 4, 0x30, 0x48, 0x60, 0x6c];
        let s = &mut *self.s;
        let b = frame_out(&mut self.dma.tx);
        b[..24].fill(0);
        b[0] = 0x40;
        b[4..10].fill(0xff);
        b[10..16].copy_from_slice(&s.mac);
        b[16..22].fill(0xff);
        b[22] = (s.probe_seq << 4) as u8;
        b[23] = (s.probe_seq >> 4) as u8;
        s.probe_seq = s.probe_seq.wrapping_add(1);
        let mut at = 24;
        b[at] = 0;
        b[at + 1] = ssid.len() as u8;
        at += 2;
        b[at..at + ssid.len()].copy_from_slice(ssid);
        at += ssid.len();
        b[at..at + RATES.len()].copy_from_slice(&RATES);
        transmit(&mut s.count, &mut self.dma.tx, (at + RATES.len()) as u32, 0);
    }

    /// Sends a request until the state moves on from state, a few times.
    fn step(&mut self, state: Station, build: fn(&mut State, &mut [u8]) -> u32) -> bool {
        self.send(build);
        let mut tries = 1;
        let mut t0 = self.s.now_ms();
        while self.s.sta.state == state {
            self.receive(24, &mut on_join_rx);
            if self.s.now_ms().wrapping_sub(t0) >= STEP_MS {
                let tried = tries;
                tries += 1;
                if tried >= STEP_TRIES {
                    return false;
                }
                t0 = self.s.now_ms();
                self.send(build);
            }
            app::feed();
        }
        true
    }

    /// None, or what failed.
    fn join_found(&mut self, want: &[u8]) -> Option<&'static str> {
        let mut why = "It didn't answer";
        self.s.count = NO_COUNT;
        for attempt in 1..=ATTEMPTS {
            let s = &mut *self.s;
            wifi_sta::set_mac_regs(&s.mac, &s.found.bssid, u32::from(s.found.channel));
            s.sta = Sta::new(s.mac, Some(rand_bytes));
            s.sta.set_pmk(&s.pmk, want);
            s.sta.set_ap(&s.found.bssid);
            s.sta.state = Station::Auth;
            self.send(|s, b| s.sta.build_deauth(b));
            let t0 = self.s.now_ms();
            while self.s.now_ms().wrapping_sub(t0) < 150 {
                self.receive(24, &mut |_, _, _, _| {});
            }
            if !self.step(Station::Auth, build_auth) || self.s.sta.state != Station::Assoc {
                let c = self.s.count;
                log!("no auth answer: ", c.frames, " frames heard, ", c.sent, " sent, ", c.send_errors, " send errors");
                continue;
            }
            if !self.step(Station::Assoc, build_assoc) || self.s.sta.state < Station::WaitM1 {
                why = "It turned the unit away";
                continue;
            }
            // Open: associated is joined. (The station now waits for WPA2's first key message, which won't come.)
            if self.s.open_net {
                return None;
            }
            let t0 = self.s.now_ms();
            while self.s.sta.state != Station::Connected
                && self.s.sta.state != Station::Failed
                && self.s.now_ms().wrapping_sub(t0) < HANDSHAKE_MS
            {
                self.receive(24, &mut on_join_rx);
                app::feed();
            }
            log!("join attempt ", attempt, ": state ", self.s.sta.state as u32);
            if self.s.sta.state == Station::Connected {
                return None;
            }
            // The AP checks our M2 with the key from the password: with the wrong one, it never sends M3.
            why = if self.s.sta.state == Station::WaitM3 { "Wrong password" } else { "It didn't start the handshake" };
        }
        // Associated, perhaps, but not joined: say so, rather than leave the router waiting.
        self.send(|s, b| s.sta.build_deauth(b));
        Some(why)
    }

    /// Joins and asks DHCP for an address: None, or why not, in words. status is told each step as it starts, and r
    /// says how far it got. Whether it's open or WPA2 is what the network says when heard; pass is ignored for an open
    /// one.
    pub fn join(&mut self, ssid: &[u8], pass: &[u8], mut status: impl FnMut(u8), r: &mut Joined) -> Option<&'static str> {
        *r = Joined::default();
        let mut at = |r: &mut Joined, step: u8| {
            r.step = step;
            status(step);
        };
        at(r, STEP_RADIO);
        self.start();
        self.leave();
        let began = self.s.now_ms();
        self.s.found = Ap::new();
        self.s.found.rssi = -128;
        self.s.answers = 0;
        at(r, STEP_FINDING);
        wifi::mac_wr(wifi::RX_FILTER, wifi::RX_FILTER_ALL);
        let mut pass_n = 0;
        while pass_n < FIND_PASSES && self.s.found.rssi == -128 {
            for ch in 1..=13 {
                wifi::set_channel(ch);
                self.probe(ssid);
                self.listen(ch, &mut |s, _, m, rssi| on_find(s, ssid, m, rssi));
            }
            pass_n += 1;
        }
        let found = self.s.found;
        if found.rssi == -128 {
            return Some("Not heard here");
        }
        r.rssi = found.rssi;
        r.channel = found.channel;
        log!(ssid, ": channel ", found.channel, ", ", i32::from(found.rssi), " dBm, security ", found.security, ", ",
            self.s.answers, " probe responses");
        if found.security == OTHER {
            return Some("Not WPA2: can't join it");
        }
        self.s.open_net = found.security == OPEN;
        if !self.s.open_net && pass.is_empty() {
            return Some("It needs a password");
        }
        wifi::set_channel(u32::from(found.channel));
        self.s.pmk = [0; 32];
        if !self.s.open_net {
            at(r, STEP_KEY);
            sha1hw::check();
            let t0 = self.s.now_ms();
            let s = &mut *self.s;
            s.job.begin(pass, ssid);
            while !s.job.step(256, &mut s.pmk) {
                app::feed();
            }
            log!("key made in ", self.s.now_ms().wrapping_sub(t0), " ms");
        }
        at(r, STEP_JOINING);
        wifi_sta::clear_hw_keys();
        if let Some(why) = self.join_found(ssid) {
            return Some(why);
        }
        self.s.joined = true;
        let t0 = self.s.now_ms();
        while self.s.now_ms().wrapping_sub(t0) < 300 {
            self.receive(24, &mut on_join_rx);
        }
        wifi::mac_wr(wifi::RX_FILTER, wifi::RX_FILTER_STA);
        at(r, STEP_ADDRESS);
        let s = &mut *self.s;
        s.ip = WifiIp::new(s.mac, found.bssid, Some(rand_bytes));
        s.ip.protect = !s.open_net;
        s.count = NO_COUNT;
        self.send(|s, b| s.ip.dhcp_start(b));
        self.s.ip.dhcp_t0 = self.s.now_us();
        let t0 = self.s.now_ms();
        while self.s.ip.ip == 0 {
            self.receive(8, &mut on_data_rx);
            self.send(|s, b| {
                let now = s.now_us();
                s.ip.poll(now, b)
            });
            if self.s.now_ms().wrapping_sub(t0) > DHCP_MS {
                let (c, ip) = (self.s.count, &self.s.ip);
                log!("no address: ", c.frames, " frames, ", c.data, " data, ", c.ours, " ours, ", c.ip, " to IP; ", c.sent,
                    " sent, ", c.send_errors, " send errors; DHCP state ", ip.dhcp_state as u32, ", ", ip.dhcp_retries,
                    " tries");
                return Some("Joined, but no address");
            }
            app::feed();
        }
        let ms = self.s.now_ms().wrapping_sub(began);
        let ip = &self.s.ip;
        (r.ip, r.gateway, r.dns, r.ms, r.step) = (ip.ip, ip.gw, ip.dns, ms, STEPS);
        log!("address ", Hex(ip.ip, 8), " gateway ", Hex(ip.gw, 8), " dns ", Hex(ip.dns, 8), ", ", ms, " ms");
        None
    }

    /// Leaves the network joined, if any.
    pub fn leave(&mut self) {
        if self.s.joined {
            self.send(|s, b| s.sta.build_deauth(b));
            wifi_sta::clear_hw_keys();
        }
        self.s.joined = false;
    }

    /// Leaves it, and turns the radio off as it was found: before an app returns, as what runs next doesn't expect it
    /// on. The next scan or join starts it again.
    pub fn off(&mut self) {
        if !self.s.started {
            return;
        }
        self.leave();
        let found = &self.s.found_regs;
        // The RF paths and BT off first, then the clock block and the analog settings as they were.
        wr(WL_EN, found.en);
        sync_cpu();
        wr(WL_28, found.w28);
        for (i, &v) in found.clock.iter().enumerate() {
            wr(WL_CLOCK + 4 * i as u32, v);
        }
        for (i, &v) in found.analog.iter().enumerate() {
            wr(WL_ANALOG + 4 * i as u32, v);
        }
        sync_cpu();
        self.s.started = false;
        log!("radio off");
    }
}

/// on_data and on_join, as receive() hands frames on.
fn on_data_rx(s: &mut State, tx: &mut Tx, f: &[u8], _rssi: i8) {
    on_data(s, tx, f);
}

fn on_join_rx(s: &mut State, tx: &mut Tx, f: &[u8], _rssi: i8) {
    on_join(s, tx, f);
}
