// SPDX-License-Identifier: MIT
//! apps/wifi/wifilist.rs on a computer: reading WIFI.TXT as people write it, and writing it back.
#[allow(dead_code)]
#[path = "../apps/wifi/wifilist.rs"]
mod wifilist;

use std::sync::atomic::{AtomicBool, Ordering};
use wifilist::{List, MOST, PASS_MOST, SSID_MOST, TEXT_MOST, pass_ok, ssid_ok};

static FAILED: AtomicBool = AtomicBool::new(false);

macro_rules! check {
    ($x:expr) => {
        if !($x) {
            println!("{}:{}: {}", file!(), line!(), stringify!($x));
            FAILED.store(true, Ordering::Relaxed);
        }
    };
}

fn read_text(l: &mut List, t: &[u8]) {
    l.read(t);
}

fn has(text: &[u8], what: &[u8]) -> bool {
    text.windows(what.len()).any(|w| w == what)
}

/// The text written, or None.
fn write(l: &List, old: &[u8], most: usize) -> Option<Vec<u8>> {
    let mut to = vec![0u8; most];
    let n = l.write(old, &mut to)?;
    to.truncate(n);
    Some(to)
}

fn same(got: &[u8], want: &[u8]) -> bool {
    if got != want {
        println!("got:\n{}", String::from_utf8_lossy(got));
    }
    got == want
}

fn main() {
    let mut l = List::new();
    // The sample on the starter card (make starter): one network, joinable, and the one in use.
    read_text(&mut l, include_bytes!("../starter/WIFI.TXT"));
    check!(l.count == 1 && l.in_use == 0 && l.skipped == 0);
    check!(l.net[0].ssid() == b"My network" && l.net[0].pass() == b"my password here");
    // As a person writes it: CRLF, a BOM, comments, spaces and = kept in values, use= after the networks.
    read_text(&mut l, b"\xef\xbb\xbf# mine\r\n\r\nssid=Home Net\r\npassword=a = b c d\r\n\r\nssid=Shed\r\npassword=12345678\r\nuse=Shed\r\n");
    check!(l.count == 2 && l.in_use == 1 && l.skipped == 0);
    check!(l.net[0].ssid() == b"Home Net" && l.net[0].pass() == b"a = b c d");
    check!(l.net[1].pass() == b"12345678");
    // No use=, or one naming nothing: the first. An open network. No newline at the end.
    read_text(&mut l, b"ssid=Cafe\nssid=Two\npassword=abcdefgh\nuse=Nowhere");
    check!(l.count == 2 && l.in_use == 0 && l.net[0].pass[0] == 0);
    // What can't be joined is skipped and counted: a short password, a long name, a password before any name, a name
    // twice, an empty name.
    read_text(
        &mut l,
        b"password=orphaned1\nssid=Short\npassword=1234567\nssid=0123456789012345678901234567890123\n\
          ssid=Ok\npassword=abcdefgh\nssid=Ok\npassword=zzzzzzzz\nssid=\nunknown=thing\n",
    );
    check!(l.count == 1 && l.skipped == 4 && l.net[0].pass() == b"abcdefgh");
    // Too many.
    let mut big = Vec::new();
    for i in 0..MOST + 3 {
        big.extend_from_slice(format!("ssid=net{i}\npassword=password{i}\n").as_bytes());
    }
    l.read(&big);
    check!(l.count == MOST && l.skipped == 3);
    // Writing it back and reading it again gives the same list, with the longest names and passwords too.
    l = List::new();
    for i in 0..MOST {
        let ssid = vec![b'a' + i as u8; SSID_MOST];
        let pass = vec![b'!' + i as u8; PASS_MOST];
        check!(l.put(&ssid, &pass) == Some(i));
    }
    check!(l.put(b"one more", b"abcdefgh").is_none());
    l.in_use = 5;
    let text = write(&l, b"", TEXT_MOST);
    check!(text.as_ref().is_some_and(|t| !t.is_empty()));
    let text = text.unwrap_or_default();
    let mut again = List::new();
    again.read(&text);
    check!(again.count == l.count && again.in_use == 5 && again.skipped == 0);
    for i in 0..l.count {
        check!(again.net[i].ssid() == l.net[i].ssid() && again.net[i].pass() == l.net[i].pass());
    }
    check!(write(&l, b"", 100).is_none());
    // Put changes a password in place and uses it; remove keeps use on the same network, or the first.
    l = List::new();
    l.put(b"A", b"aaaaaaaa");
    l.put(b"B", b"bbbbbbbb");
    l.put(b"C", b"");
    check!(l.count == 3 && l.in_use == 2);
    check!(l.put(b"A", b"AAAAAAAA") == Some(0) && l.in_use == 0 && l.net[0].pass() == b"AAAAAAAA");
    l.in_use = 2;
    l.remove(0);
    check!(l.count == 2 && l.in_use == 1 && l.net[l.in_use].ssid() == b"C");
    l.remove(1);
    check!(l.count == 1 && l.in_use == 0);
    l.remove(0);
    check!(l.count == 0 && l.in_use == 0);
    let text = write(&l, b"", TEXT_MOST);
    check!(text.as_ref().is_some_and(|t| !t.is_empty() && !has(t, b"\nuse=")));
    // Changing a file keeps what it doesn't know: a byte order mark, CRLF, comments, blank lines, a setting it doesn't
    // know, a network it skipped (a short password), and where use= is. A password changes in place, a removed
    // network's settings go and its comment stays, a password the file didn't have goes at the end of its settings,
    // and a new network goes at the end.
    let mine: &[u8] = b"\xef\xbb\xbf# my networks\r\n\r\n# at home\r\nssid=Home\r\npassword=old-home-pw\r\nband=5\r\n\r\n\
        # the shed\r\nssid=Shed\r\npassword=shed-pw-1\r\nhidden=1\r\n\r\nssid=Short\r\npassword=1234\r\n\r\n\
        ssid=Cafe\r\n\r\nuse=Shed\r\n";
    read_text(&mut l, mine);
    check!(l.count == 3 && l.skipped == 1 && l.in_use == 1);
    check!(l.put(b"Home", b"new-home-pw") == Some(0));
    let shed = l.find(b"Shed").unwrap_or(usize::MAX);
    l.remove(shed);
    check!(l.put(b"Cafe", b"cafe-pw-22") == Some(1));
    check!(l.put(b"New", b"") == Some(2));
    let text = write(&l, mine, TEXT_MOST);
    check!(text.is_some());
    let want: &[u8] = b"\xef\xbb\xbf# my networks\r\n\r\n# at home\r\nssid=Home\r\npassword=new-home-pw\r\nband=5\r\n\r\n\
        # the shed\r\n\r\nssid=Short\r\npassword=1234\r\n\r\nssid=Cafe\r\npassword=cafe-pw-22\r\n\r\n\
        use=New\r\n\r\nssid=New\r\npassword=\r\n";
    check!(same(&text.unwrap_or_default(), want));
    // Removed and put back: still where it was, with its new password, in the list too. With no use= in the file, one
    // goes before the first network. A last line with no line end gets one.
    let plain: &[u8] = b"# x\nssid=A\npassword=aaaaaaaa\nssid=B\npassword=bbbbbbbb";
    read_text(&mut l, plain);
    l.remove(0);
    check!(l.put(b"A", b"AAAAAAAA") == Some(0) && l.net[1].ssid() == b"B");
    check!(l.put(b"C", b"cccccccc") == Some(2));
    let text = write(&l, plain, TEXT_MOST);
    check!(text.is_some());
    let text = text.unwrap_or_default();
    check!(same(&text, b"# x\nuse=C\n\nssid=A\npassword=AAAAAAAA\nssid=B\npassword=bbbbbbbb\n\nssid=C\npassword=cccccccc\n"));
    // Read back, it's the same list.
    again.read(&text);
    check!(again.count == 3 && again.in_use == 2 && again.net[0].pass() == b"AAAAAAAA");
    // Adding a network and removing it again leaves the file as it was, but for use=.
    let two: &[u8] = b"use=One\n\nssid=One\npassword=11111111\n\nssid=Two\npassword=22222222\n";
    read_text(&mut l, two);
    l.put(b"Three", b"33333333");
    l.in_use = 0;
    let first = write(&l, two, TEXT_MOST);
    check!(first.is_some());
    let first = first.unwrap_or_default();
    read_text(&mut l, b"");
    l.read(&first);
    check!(l.count == 3 && l.net[2].in_file);
    l.remove(2);
    let text = write(&l, &first, TEXT_MOST);
    check!(text.is_some());
    check!(same(&text.unwrap_or_default(), two));
    // The writer's harder cases. A comment between a network's name and its password: the password changes once, in
    // place. Each rewritten line keeps its own line end.
    let mixed: &[u8] = b"ssid=A\r\n# its password\npassword=aaaaaaaa\r\nuse=A\n";
    read_text(&mut l, mixed);
    check!(l.put(b"A", b"AAAAAAAA") == Some(0));
    let text = write(&l, mixed, TEXT_MOST);
    check!(text.is_some_and(|t| t == b"ssid=A\r\n# its password\npassword=AAAAAAAA\r\nuse=A\n"));
    // Removing the only network of a file with nothing else: nothing is left, and that's no failure.
    let only: &[u8] = b"ssid=A\npassword=aaaaaaaa\n";
    read_text(&mut l, only);
    l.remove(0);
    let mut out = [0u8; TEXT_MOST];
    check!(l.write(only, &mut out) == Some(0));
    // A name twice takes one place: twelve others after it all fit. Removing it removes every network of its name.
    let mut big = b"ssid=dup\npassword=dupdupdup\nssid=dup\npassword=otherdup\n".to_vec();
    for i in 0..MOST - 1 {
        big.extend_from_slice(format!("ssid=net{i}\npassword=password{i}\n").as_bytes());
    }
    l.read(&big);
    check!(l.count == MOST && l.skipped == 1 && l.net[0].pass() == b"dupdupdup");
    l.remove(0);
    let text = write(&l, &big, TEXT_MOST);
    check!(text.is_some_and(|t| !has(&t, b"dup")));
    // A name with a zero byte in it can't be one.
    read_text(&mut l, b"");
    l.read(b"ssid=a\0b\npassword=abcdefgh\n");
    check!(l.count == 0 && l.skipped == 1);
    // A password the file didn't have goes just after its name, even on a last line with no end.
    let bare: &[u8] = b"ssid=Cafe";
    read_text(&mut l, bare);
    check!(l.put(b"Cafe", b"cafe-pw-22") == Some(0));
    let text = write(&l, bare, TEXT_MOST);
    check!(text.is_some());
    check!(same(&text.unwrap_or_default(), b"use=Cafe\n\nssid=Cafe\npassword=cafe-pw-22\n"));
    // A network added with the name of one the reader skipped (its password can't be): that one stays as it was, with
    // its settings, and the new one goes at the end, so the list reads back in the same order.
    let skipped: &[u8] = b"ssid=Home\npassword=short\nband=5\n\nssid=Cafe\npassword=cafe-pw-1\n";
    read_text(&mut l, skipped);
    check!(l.count == 1 && l.skipped == 1);
    check!(l.put(b"Home", b"home-pw-12") == Some(1));
    let text = write(&l, skipped, TEXT_MOST);
    check!(text.is_some());
    let text = text.unwrap_or_default();
    check!(text == b"use=Home\n\nssid=Home\npassword=short\nband=5\n\nssid=Cafe\npassword=cafe-pw-1\n\nssid=Home\npassword=home-pw-12\n");
    again.read(&text);
    check!(again.count == 2 && again.net[0].ssid() == b"Cafe" && again.net[1].pass() == b"home-pw-12");
    // One the reader didn't take as the list was full, but could have: changed where it is, as the next read takes it.
    let mut big = Vec::new();
    for i in 0..MOST {
        big.extend_from_slice(format!("ssid=net{i}\npassword=password{i}\n").as_bytes());
    }
    big.extend_from_slice(b"ssid=late\npassword=late-old-pw\n");
    l.read(&big);
    check!(l.count == MOST && l.skipped == 1);
    l.remove(0);
    check!(l.put(b"late", b"late-new-pw").is_some());
    let text = write(&l, &big, TEXT_MOST);
    check!(text.is_some());
    let text = text.unwrap_or_default();
    again.read(&text);
    let late = again.find(b"late");
    check!(late.is_some_and(|i| again.net[i].pass() == b"late-new-pw") && !has(&text, b"late-old-pw"));
    // A last line with no LF that ends in a lone CR (dropped by the reader): what's added after it doesn't change it,
    // even when the line holds a CR of its own (ssid=w CR CR: the name "w CR", which can't be one).
    let cr: &[u8] = b"ssid=Home\npassword=home-pw-1\nuse=Home\r";
    read_text(&mut l, cr);
    check!(l.put(b"New", b"new-pw-123") == Some(1));
    l.in_use = 0;
    let text = write(&l, cr, TEXT_MOST);
    check!(text.is_some());
    again.read(&text.unwrap_or_default());
    check!(again.count == 2 && again.in_use == 0);
    let crcr: &[u8] = b"ssid=w\r\r";
    read_text(&mut l, crcr);
    check!(l.count == 0 && l.skipped == 1);
    check!(l.put(b"New", b"") == Some(0));
    let text = write(&l, crcr, TEXT_MOST);
    check!(text.is_some());
    again.read(&text.unwrap_or_default());
    check!(again.count == 1 && again.net[0].ssid() == b"New");
    // A password given twice: the last counts, though the first can't be one.
    read_text(&mut l, b"ssid=A\npassword=bad\npassword=good-pass\n");
    check!(l.count == 1 && l.net[0].pass() == b"good-pass");
    // A file too big for out isn't cut short.
    check!(write(&l, plain, 20).is_none());
    // Names and passwords that can't be.
    check!(!ssid_ok(b"") && !ssid_ok(b"a\nb") && ssid_ok(b"caf\xc3\xa9"));
    check!(pass_ok(b"") && !pass_ok(b"1234567") && !pass_ok(b"caf\xc3\xa9 1234") && pass_ok(b"~!@#$%^&*()"));
    let failed = FAILED.load(Ordering::Relaxed);
    println!("{}", if failed { "FAILED" } else { "wifi list OK" });
    std::process::exit(i32::from(failed));
}
