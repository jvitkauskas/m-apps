// SPDX-License-Identifier: MIT
//! The WiFi networks to join, in /WIFI.TXT at the card's root, for any firmware or app on the unit: a text file to edit
//! on a computer or with the WiFi app. docs/WIFI-TXT.md is the format:
//!
//! ```text
//! # WiFi networks on this card. use= is the one to join.
//! use=Home
//!
//! ssid=Home
//! password=correct horse
//!
//! ssid=Workshop
//! password=shed-key-2041
//! ```
//!
//! One setting a line, as name=value: the value is everything after the first =, spaces and all, so a name or
//! password may have = and spaces in it. ssid= starts a network and password= is its password. use= names the network
//! to join; without it, or naming none of them, it's the first. Lines starting with # and blank lines are skipped, and
//! so are settings it doesn't know. Line ends may be LF or CRLF, and a UTF-8 byte order mark at the start is skipped.
//!
//! A name is 1 to 32 bytes and a password 8 to 63 characters (WPA2-PSK), or none for an open network. A name given
//! twice: the first that can be joined counts; a password given twice, the last. Pure data, on byte slices, so it builds
//! on a computer too (tests/wifi_list_test.py, and a fuzzer).

pub const SSID_MOST: usize = 32;
pub const PASS_LEAST: usize = 8;
pub const PASS_MOST: usize = 63;
pub const MOST: usize = 12;
/// The largest file this module reads and changes.
pub const TEXT_MOST: usize = 4096;

const HEAD: &[u8] = b"# WiFi networks on this card. use= is the one to join.\n\
# ssid= starts a network, password= is its password (none for an open one).\n";

/// The bytes before the first zero: the text of a name or password kept with its zero, as the file's reader keeps
/// them.
pub fn until_zero(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |n| &s[..n])
}

#[derive(Clone, Copy)]
pub struct Net {
    /// The name, ending in a zero.
    pub ssid: [u8; SSID_MOST + 1],
    /// The password, ending in a zero; empty for an open network.
    pub pass: [u8; PASS_MOST + 1],
    /// Read from the file, so removing it takes it out of the file.
    pub in_file: bool,
    /// Where its ssid= line starts in the text read, if in_file: a name given twice is the copy read, and the one a
    /// save changes.
    pub at: usize,
}

impl Net {
    pub const fn new() -> Net {
        Net { ssid: [0; SSID_MOST + 1], pass: [0; PASS_MOST + 1], in_file: false, at: 0 }
    }

    pub fn ssid(&self) -> &[u8] {
        until_zero(&self.ssid)
    }

    pub fn pass(&self) -> &[u8] {
        until_zero(&self.pass)
    }
}

impl Default for Net {
    fn default() -> Net {
        Net::new()
    }
}

/// A name removed since the file was read, and where its ssid= line was.
#[derive(Clone, Copy)]
pub struct Removed {
    pub ssid: [u8; SSID_MOST + 1],
    pub at: usize,
}

#[derive(Clone, Copy)]
pub struct List {
    pub net: [Net; MOST],
    pub count: usize,
    /// The one to join; 0 with none.
    pub in_use: usize,
    /// Networks in the file that weren't read: too many, or a name or password that can't be.
    pub skipped: usize,
    /// The names removed since it was read, so that writing drops them from the file and nothing else.
    pub removed: [Removed; MOST],
    pub removed_count: usize,
}

impl Default for List {
    fn default() -> List {
        List::new()
    }
}

/// s as a name or password with its zero, s being one that fits.
fn set(to: &mut [u8], from: &[u8]) {
    to[..from.len()].copy_from_slice(from);
    to[from.len()] = 0;
}

fn printable(s: &[u8]) -> bool {
    !s.iter().any(|&c| c == b'\n' || c == b'\r')
}

/// Whether a name can go in the file and be joined: no line ends, and 1 to 32 bytes. (A zero byte can't be in one:
/// it's where the name ends.)
pub fn ssid_ok(ssid: &[u8]) -> bool {
    !ssid.is_empty() && ssid.len() <= SSID_MOST && printable(ssid) && !ssid.contains(&0)
}

/// Whether a password can: none, for an open network, or 8 to 63 characters of printable ASCII, WPA2's passphrase.
pub fn pass_ok(pass: &[u8]) -> bool {
    let n = pass.len();
    if n == 0 {
        return true;
    }
    if !(PASS_LEAST..=PASS_MOST).contains(&n) {
        return false;
    }
    pass.iter().all(|&c| (32..=126).contains(&c))
}

// ---- the text, a line at a time ----

#[derive(Clone, Copy)]
struct Line {
    /// Where it starts.
    s: usize,
    /// Without its line end.
    len: usize,
    /// With it: LF, CRLF, or none on a last line.
    whole: usize,
}

impl Line {
    fn text<'a>(&self, t: &'a [u8]) -> &'a [u8] {
        &t[self.s..self.s + self.len]
    }

    fn with_end<'a>(&self, t: &'a [u8]) -> &'a [u8] {
        &t[self.s..self.s + self.whole]
    }
}

/// The line starting at *at, and *at moved past it; None at the end.
fn next_line(text: &[u8], at: &mut usize) -> Option<Line> {
    let n = text.len();
    if *at >= n {
        return None;
    }
    let end = text[*at..].iter().position(|&c| c == b'\n').map_or(n, |k| *at + k);
    let mut l = Line { s: *at, len: end - *at, whole: if end < n { end + 1 } else { end } - *at };
    if l.len != 0 && text[l.s + l.len - 1] == b'\r' {
        l.len -= 1;
    }
    *at = if end < n { end + 1 } else { n };
    Some(l)
}

fn bom(text: &[u8]) -> usize {
    if text.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 }
}

fn skipped_line(text: &[u8], l: &Line) -> bool {
    l.len == 0 || text[l.s] == b'#'
}

/// The value of a setting of this name, if the line is one.
fn is<'a>(text: &'a [u8], l: &Line, name: &[u8]) -> Option<&'a [u8]> {
    l.text(text).strip_prefix(name)
}

/// A value as a name or password with its zero: false if it's too long, or has a zero byte, which it can't hold.
fn copy(to: &mut [u8], most: usize, from: &[u8]) -> bool {
    if from.len() > most || from.contains(&0) {
        return false;
    }
    set(to, from);
    true
}

// ---- the list ----

impl List {
    /// No networks: all zeros, so a static of it takes no room in the app's file.
    pub const fn new() -> List {
        List {
            net: [Net::new(); MOST],
            count: 0,
            in_use: 0,
            skipped: 0,
            removed: [Removed { ssid: [0; SSID_MOST + 1], at: 0 }; MOST],
            removed_count: 0,
        }
    }

    /// Empty again: all zeros, as new() makes it, without a new one on the stack.
    pub fn clear(&mut self) {
        self.net.fill(Net::new());
        self.count = 0;
        self.in_use = 0;
        self.skipped = 0;
        self.removed.fill(Removed { ssid: [0; SSID_MOST + 1], at: 0 });
        self.removed_count = 0;
    }

    /// Takes network i out of the list, keeping in_use on the same network, or the first.
    fn drop(&mut self, i: usize) {
        self.net.copy_within(i + 1..self.count, i);
        self.count -= 1;
        if self.in_use == i || self.in_use > self.count {
            self.in_use = 0;
        } else if self.in_use > i {
            self.in_use -= 1;
        }
    }

    fn removed_at(&self, ssid: &[u8]) -> Option<usize> {
        self.removed[..self.removed_count].iter().position(|r| until_zero(&r.ssid) == ssid)
    }

    /// Reads a file's text.
    pub fn read(&mut self, text: &[u8]) {
        self.clear();
        let mut want = [0u8; SSID_MOST + 1];
        let mut have_use = false;
        // The network being read (None while skipping one), and whether its (last) password can't be.
        let mut net: Option<usize> = None;
        let mut bad = false;
        let mut at = bom(text);
        let mut start = at;
        while let Some(line) = next_line(text, &mut at) {
            if !skipped_line(text, &line) {
                if let Some(value) = is(text, &line, b"ssid=") {
                    if net.is_some() && bad {
                        self.count -= 1;
                        self.skipped += 1;
                    }
                    net = None;
                    bad = false;
                    // A name that can't be, one already read (the first that can be joined counts), or one too many.
                    let mut name = [0u8; SSID_MOST + 1];
                    if !copy(&mut name, SSID_MOST, value)
                        || !ssid_ok(until_zero(&name))
                        || self.find(until_zero(&name)).is_some()
                        || self.count == MOST
                    {
                        self.skipped += 1;
                    } else {
                        let n = &mut self.net[self.count];
                        *n = Net::new();
                        n.ssid = name;
                        n.in_file = true;
                        n.at = start;
                        net = Some(self.count);
                        self.count += 1;
                    }
                } else if let Some(value) = is(text, &line, b"password=") {
                    // Given twice, the last counts.
                    if let Some(i) = net {
                        let n = &mut self.net[i];
                        bad = !copy(&mut n.pass, PASS_MOST, value) || !pass_ok(n.pass());
                    }
                } else if let Some(value) = is(text, &line, b"use=") {
                    have_use = copy(&mut want, SSID_MOST, value);
                }
            }
            start = at;
        }
        if net.is_some() && bad {
            self.count -= 1;
            self.skipped += 1;
        }
        let found = if have_use { self.find(until_zero(&want)) } else { None };
        self.in_use = found.unwrap_or(0);
    }

    /// The file's text for the list, in `to`: None if it doesn't fit, and otherwise its length, which may be 0. It's
    /// the old text (or none, empty) changed as little as can be (docs/WIFI-TXT.md): comments, blank lines, settings
    /// it doesn't know and networks it skipped stay as they were, with their line ends. A network's password= is
    /// rewritten in place, or added just after its ssid=; a removed one's settings go, in every network of its name,
    /// with a blank line just before it (other blank lines and comments stay); use= is set where it is (or added
    /// before the first network); and new networks go at the end.
    pub fn write(&self, old: &[u8], to: &mut [u8]) -> Option<usize> {
        let mut o = Out { s: to, n: 0, ok: true };
        let using: Option<&[u8]> =
            if self.count != 0 { Some(self.net[if self.in_use < self.count { self.in_use } else { 0 }].ssid()) } else { None };
        // For lines added: the file's own line end, from its first line.
        let nl: &[u8] = match old.iter().position(|&c| c == b'\n') {
            Some(i) if i != 0 && old[i - 1] == b'\r' => b"\r\n",
            _ => b"\n",
        };
        if old.is_empty() {
            o.put(HEAD);
        }
        let mut at = bom(old);
        o.put(&old[..at]);
        // A use= anywhere is set where it is; with none, one goes before the first network.
        let (mut had_use, mut use_added) = (false, false);
        let mut i = at;
        while let Some(line) = next_line(old, &mut i) {
            had_use |= is(old, &line, b"use=").is_some();
        }
        // Whose lines these are: a network kept as it is, ours (its password rewritten), or one removed.
        #[derive(PartialEq)]
        enum Mode {
            Keep,
            Ours(usize),
            Drop,
        }
        let mut mode = Mode::Keep;
        let mut written = [false; MOST];
        // Where the blank line just copied began, and where it ended: a removed network takes it too, so adding and
        // removing one leaves the file as it was.
        let (mut blank_at, mut after_blank) = (0usize, usize::MAX);
        let mut start = at;
        while o.ok {
            let Some(line) = next_line(old, &mut at) else { break };
            // This line's own end, for a line rewritten in place: none for a last line with no LF (a lone CR there is
            // no line end; the reader drops it).
            let end: &[u8] = if line.whole > line.len && old[line.s + line.whole - 1] == b'\n' {
                &old[line.s + line.len..line.s + line.whole]
            } else {
                b""
            };
            if skipped_line(old, &line) {
                let before = o.n;
                o.put_line(&line, old, nl);
                if line.len == 0 {
                    (blank_at, after_blank) = (before, o.n);
                }
            } else if let Some(value) = is(old, &line, b"ssid=") {
                if let Some(u) = using
                    && !had_use
                    && !use_added
                {
                    o.put_setting(b"use=", u, nl);
                    o.put(nl);
                    use_added = true;
                }
                let mut name = [0u8; SSID_MOST + 1];
                let named = copy(&mut name, SSID_MOST, value);
                let name = until_zero(&name);
                let i = if named { self.find(name) } else { None };
                // Changed in place: the copy that was read; for one that wasn't read, the first copy that can be
                // joined (one past the 12 the reader took), which the next read would take. One that can't be joined
                // stays, and the network goes at the end.
                let ours = i.filter(|&i| {
                    !written[i]
                        && if self.net[i].in_file {
                            self.net[i].at == start
                        } else {
                            ssid_ok(name) && usable(old, at)
                        }
                });
                if let Some(i) = ours {
                    mode = Mode::Ours(i);
                    written[i] = true;
                    // Its name as it is; a last line with no end gets one, as its password may follow.
                    o.put(line.text(old));
                    o.put(if end.is_empty() { nl } else { end });
                    // A password the file didn't have goes just after its name.
                    if self.net[i].pass[0] != 0 && !has_password(old, at) {
                        o.put_setting(b"password=", self.net[i].pass(), nl);
                    }
                } else if i.is_none() && named && self.removed_at(name).is_some() {
                    // Removed: every network of that name goes.
                    mode = Mode::Drop;
                    if o.n == after_blank {
                        o.n = blank_at;
                    }
                } else {
                    mode = Mode::Keep;
                    o.put_line(&line, old, nl);
                }
            } else if is(old, &line, b"password=").is_some() && mode != Mode::Keep {
                if let Mode::Ours(i) = mode {
                    o.put_setting(b"password=", self.net[i].pass(), end);
                }
            } else if is(old, &line, b"use=").is_some() {
                // With no networks left, use= names none: it goes.
                if let Some(u) = using {
                    o.put_setting(b"use=", u, end);
                }
            } else if mode != Mode::Drop {
                o.put_line(&line, old, nl);
            }
            start = at;
        }
        // What's added starts on a line of its own.
        let add_use = using.is_some() && !had_use && !use_added;
        let adding = add_use || written[..self.count].iter().any(|&w| !w);
        // A last line with no LF gets one. If it ends in a lone CR, which the reader drops, LF alone makes it the CRLF
        // it drops; otherwise the file's line end. Either way the line reads as before.
        if adding && o.n != 0 && o.s[o.n - 1] != b'\n' {
            o.put(if o.s[o.n - 1] == b'\r' { b"\n" } else { nl });
        }
        if let Some(u) = using
            && add_use
        {
            o.put_setting(b"use=", u, nl);
        }
        for (net, _) in self.net[..self.count].iter().zip(written).filter(|(_, w)| !w) {
            if o.n != 0 {
                o.put(nl);
            }
            o.put_setting(b"ssid=", net.ssid(), nl);
            o.put_setting(b"password=", net.pass(), nl);
        }
        if o.ok { Some(o.n) } else { None }
    }

    /// Which network has this name.
    pub fn find(&self, ssid: &[u8]) -> Option<usize> {
        self.net[..self.count].iter().position(|n| n.ssid() == ssid)
    }

    /// Adds a network, or changes the password of the one with its name, and makes it the one to use. Its index, or
    /// None if the name or password can't be or the list is full.
    pub fn put(&mut self, ssid: &[u8], pass: &[u8]) -> Option<usize> {
        if !ssid_ok(ssid) || !pass_ok(pass) {
            return None;
        }
        let i = match self.find(ssid) {
            Some(i) => i,
            None => {
                if self.count == MOST {
                    return None;
                }
                // Removed and put back: it's still in the file, where its password is changed, so it goes back to its
                // place among the networks read. A new one goes at the end.
                let r = self.removed_at(ssid);
                let mut i = self.count;
                if let Some(r) = r {
                    let was = self.removed[r].at;
                    i = self.net[..self.count].iter().position(|n| !(n.in_file && n.at < was)).unwrap_or(self.count);
                }
                self.net.copy_within(i..self.count, i + 1);
                self.count += 1;
                self.net[i] = Net::new();
                set(&mut self.net[i].ssid, ssid);
                if let Some(r) = r {
                    self.net[i].in_file = true;
                    self.net[i].at = self.removed[r].at;
                    self.removed.copy_within(r + 1..self.removed_count, r);
                    self.removed_count -= 1;
                }
                i
            }
        };
        set(&mut self.net[i].pass, pass);
        self.in_use = i;
        Some(i)
    }

    pub fn remove(&mut self, i: usize) {
        if i >= self.count {
            return;
        }
        // At most MOST were read, and each is recorded once.
        if self.net[i].in_file && self.removed_count < MOST {
            self.removed[self.removed_count] = Removed { ssid: self.net[i].ssid, at: self.net[i].at };
            self.removed_count += 1;
        }
        self.drop(i);
    }
}

// ---- writing ----

struct Out<'a> {
    s: &'a mut [u8],
    n: usize,
    ok: bool,
}

impl Out<'_> {
    fn put(&mut self, s: &[u8]) {
        if s.is_empty() {
            return;
        }
        if !self.ok || self.n + s.len() > self.s.len() {
            self.ok = false;
            return;
        }
        self.s[self.n..self.n + s.len()].copy_from_slice(s);
        self.n += s.len();
    }

    /// name and value, then end, the line end.
    fn put_setting(&mut self, name: &[u8], value: &[u8], end: &[u8]) {
        self.put(name);
        self.put(value);
        self.put(end);
    }

    /// A line as it was. One that starts with a byte order mark is plain text where it is, but would be skipped as a
    /// mark at the start of the file: one that wasn't first, but would be now, gets a line end before it.
    fn put_line(&mut self, l: &Line, text: &[u8], nl: &[u8]) {
        if self.n == 0 && l.s != 0 && bom(l.text(text)) != 0 {
            self.put(nl);
        }
        self.put(l.with_end(text));
    }
}

/// Whether the network whose ssid= ends at `at` can be joined as it is: its last password= one that can be, as the
/// reader has it (no password= is an open network).
fn usable(text: &[u8], mut at: usize) -> bool {
    let mut ok = true;
    while let Some(line) = next_line(text, &mut at) {
        if is(text, &line, b"ssid=").is_some() {
            break;
        }
        // The last password= counts, as the reader has it.
        if let Some(value) = is(text, &line, b"password=") {
            let mut pass = [0u8; PASS_MOST + 1];
            ok = copy(&mut pass, PASS_MOST, value) && pass_ok(until_zero(&pass));
        }
    }
    ok
}

/// Whether the network whose ssid= ends at `at` has a password= among its lines.
fn has_password(text: &[u8], mut at: usize) -> bool {
    while let Some(line) = next_line(text, &mut at) {
        if is(text, &line, b"ssid=").is_some() {
            return false;
        }
        if is(text, &line, b"password=").is_some() {
            return true;
        }
    }
    false
}
