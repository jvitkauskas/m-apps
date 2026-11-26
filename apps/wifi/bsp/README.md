# From w0ts/fm1-base

The radio's start-up and WiFi path (`bsp/`) and the WPA2 station, its crypto and the IP layer (`wifi_sta`,
`wifi_crypto`, `wifi_ip`), cut down as below, from [w0ts/fm1-base](https://github.com/w0ts/fm1-base) at
`0f36ac350a59f8c857d0e96b0e33995d0bf88d09` and its `bsp` submodule
([hugelton/felucca-bsp](https://github.com/hugelton/felucca-bsp)) at `d83230fbdd2d36794024e5a37008258fe63ac7d1`.
`ble_rf_tables.rs` is `chip/ble_rf_tables_v15.h`'s values.

Licences, as their headers say: MPL-2.0; "GPL-3.0-only OR MPL-2.0", taken here under MPL-2.0; and Apache-2.0 for
`fm1_rf_cal.rs`, `fm1_wifi_wlc.rs` and `ble_rf_tables.rs` (from JieLi's SDK). Copyright (C) 2026 Henri David
(hdavid), Optimist (github.com/w0ts/optimist); Leo Kuroshita (@kurogedelic), Hügelton Instruments; Charles Vestal;
Zhuhai Jieli Technology Co., Ltd.; Zhuhai Shengke / M-VAVE: each file's header says which.

## What's changed here

In Rust, from m-apps' C of these files (in its history up to the Rust port), which was upstream's cut down. Every
register write, its order and its delays are the C's; the C's statics are one struct each (`fm1_ble_rf::Rf`, with the
calibration's `Cal`; `net::State`), and the tables are the same values as Rust statics (`tools/c_tables.py`).
`fm1_cc.rs` reaches the radio's registers by address, checking each is one of the chip's (0x10000-0x3FFFF).

Kept small, for a station that sets up WiFi and, next, a store that fetches over HTTP: what neither uses is cut. To
take a newer upstream, start from its files and cut the same again; what was cut is in this repo's history (the
commit "wifi: only what a station uses of the copied stack").

- **`wifi_crypto.rs`:** its SHA-1 block function is `sha1_block_sw()`, and on the unit `sha1_block()` first offers
  each block to the chip's SHA unit (`sha1hw.rs`), as the C's `WC_SHA1_HW` hook did, marked "m-apps". Cut:
  `wc_pmk()` (the PMK in one go: the sliced job is used), AES encryption, the AES key wrap and the software CCMP (an
  access point's, and the host's tests').
- **`wifi_ip.rs`:** the station's side only. Cut: the access point and ad-hoc modes (FromDS and IBSS frames, the DHCP
  server's hook, the ping reply), IGMP and multicast, `wifi_ip_reply_udp()`, `wifi_ip_arp_learn()`, the `on_addr`
  and `send` hooks. Kept for the store: ARP, the DHCP client, UDP ports, `send_udp()`.
- **`wifi_sta.rs`:** cut `wifi_sta_set_creds()` and `wifi_sta_get_mac()` (the MAC comes from the flash's unique ID,
  `net.rs`). On a computer its MAC writes go nowhere, as the C's fuzz test had them.
- **`bsp/fm1_ble.rs`:** the radio's bring-up only. Cut: the BLE baseband, its link-column port and interrupts.
- **`bsp/fm1_ble_rf.rs`:** cut the breadcrumb (a `.noinit` record for felucca's console, written on every start-up
  op) and `fm1_rf_cal_wifi_iq()`.
- **`bsp/fm1_rf_cal.rs`:** without `FM1_RF_CAL_FULL`'s TX DC, gain and phase searches, which the C built only on
  request.
- **`bsp/fm1_irq.rs`:** only the global interrupt enable the start-up uses.
- **`bsp/fm1_wifi.rs`, `bsp/fm1_time.rs`:** cut helpers nothing calls.
- **Left out:** `fm1_sys.h` and `fm1_cpu1_stop.h` (firmware's P33, watchdog and second core).
