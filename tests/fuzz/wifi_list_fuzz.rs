// SPDX-License-Identifier: MIT
//! libFuzzer: apps/wifi's WIFI.TXT reader and writer on any text. Reading must stay inside it; writing it back, after
//! changes the input picks, must give a file that reads as the list it was written from.
#![no_main]
#[macro_use]
#[path = "target.rs"]
mod target;
#[allow(dead_code)]
#[path = "../../apps/wifi/wifilist.rs"]
mod wifilist;
use wifilist::{List, MOST, SSID_MOST, TEXT_MOST, until_zero};

const CHANGES: usize = 4;

/// b, read back, is a, in the same order. When the file held more networks than fit (full), the reader may now take one
/// it didn't before, one changed in place can come back earlier, and a new one, at the end of the file, may be past the
/// 12 it takes (docs/WIFI-TXT.md): each of a's from the file is there, as it was.
fn same_list(a: &List, b: &List, full: bool) {
    assert!(full || b.count == a.count);
    for (i, n) in a.net[..a.count].iter().enumerate() {
        let j = if full { b.find(n.ssid()) } else { Some(i) };
        if full && j.is_none() && !n.in_file {
            continue;
        }
        let Some(j) = j else { panic!() };
        assert!(n.ssid() == b.net[j].ssid() && n.pass() == b.net[j].pass());
    }
    if a.count != 0 && (!full || b.find(a.net[a.in_use].ssid()).is_some()) {
        assert!(a.net[a.in_use].ssid() == b.net[b.in_use].ssid());
    }
}

/// The k-th ssid= line's value in text, whether the reader took it or not: names to put that the file has.
fn nth_name(text: &[u8], k: usize) -> Option<&[u8]> {
    let lines = text.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    lines.filter_map(|l| l.strip_prefix(b"ssid=")).filter(|v| v.len() <= SSID_MOST).nth(k)
}

/// The list's state as the reader and every change must leave it.
fn sound(l: &List) {
    assert!(l.count <= MOST && (l.count == 0 || l.in_use < l.count));
}

fuzz!(|data| {
    // The last CHANGES bytes pick the changes; the rest is the file.
    if data.len() < CHANGES || data.len() - CHANGES > TEXT_MOST {
        return;
    }
    let (text, picks) = data.split_at(data.len() - CHANGES);
    let mut l = Box::new(List::new());
    l.read(text);
    sound(&l);
    let full = l.count == MOST && l.skipped != 0;
    for &pick in picks {
        let k = usize::from(pick >> 3);
        let pass: &[u8] = if pick & 4 != 0 { b"" } else { b"fuzz-pass-1" };
        match pick % 6 {
            // remove one
            1 if l.count != 0 => {
                let i = k % l.count;
                l.remove(i);
            }
            // a password changed
            2 if l.count != 0 => {
                let name = l.net[k % l.count].ssid;
                l.put(until_zero(&name), pass);
            }
            // a new one
            3 => {
                l.put(if k & 1 != 0 { b"fuzz net" } else { b"another" }, pass);
            }
            // a name the file has, taken or skipped
            4 => {
                if let Some(name) = nth_name(text, k % 16) {
                    l.put(until_zero(name), pass);
                }
            }
            // removed, and put back
            5 if l.count != 0 => {
                let i = k % l.count;
                let name = l.net[i].ssid;
                l.remove(i);
                l.put(until_zero(&name), pass);
            }
            _ => {}
        }
        sound(&l);
    }
    let mut out = vec![0u8; TEXT_MOST * 2];
    if let Some(len) = l.write(text, &mut out) {
        let mut again = Box::new(List::new());
        again.read(&out[..len]);
        same_list(&l, &again, full);
    }
});
