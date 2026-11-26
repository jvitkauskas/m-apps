// SPDX-License-Identifier: MIT
//! Draws the WiFi app's screens on a computer, from the same code, into PPM files: wifi_screens_host OUT_DIR.
//!
//! The canvas only draws into its picture in memory, which this saves: every screen marks all it draws, so it's what
//! the panel would show.
#[allow(dead_code)]
#[path = "../apps/wifi/network.rs"]
mod network;
#[allow(dead_code)]
#[path = "../apps/wifi/screens.rs"]
mod screens;
#[allow(dead_code)]
#[path = "../apps/wifi/wifilist.rs"]
mod wifilist;

use kit::canvas::{self, Canvas, DIM, GREEN, RED};
use kit::textentry;
use network::{Ap, Joined, OPEN, OTHER, STEP_FINDING, STEP_JOINING, STEP_KEY, STEPS, WPA2};
use screens::{ACTION_DELETE, ACTION_PASSWORD, ACTION_TEST, ACTION_USE};
use std::io::Write;
use wifilist::List;

fn save(c: &mut Canvas, dir: &str, name: &str, saved: &mut u32) {
    let mut f = std::fs::File::create(format!("{dir}/{name}.ppm")).unwrap_or_else(|_| std::process::exit(2));
    let mut out = b"P6 240 240 255\n".to_vec();
    for row in &c.fb {
        for &c in row {
            out.extend_from_slice(&[
                ((c >> 11) << 3 | (c >> 13)) as u8,
                (((c >> 5) & 63) << 2 | ((c >> 9) & 3)) as u8,
                ((c & 31) << 3 | ((c & 31) >> 2)) as u8,
            ]);
        }
    }
    f.write_all(&out).unwrap_or_else(|_| std::process::exit(2));
    c.forget();
    *saved += 1;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        std::process::exit(2);
    }
    let dir = &args[1];
    // Safety: the only use of it.
    let c = unsafe { canvas::claim() };
    let mut saved = 0;
    let mut l = List::new();
    l.put(b"Home", b"correct horse");
    l.put(b"Workshop with a name too long to fit", b"shed-key-2041");
    l.put(b"Cafe", b"");
    l.in_use = 0;
    let heard: [(&[u8], i8, u8, u8); 5] = [
        (b"Home", -42, 1, WPA2),
        (b"Neighbours 5G", -61, 6, WPA2),
        (b"Free Cafe WiFi", -70, 11, OPEN),
        (b"Office WPA3", -78, 3, OTHER),
        (b"Far away", -88, 13, WPA2),
    ];
    let mut aps = [Ap::new(); 5];
    for (ap, &(ssid, rssi, channel, security)) in aps.iter_mut().zip(&heard) {
        ap.ssid[..ssid.len()].copy_from_slice(ssid);
        (ap.rssi, ap.channel, ap.security) = (rssi, channel, security);
    }
    let ok = Joined { rssi: -46, channel: 1, ip: 0xc0a8_01b5, gateway: 0xc0a8_0101, dns: 0xc0a8_0101, ms: 6230, step: STEPS };
    let heard_only = Joined { rssi: -71, channel: 6, step: STEP_JOINING, ..Joined::default() };
    for board in 1..=2u32 {
        let mut shot = |c: &mut Canvas, what: &str| save(c, dir, &format!("{board}-{what}"), &mut saved);
        screens::list(c, &l, board, 0, b"", 0);
        shot(c, "list-add");
        screens::list(c, &l, board, 2, b"Saved, and in use", GREEN);
        shot(c, "list-second");
        let empty = List::new();
        screens::list(c, &empty, board, 0, b"No WIFI.TXT yet", DIM);
        shot(c, "list-empty");
        let actions = [ACTION_TEST, ACTION_USE, ACTION_PASSWORD, ACTION_DELETE];
        screens::network(c, &l, 1, board, &actions, 0, 0, b"", 0);
        shot(c, "network");
        screens::network(c, &l, 1, board, &actions, 3, 60, b"", 0);
        shot(c, "network-delete");
        let used = [ACTION_TEST, ACTION_PASSWORD, ACTION_DELETE];
        screens::network(c, &l, 0, board, &used, 0, 0, b"", 0);
        shot(c, "network-used");
        screens::scanning(c, 0, 13);
        screens::scanning(c, 5, 13);
        shot(c, "scanning");
        screens::heard(c, &aps, &l, board, 0);
        shot(c, "heard");
        screens::heard(c, &aps, &l, board, 3);
        shot(c, "heard-other");
        screens::heard(c, &aps, &l, board, 5);
        shot(c, "heard-type");
        screens::heard(c, &aps[..0], &l, board, 0);
        shot(c, "heard-none");
        textentry::all_symbols(true);
        for layer in 0..4 {
            screens::typing(
                c,
                board,
                b"Password",
                b"Neighbours 5G",
                if layer != 0 { b"secret#1" } else { b"" },
                b"8 to 63 characters",
                layer,
                if layer == 2 { 1 << 3 } else { 0 },
                true,
                if layer == 3 { b"At least 8 characters" } else { b"" },
            );
            shot(c, &format!("type-{layer}"));
        }
        screens::test(c, b"Home", STEP_KEY, false, None, &ok, b"Testing", DIM, b"");
        shot(c, "test-running");
        screens::test(c, b"Home", STEPS, true, None, &ok, b"Saved, and in use", GREEN, if board == 2 { b"SEQ PLAY OK" } else { b"PLAY OK" });
        shot(c, "test-ok");
        screens::test(
            c,
            b"Neighbours 5G",
            STEP_JOINING,
            true,
            Some(b"Wrong password"),
            &heard_only,
            if board == 2 { b"SEQ PLAY: save it anyway" } else { b"PLAY: save it anyway" },
            RED,
            if board == 2 { b"SEQ REC EDIT" } else { b"REC EDIT" },
        );
        shot(c, "test-failed");
        screens::typing(c, board, b"Network name", b"", b"My hidden network", b"", textentry::LOWER, 0, false, b"");
        shot(c, "type-name");
        screens::test(c, b"Workshop with a name too long to fit", STEP_FINDING, false, None, &ok, b"Testing", DIM, b"");
        shot(c, "test-long-name");
        screens::test(c, b"Far away", STEP_FINDING, true, Some(b"Not heard here"), &Joined::default(), b"", DIM, b"");
        shot(c, "test-not-heard");
    }
    println!("{saved} screens");
}
