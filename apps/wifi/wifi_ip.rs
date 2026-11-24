// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
// Copyright (C) 2026 Henri David (hdavid), Optimist (github.com/w0ts/optimist)
//! A small IP layer on the FM-1's Wi-Fi, a station's (no TCP, no DNS):
//! - ARP: answering for our address, and the addresses of those that send us one
//! - DHCP client state machine (Discover -> Offer -> Request -> Ack / Bound)
//! - UDP: up to PORTS ports bound, their datagrams (unicast, broadcast) to one receive hook, and datagrams sent
//! - an early drop filter right after the RX DMA
//!
//! From Optimist's wifi/wifi_ip.c, without its access point, ad-hoc and multicast (IGMP) modes:
//! apps/wifi/bsp/README.md. Taken here under MPL-2.0. Changed by m-apps: in Rust, on slices. A frame whose UDP length
//! says more than it holds is refused (-1) here too, where the C relied on frames.rs's check having been made.

pub const DHCP_PORT_SERVER: u16 = 67;
pub const DHCP_PORT_CLIENT: u16 = 68;
/// UDP ports bound at once.
pub const PORTS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
pub enum Dhcp {
    Idle = 0,
    Discovering,
    Requesting,
    Bound,
    Failed,
}

#[derive(Clone, Copy)]
pub struct ArpEntry {
    /// Host order.
    pub ip: u32,
    pub mac: [u8; 6],
    pub valid: bool,
}

/// A datagram to a bound port (unicast to us, or broadcast), with its arrival time: ctx, now_us, src_ip, src_port,
/// dst_ip, dst_port, the payload.
pub type UdpRx = fn(usize, i64, u32, u16, u32, u16, &[u8]);
/// Random bytes.
pub type Rand = fn(&mut [u8]);

pub struct WifiIp {
    /// Our station MAC.
    pub mac: [u8; 6],
    /// AP BSSID.
    pub bssid: [u8; 6],
    /// Our IPv4 address (host order), 0 = unassigned.
    pub ip: u32,
    /// Subnet mask (host order).
    pub netmask: u32,
    /// Gateway IP (host order).
    pub gw: u32,
    /// DNS server (host order).
    pub dns: u32,
    /// DHCP server IP (host order).
    pub dhcp_server_ip: u32,
    /// DHCP offered IP (host order).
    pub dhcp_offered_ip: u32,
    /// DHCP transaction ID.
    pub dhcp_xid: u32,
    pub dhcp_state: Dhcp,
    /// Host time (us) of current state.
    pub dhcp_t0: i64,
    pub dhcp_retries: u8,
    /// The UDP ports bound (0: a free slot).
    pub ports: [u16; PORTS],
    pub udp_rx: Option<UdpRx>,
    /// For udp_rx.
    pub ctx: usize,
    pub arp_tbl: [ArpEntry; 4],
    /// 802.11 sequence counter.
    pub tx_seq: u16,
    /// True: our data frames carry the Protected bit (keys installed: the MAC encrypts).
    pub protect: bool,
    pub rand: Option<Rand>,
}

fn write_u16_be(p: &mut [u8], v: u16) {
    p[..2].copy_from_slice(&v.to_be_bytes());
}

fn write_u32_be(p: &mut [u8], v: u32) {
    p[..4].copy_from_slice(&v.to_be_bytes());
}

fn read_u16_be(p: &[u8]) -> u16 {
    u16::from_be_bytes([p[0], p[1]])
}

fn read_u32_be(p: &[u8]) -> u32 {
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}

fn ip_chksum(buf: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut pairs = buf.chunks_exact(2);
    for p in &mut pairs {
        sum += u32::from(p[0]) << 8 | u32::from(p[1]);
    }
    if let [last] = pairs.remainder() {
        sum += u32::from(*last) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

impl WifiIp {
    pub const fn new(sta_mac: [u8; 6], bssid: [u8; 6], rand: Option<Rand>) -> WifiIp {
        WifiIp {
            mac: sta_mac,
            bssid,
            ip: 0,
            netmask: 0,
            gw: 0,
            dns: 0,
            dhcp_server_ip: 0,
            dhcp_offered_ip: 0,
            dhcp_xid: 0,
            dhcp_state: Dhcp::Idle,
            dhcp_t0: 0,
            dhcp_retries: 0,
            ports: [0; PORTS],
            udp_rx: None,
            ctx: 0,
            arp_tbl: [ArpEntry { ip: 0, mac: [0; 6], valid: false }; 4],
            tx_seq: 0,
            protect: false,
            rand,
        }
    }

    fn arp_cache_insert(&mut self, ip: u32, mac: &[u8]) {
        if ip == 0 {
            return;
        }
        if let Some(e) = self.arp_tbl.iter_mut().find(|e| e.valid && e.ip == ip) {
            e.mac.copy_from_slice(&mac[..6]);
            return;
        }
        // A free one, or simply the first one overwritten.
        let at = self.arp_tbl.iter().position(|e| !e.valid).unwrap_or(0);
        let e = &mut self.arp_tbl[at];
        e.ip = ip;
        e.mac.copy_from_slice(&mac[..6]);
        e.valid = true;
    }

    fn arp_lookup(&self, ip: u32) -> Option<[u8; 6]> {
        self.arp_tbl.iter().find(|e| e.valid && e.ip == ip).map(|e| e.mac)
    }

    pub fn bound(&self, port: u16) -> bool {
        port != 0 && self.ports.contains(&port)
    }

    /// Binds a UDP port (its datagrams to udp_rx): false with no free slot, or port 0. unbind lets it go.
    #[allow(dead_code)]
    pub fn bind(&mut self, port: u16) -> bool {
        if port == 0 {
            return false;
        }
        if self.bound(port) {
            return true;
        }
        match self.ports.iter_mut().find(|p| **p == 0) {
            Some(p) => {
                *p = port;
                true
            }
            None => false,
        }
    }

    #[allow(dead_code)]
    pub fn unbind(&mut self, port: u16) {
        for p in self.ports.iter_mut().filter(|p| **p == port) {
            *p = 0;
        }
    }

    /// Fast early drop filter (Optimist's docs/LINK-DESIGN.md §2.7). Call immediately after RX DMA (or after
    /// decryption). True to keep and process the frame, false to drop it now.
    pub fn early_filter(&self, frame: &[u8]) -> bool {
        let (our_ip, len) = (self.ip, frame.len());
        // Must be 802.11 Data frame.
        if len < 24 + 8 || frame[0] & 0x0C != 0x08 {
            return false;
        }
        // QoS Data vs Data header length.
        let hlen = if frame[0] & 0x80 != 0 { 26 } else { 24 };
        if len < hlen + 8 {
            return false;
        }
        let llc = &frame[hlen..];
        // Verify LLC/SNAP prefix AA-AA-03-00-00-00.
        if llc[..6] != [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00] {
            return false;
        }
        match read_u16_be(&llc[6..]) {
            // 1. EAPOL
            0x888E => true,
            // 2. ARP
            0x0806 => {
                if len < hlen + 8 + 28 {
                    return false;
                }
                let tip = read_u32_be(&llc[8 + 24..]);
                our_ip == 0 || tip == our_ip || tip == 0xFFFF_FFFF
            }
            // 3. IPv4
            0x0800 => {
                if len < hlen + 8 + 20 {
                    return false;
                }
                let ip_pkt = &llc[8..];
                if ip_pkt[0] >> 4 != 4 {
                    return false;
                }
                // UDP (Protocol 17)
                if ip_pkt[9] == 17 {
                    let ihl = usize::from(ip_pkt[0] & 0x0F) * 4;
                    if len < hlen + 8 + ihl + 8 {
                        return false;
                    }
                    let udp = &ip_pkt[ihl..];
                    let dport = read_u16_be(&udp[2..]);
                    let dst_ip = read_u32_be(&ip_pkt[16..]);
                    // DHCP Client (Port 68)
                    if dport == DHCP_PORT_CLIENT {
                        return true;
                    }
                    // A bound port: to us, or broadcast.
                    if self.bound(dport) && (dst_ip == our_ip || our_ip == 0 || dst_ip == 0xFFFF_FFFF) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn build_80211_data_hdr(&mut self, da: &[u8; 6], eth_type: u16, tx: &mut [u8]) -> usize {
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        tx[..32].fill(0);
        // Type: Data, Subtype: Data
        tx[0] = 0x08;
        // ToDS = 1, FromDS = 0; Protected once keys are in.
        tx[1] = 0x01 | if self.protect { 0x40 } else { 0 };
        // Addr 1: RA (AP BSSID)
        tx[4..10].copy_from_slice(&self.bssid);
        // Addr 2: TA/SA (Our Station MAC)
        tx[10..16].copy_from_slice(&self.mac);
        // Addr 3: DA (Destination MAC)
        tx[16..22].copy_from_slice(da);
        tx[22] = (seq << 4) as u8;
        tx[23] = (seq >> 4) as u8;
        // The MAC encrypts (CCMP) a frame with the Protected bit, by the WCID's key in its table: it's handed the
        // plaintext 802.11 frame, with no room for the CCMP header, which the MAC adds.
        // LLC/SNAP Header (8 bytes): plaintext the MAC encrypts.
        tx[24..30].copy_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00]);
        write_u16_be(&mut tx[30..], eth_type);
        32
    }

    fn get_dst_mac(&self, dst_ip: u32) -> [u8; 6] {
        if dst_ip == 0xFFFF_FFFF {
            return [0xFF; 6];
        }
        if let Some(mac) = self.arp_lookup(dst_ip) {
            return mac;
        }
        if self.gw != 0
            && let Some(mac) = self.arp_lookup(self.gw)
        {
            return mac;
        }
        // Fallback: AP BSSID.
        self.bssid
    }

    #[allow(clippy::too_many_arguments)]
    fn build_udp(&mut self, dst_mac: &[u8; 6], dst_ip: u32, src_port: u16, dst_port: u16, payload: &[u8], tx: &mut [u8]) -> u32 {
        let off = self.build_80211_data_hdr(dst_mac, 0x0800, tx);
        let ip_len = (20u16 + 8).wrapping_add(payload.len() as u16);
        let tx_seq = self.tx_seq;
        let ip_our = self.ip;
        let ip_hdr = &mut tx[off..off + 20];
        ip_hdr.fill(0);
        // IPv4, IHL 5; DSCP/ECN
        ip_hdr[0] = 0x45;
        ip_hdr[1] = 0x00;
        write_u16_be(&mut ip_hdr[2..], ip_len);
        // Identification
        write_u16_be(&mut ip_hdr[4..], tx_seq);
        // Flags: DF
        write_u16_be(&mut ip_hdr[6..], 0x4000);
        // TTL
        ip_hdr[8] = 64;
        // Protocol: UDP
        ip_hdr[9] = 17;
        // Source IP
        write_u32_be(&mut ip_hdr[12..], ip_our);
        // Destination IP
        write_u32_be(&mut ip_hdr[16..], dst_ip);
        let sum = ip_chksum(ip_hdr);
        write_u16_be(&mut ip_hdr[10..], sum);
        let udp_hdr = &mut tx[off + 20..off + 28];
        let udp_len = 8u16.wrapping_add(payload.len() as u16);
        write_u16_be(&mut udp_hdr[0..], src_port);
        write_u16_be(&mut udp_hdr[2..], dst_port);
        write_u16_be(&mut udp_hdr[4..], udp_len);
        // UDP checksum optional
        write_u16_be(&mut udp_hdr[6..], 0);
        tx[off + 28..off + 28 + payload.len()].copy_from_slice(payload);
        (off + 28 + payload.len()) as u32
    }

    /// A UDP datagram from src_port, in 802.11 Data + LLC/SNAP + IPv4 + UDP: the total frame length written to tx. It
    /// goes to the address's MAC if known (ARP), else the gateway's, else the access point's.
    #[allow(dead_code)]
    pub fn send_udp(&mut self, src_port: u16, dst_ip: u32, dst_port: u16, payload: &[u8], tx: &mut [u8]) -> u32 {
        let dst_mac = self.get_dst_mac(dst_ip);
        self.build_udp(&dst_mac, dst_ip, src_port, dst_port, payload, tx)
    }

    fn build_dhcp_msg(&mut self, msg_type: u8, req_ip: u32, srv_ip: u32, tx: &mut [u8]) -> u32 {
        let off = self.build_80211_data_hdr(&[0xFF; 6], 0x0800, tx);
        let (xid, mac) = (self.dhcp_xid, self.mac);
        let (ip_hdr, rest) = tx[off..].split_at_mut(20);
        let (udp_hdr, dhcp) = rest.split_at_mut(8);
        ip_hdr.fill(0);
        ip_hdr[0] = 0x45;
        ip_hdr[8] = 64;
        // UDP
        ip_hdr[9] = 17;
        // 0.0.0.0 to 255.255.255.255
        write_u32_be(&mut ip_hdr[12..], 0);
        write_u32_be(&mut ip_hdr[16..], 0xFFFF_FFFF);
        write_u16_be(&mut udp_hdr[0..], DHCP_PORT_CLIENT);
        write_u16_be(&mut udp_hdr[2..], DHCP_PORT_SERVER);
        write_u16_be(&mut udp_hdr[6..], 0);
        dhcp[..236 + 64].fill(0);
        // Boot Request, Ethernet, HLen 6
        dhcp[0] = 0x01;
        dhcp[1] = 0x01;
        dhcp[2] = 0x06;
        dhcp[3] = 0x00;
        write_u32_be(&mut dhcp[4..], xid);
        // Broadcast flag
        write_u16_be(&mut dhcp[10..], 0x8000);
        dhcp[28..34].copy_from_slice(&mac);
        // Magic Cookie (0x63, 0x82, 0x53, 0x63)
        dhcp[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        let mut opt_len = 240;
        let mut put = |b: &[u8]| {
            dhcp[opt_len..opt_len + b.len()].copy_from_slice(b);
            opt_len += b.len();
        };
        // Option 53: DHCP Message Type
        put(&[53, 1, msg_type]);
        if msg_type == 1 {
            // Discover: Option 55 Parameter Request List: Subnet Mask, Router, DNS, Domain Name.
            put(&[55, 4, 1, 3, 6, 15]);
        } else if msg_type == 3 {
            // Request: Option 50 (Req IP) & Option 54 (Server ID)
            put(&[50, 4]);
            put(&req_ip.to_be_bytes());
            if srv_ip != 0 {
                put(&[54, 4]);
                put(&srv_ip.to_be_bytes());
            }
        }
        // Option 255: End
        put(&[255]);
        // Finalize UDP & IP lengths.
        write_u16_be(&mut udp_hdr[4..], (8 + opt_len) as u16);
        write_u16_be(&mut ip_hdr[2..], (20 + 8 + opt_len) as u16);
        let sum = ip_chksum(ip_hdr);
        write_u16_be(&mut ip_hdr[10..], sum);
        (off + 20 + 8 + opt_len) as u32
    }

    /// Starts DHCP client negotiation: the frame's length written to tx (or 0).
    pub fn dhcp_start(&mut self, tx: &mut [u8]) -> u32 {
        self.dhcp_xid = match self.rand {
            Some(rand) => {
                let mut b = [0u8; 4];
                rand(&mut b);
                u32::from_ne_bytes(b)
            }
            None => 0x1234_5678,
        };
        self.dhcp_state = Dhcp::Discovering;
        self.dhcp_retries = 0;
        self.ip = 0;
        self.build_dhcp_msg(1, 0, 0, tx)
    }

    fn build_arp_reply(&mut self, arp_req: &[u8], tx: &mut [u8]) -> u32 {
        // Sender MAC in req -> Target MAC in rep; sender IP in req -> Target IP in rep.
        let mut target_mac = [0u8; 6];
        target_mac.copy_from_slice(&arp_req[8..14]);
        let target_ip = read_u32_be(&arp_req[14..]);
        let off = self.build_80211_data_hdr(&target_mac, 0x0806, tx);
        let arp_rep = &mut tx[off..off + 28];
        // HW Type: Ethernet
        write_u16_be(&mut arp_rep[0..], 0x0001);
        // Proto Type: IPv4
        write_u16_be(&mut arp_rep[2..], 0x0800);
        // HW Len, Proto Len
        arp_rep[4] = 6;
        arp_rep[5] = 4;
        // Opcode: Reply
        write_u16_be(&mut arp_rep[6..], 0x0002);
        // Sender MAC: Our MAC
        arp_rep[8..14].copy_from_slice(&self.mac);
        // Sender IP: Our IP
        write_u32_be(&mut arp_rep[14..], self.ip);
        // Target MAC
        arp_rep[18..24].copy_from_slice(&target_mac);
        // Target IP
        write_u32_be(&mut arp_rep[24..], target_ip);
        (off + 28) as u32
    }

    /// Processes an incoming 802.11 Data frame (LLC/SNAP encapsulated ARP, IPv4/UDP). Delivers the datagrams of bound
    /// ports to udp_rx. If a response frame (ARP reply, DHCP request) is generated, writes it to tx and sets tx_len,
    /// returning 1; returns 0 if consumed, -1 if ignored or in error.
    pub fn rx_frame(&mut self, frame: &[u8], now_us: i64, tx: &mut [u8], tx_len: &mut u32) -> i32 {
        let len = frame.len();
        if len < 32 {
            return -1;
        }
        let hlen = if frame[0] & 0x80 != 0 { 26 } else { 24 };
        if len < hlen + 8 {
            return -1;
        }
        let llc = &frame[hlen..];
        if llc[..3] != [0xAA, 0xAA, 0x03] {
            return -1;
        }
        let eth_type = read_u16_be(&llc[6..]);

        // 1. ARP (0x0806)
        if eth_type == 0x0806 {
            if len < hlen + 8 + 28 {
                return -1;
            }
            let arp = &llc[8..];
            let (op, sip, tip) = (read_u16_be(&arp[6..]), read_u32_be(&arp[14..]), read_u32_be(&arp[24..]));
            // Update ARP cache with sender.
            self.arp_cache_insert(sip, &arp[8..14]);
            // If ARP request for our IP, reply.
            if op == 1 && self.ip != 0 && tip == self.ip {
                *tx_len = self.build_arp_reply(arp, tx);
                return 1;
            }
            return 0;
        }

        // 2. IPv4 (0x0800)
        if eth_type == 0x0800 {
            let ip_hdr = &llc[8..];
            if len < hlen + 8 + 20 || ip_hdr[0] >> 4 != 4 {
                return -1;
            }
            let ihl = usize::from(ip_hdr[0] & 0x0F) * 4;
            let ip_tot_len = usize::from(read_u16_be(&ip_hdr[2..]));
            let proto = ip_hdr[9];
            let src_ip = read_u32_be(&ip_hdr[12..]);
            if len < hlen + 8 + ihl || ip_tot_len < ihl {
                return -1;
            }
            // UDP Dispatch
            if proto == 17 {
                if len < hlen + 8 + ihl + 8 {
                    return -1;
                }
                let udp = &ip_hdr[ihl..];
                let (src_port, dst_port, udp_len) = (read_u16_be(udp), read_u16_be(&udp[2..]), read_u16_be(&udp[4..]));
                if udp_len < 8 {
                    return -1;
                }
                let payload_len = usize::from(udp_len) - 8;

                // DHCP Response (Port 68)
                if dst_port == DHCP_PORT_CLIENT && payload_len >= 240 {
                    // (The C read it trusting the length frames.rs had checked.)
                    let Some(payload) = udp.get(8..8 + payload_len) else { return -1 };
                    let xid = read_u32_be(&payload[4..]);
                    let yiaddr = read_u32_be(&payload[16..]);
                    let (mut msg_type, mut server_id, mut netmask, mut gw, mut dns) = (0u8, 0u32, 0u32, 0u32, 0u32);
                    if xid != self.dhcp_xid {
                        return 0;
                    }
                    // Parse DHCP options after magic cookie.
                    if payload[236..240] == [0x63, 0x82, 0x53, 0x63] {
                        let mut o = 240;
                        while o < payload_len && payload[o] != 255 {
                            let code = payload[o];
                            o += 1;
                            if code == 0 {
                                continue;
                            }
                            if o >= payload_len {
                                break;
                            }
                            let olen = usize::from(payload[o]);
                            o += 1;
                            if o + olen > payload_len {
                                break;
                            }
                            let v = &payload[o..];
                            match code {
                                53 if olen >= 1 => msg_type = v[0],
                                54 if olen >= 4 => server_id = read_u32_be(v),
                                1 if olen >= 4 => netmask = read_u32_be(v),
                                3 if olen >= 4 => gw = read_u32_be(v),
                                6 if olen >= 4 => dns = read_u32_be(v),
                                _ => {}
                            }
                            o += olen;
                        }
                    }
                    if self.dhcp_state == Dhcp::Discovering && msg_type == 2 {
                        // Offer
                        self.dhcp_t0 = now_us;
                        self.dhcp_offered_ip = yiaddr;
                        self.dhcp_server_ip = server_id;
                        self.dhcp_state = Dhcp::Requesting;
                        *tx_len = self.build_dhcp_msg(3, yiaddr, server_id, tx);
                        return 1;
                    }
                    if self.dhcp_state == Dhcp::Requesting && msg_type == 5 {
                        // Ack
                        self.ip = yiaddr;
                        self.netmask = if netmask != 0 { netmask } else { 0xFFFF_FF00 };
                        self.gw = gw;
                        self.dns = dns;
                        self.dhcp_state = Dhcp::Bound;
                        return 0;
                    }
                    return 0;
                }

                // A bound port's datagram.
                if let Some(udp_rx) = self.udp_rx
                    && self.bound(dst_port)
                {
                    let dst_ip = read_u32_be(&ip_hdr[16..]);
                    if payload_len > ip_tot_len.wrapping_sub(ihl).wrapping_sub(8) || len < hlen + 8 + ihl + 8 + payload_len {
                        return -1;
                    }
                    udp_rx(self.ctx, now_us, src_ip, src_port, dst_ip, dst_port, &udp[8..8 + payload_len]);
                    return 0;
                }
            }
        }
        0
    }

    /// Periodic timer polling (DHCP timeouts / retransmissions): the frame's length written to tx if one is to go.
    pub fn poll(&mut self, now_us: i64, tx: &mut [u8]) -> u32 {
        if self.dhcp_state == Dhcp::Discovering || self.dhcp_state == Dhcp::Requesting {
            // (An answer is awaited 2 s, then the Discover again.)
            if now_us - self.dhcp_t0 < 2_000_000 {
                return 0;
            }
            self.dhcp_t0 = now_us;
            self.dhcp_retries += 1;
            if self.dhcp_retries > 5 {
                self.dhcp_state = Dhcp::Failed;
                return 0;
            }
            self.dhcp_state = Dhcp::Discovering;
            return self.build_dhcp_msg(1, 0, 0, tx);
        }
        0
    }
}
