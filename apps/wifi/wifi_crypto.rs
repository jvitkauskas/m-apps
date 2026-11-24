// SPDX-License-Identifier: GPL-3.0-only OR MPL-2.0
// Copyright (C) 2026 Henri David (hdavid), Optimist (github.com/w0ts/optimist)
//! The crypto a WPA2-PSK station needs (IEEE 802.11-2020 §12.7): SHA-1 (FIPS 180-4), HMAC-SHA1 (RFC 2104), PBKDF2 (RFC
//! 8018) for the PMK from the passphrase, the PRF for the PTK (§12.7.1.2), AES-128 decryption (FIPS-197) for the AES
//! key unwrap (RFC 3394) of the group key. CCMP itself is the MAC's (Optimist's docs/WIFI-HW-FACTS.md). Written for
//! Optimist, host-tested there against the published vectors (its tests/wifi/wifi_sta_test.c). Small over fast: run a
//! few times per join, never per frame.
//!
//! Taken here under MPL-2.0. Changed by m-apps: in Rust; on the unit, each SHA-1 block is offered to the chip's SHA
//! unit first (sha1hw.rs), as the C's WC_SHA1_HW hook had it; on a computer, software alone.

// ---- SHA-1 (FIPS 180-4)

/// The software block. The schedule is a ring of 16 words, each made as it's needed, rather than 80 made first: on the
/// unit an array the compiler has to clear first was a third of the block's time.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn sha1_block_sw(h: &mut [u32; 5], p: &[u8; 64]) {
    let mut w: [u32; 16] = core::array::from_fn(|i| u32::from_be_bytes([p[4 * i], p[4 * i + 1], p[4 * i + 2], p[4 * i + 3]]));
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for i in 0..80 {
        let wi = if i < 16 {
            w[i]
        } else {
            let v = (w[(i + 13) & 15] ^ w[(i + 8) & 15] ^ w[(i + 2) & 15] ^ w[i & 15]).rotate_left(1);
            w[i & 15] = v;
            v
        };
        let (f, k) = if i < 20 {
            ((b & c) | (!b & d), 0x5A82_7999)
        } else if i < 40 {
            (b ^ c ^ d, 0x6ED9_EBA1)
        } else if i < 60 {
            ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
        } else {
            (b ^ c ^ d, 0xCA62_C1D6)
        };
        let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
        (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
    }
    for (h, v) in h.iter_mut().zip([a, b, c, d, e]) {
        *h = h.wrapping_add(v);
    }
}

/// A block: on the unit, the chip's SHA unit if it does it; otherwise software.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn sha1_block(h: &mut [u32; 5], p: &[u8; 64]) {
    #[cfg(target_arch = "pi32v2")]
    if crate::sha1hw::block(h, p) {
        return;
    }
    sha1_block_sw(h, p);
}

#[derive(Clone, Copy)]
pub struct Sha1 {
    h: [u32; 5],
    n: u64,
    b: [u8; 64],
    bn: usize,
}

impl Sha1 {
    /// Not started: only a placeholder until new() replaces it.
    const ZERO: Sha1 = Sha1 { h: [0; 5], n: 0, b: [0; 64], bn: 0 };

    pub const fn new() -> Sha1 {
        Sha1 { h: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0], n: 0, b: [0; 64], bn: 0 }
    }

    /// Whole blocks go to sha1_block() from where they are; only a part block is copied.
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn add(&mut self, mut p: &[u8]) {
        self.n = self.n.wrapping_add(p.len() as u64);
        if self.bn > 0 {
            let k = (64 - self.bn).min(p.len());
            self.b[self.bn..self.bn + k].copy_from_slice(&p[..k]);
            self.bn += k;
            p = &p[k..];
            if self.bn < 64 {
                return;
            }
            sha1_block(&mut self.h, &self.b);
            self.bn = 0;
        }
        let (blocks, rest) = p.as_chunks::<64>();
        for block in blocks {
            sha1_block(&mut self.h, block);
        }
        self.b[..rest.len()].copy_from_slice(rest);
        self.bn = rest.len();
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn done(&mut self) -> [u8; 20] {
        let bits = self.n.wrapping_mul(8);
        // The padding: 0x80, zeros to 56 bytes into a block, then the length in bits.
        let bn = self.bn.min(63);
        self.b[bn] = 0x80;
        self.b[bn + 1..].fill(0);
        if bn >= 56 {
            sha1_block(&mut self.h, &self.b);
            self.b.fill(0);
        }
        self.b[56..].copy_from_slice(&bits.to_be_bytes());
        sha1_block(&mut self.h, &self.b);
        self.bn = 0;
        let mut out = [0u8; 20];
        for (o, h) in out.chunks_exact_mut(4).zip(self.h) {
            o.copy_from_slice(&h.to_be_bytes());
        }
        out
    }
}

impl Default for Sha1 {
    fn default() -> Sha1 {
        Sha1::new()
    }
}

// ---- HMAC-SHA1 (RFC 2104)

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn hmac_start(key: &[u8], x: u8) -> Sha1 {
    let d: [u8; 20];
    let key = if key.len() > 64 {
        let mut s = Sha1::new();
        s.add(key);
        d = s.done();
        &d[..]
    } else {
        key
    };
    let k: [u8; 64] = core::array::from_fn(|i| key.get(i).copied().unwrap_or(0) ^ x);
    let mut s = Sha1::new();
    s.add(&k);
    s
}

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn hmac_sha1(key: &[u8], p: &[u8]) -> [u8; 20] {
    Hmac::new(key).mac(p)
}

/// HMAC-SHA1 with one key, its two pad blocks hashed once: PBKDF2's 4096 rounds then hash two blocks each, not four.
#[derive(Clone, Copy)]
pub struct Hmac {
    inner: Sha1,
    outer: Sha1,
}

impl Hmac {
    /// No key yet: all zeros, so a static job takes no room in the file.
    const EMPTY: Hmac = Hmac { inner: Sha1::ZERO, outer: Sha1::ZERO };

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn new(key: &[u8]) -> Hmac {
        Hmac { inner: hmac_start(key, 0x36), outer: hmac_start(key, 0x5C) }
    }

    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn mac(&self, p: &[u8]) -> [u8; 20] {
        let mut s = self.inner;
        s.add(p);
        let inner = s.done();
        let mut s = self.outer;
        s.add(&inner);
        s.done()
    }
}

// ---- the PMK as a job in slices (the Wi-Fi page: a main-loop pass does a few hundred of the 8192 HMACs, never the
// lot). PBKDF2 (RFC 8018): two blocks of 4096 rounds of 20 bytes, for 32; the passphrase is copied into the job and
// wiped when it is done.

/// The PMK: PBKDF2-HMAC-SHA1(passphrase, ssid, 4096, 32), in slices.
pub struct PmkJob {
    hmac: Hmac,
    pass: [u8; 64],
    salt: [u8; 32],
    u: [u8; 20],
    t: [u8; 20],
    pn: usize,
    sn: usize,
    blk: usize,
    round: u32,
    done: bool,
}

impl PmkJob {
    pub const fn new() -> PmkJob {
        PmkJob { hmac: Hmac::EMPTY, pass: [0; 64], salt: [0; 32], u: [0; 20], t: [0; 20], pn: 0, sn: 0, blk: 0, round: 0, done: false }
    }

    /// The passphrase up to a zero, or 63 bytes; the name up to 32.
    pub fn begin(&mut self, pass: &[u8], ssid: &[u8]) {
        self.pn = 0;
        while self.pn < pass.len() && pass[self.pn] != 0 && self.pn < 63 {
            self.pass[self.pn] = pass[self.pn];
            self.pn += 1;
        }
        self.sn = ssid.len().min(32);
        self.salt[..self.sn].copy_from_slice(&ssid[..self.sn]);
        self.blk = 1;
        self.round = 0;
        self.done = false;
    }

    /// Up to n rounds; true when pmk is complete (and the passphrase wiped).
    #[cfg_attr(target_arch = "pi32v2", optimize(speed))]
    pub fn step(&mut self, mut n: u32, pmk: &mut [u8; 32]) -> bool {
        while n != 0 && !self.done {
            if self.round == 0 {
                // A block's first round: U1 = HMAC(P, salt || INT(blk)).
                let mut s = [0u8; 36];
                s[..self.sn].copy_from_slice(&self.salt[..self.sn]);
                s[self.sn + 3] = self.blk as u8;
                // The passphrase's pads, hashed once for the 4096 rounds.
                self.hmac = Hmac::new(&self.pass[..self.pn]);
                self.u = self.hmac.mac(&s[..self.sn + 4]);
                self.t = self.u;
                self.round = 1;
            } else {
                self.u = self.hmac.mac(&self.u);
                for (t, u) in self.t.iter_mut().zip(self.u) {
                    *t ^= u;
                }
                self.round += 1;
            }
            n -= 1;
            if self.round == 4096 {
                // The block done.
                let at = (self.blk - 1) * 20;
                let k = (32 - at).min(20);
                pmk[at..at + k].copy_from_slice(&self.t[..k]);
                self.round = 0;
                self.blk += 1;
                if self.blk > 2 {
                    // (The job is a caller's object: the store is not dead.)
                    self.pass.fill(0);
                    self.hmac = Hmac::EMPTY;
                    self.done = true;
                }
            }
        }
        self.done
    }
}

// ---- the PRF (§12.7.1.2): HMAC-SHA1(K, A || 0 || B || i) for i = 0, 1, ...

/// PRF-X(K, A, B): label A, data B, out.len() bytes.
pub fn prf(k: &[u8], label: &[u8], b: &[u8], out: &mut [u8]) {
    let mut m = [0u8; 128];
    let ln = label.len();
    m[..ln].copy_from_slice(label);
    m[ln] = 0;
    m[ln + 1..ln + 1 + b.len()].copy_from_slice(b);
    let n = ln + 1 + b.len();
    for (i, chunk) in out.chunks_mut(20).enumerate() {
        m[n] = i as u8;
        let d = hmac_sha1(k, &m[..n + 1]);
        chunk.copy_from_slice(&d[..chunk.len()]);
    }
}

// ---- AES-128 (FIPS-197)

static SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76, 0xca, 0x82, 0xc9, 0x7d,
    0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0, 0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc,
    0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15, 0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2,
    0xeb, 0x27, 0xb2, 0x75, 0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf, 0xd0, 0xef, 0xaa, 0xfb,
    0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8, 0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5,
    0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2, 0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d,
    0x64, 0x5d, 0x19, 0x73, 0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79, 0xe7, 0xc8, 0x37, 0x6d,
    0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08, 0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6,
    0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a, 0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9,
    0x86, 0xc1, 0x1d, 0x9e, 0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

fn xt(x: u8) -> u8 {
    x << 1 ^ if x & 0x80 != 0 { 0x1B } else { 0 }
}

fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xt(a);
        b >>= 1;
    }
    r
}

/// (The inverse S-box by search: a few unwraps per join.)
fn inv_sbox(y: u8) -> u8 {
    SBOX.iter().position(|&s| s == y).unwrap_or(0) as u8
}

fn expand(key: &[u8; 16]) -> [u8; 176] {
    let mut rk = [0u8; 176];
    let mut rc = 1u8;
    rk[..16].copy_from_slice(key);
    for i in (16..176).step_by(4) {
        let (mut t0, mut t1, mut t2, mut t3) = (rk[i - 4], rk[i - 3], rk[i - 2], rk[i - 1]);
        if i % 16 == 0 {
            let s = t0;
            (t0, t1, t2, t3) =
                (SBOX[usize::from(t1)] ^ rc, SBOX[usize::from(t2)], SBOX[usize::from(t3)], SBOX[usize::from(s)]);
            rc = xt(rc);
        }
        rk[i] = rk[i - 16] ^ t0;
        rk[i + 1] = rk[i - 15] ^ t1;
        rk[i + 2] = rk[i - 14] ^ t2;
        rk[i + 3] = rk[i - 13] ^ t3;
    }
    rk
}

pub fn aes128_decrypt(key: &[u8; 16], input: &[u8; 16]) -> [u8; 16] {
    let rk = expand(key);
    let mut s = [0u8; 16];
    let mut t = [0u8; 16];
    for i in 0..16 {
        s[i] = input[i] ^ rk[160 + i];
    }
    let mut r = 9;
    loop {
        // InvShiftRows + InvSubBytes
        for i in 0..16 {
            t[(i + 4 * (i % 4)) % 16] = inv_sbox(s[i]);
        }
        for i in 0..16 {
            s[i] = t[i] ^ rk[16 * r + i];
        }
        if r == 0 {
            break;
        }
        // InvMixColumns
        for p in s.chunks_exact_mut(4) {
            let [a0, a1, a2, a3] = [p[0], p[1], p[2], p[3]];
            p[0] = mul(a0, 14) ^ mul(a1, 11) ^ mul(a2, 13) ^ mul(a3, 9);
            p[1] = mul(a0, 9) ^ mul(a1, 14) ^ mul(a2, 11) ^ mul(a3, 13);
            p[2] = mul(a0, 13) ^ mul(a1, 9) ^ mul(a2, 14) ^ mul(a3, 11);
            p[3] = mul(a0, 11) ^ mul(a1, 13) ^ mul(a2, 9) ^ mul(a3, 14);
        }
        r -= 1;
    }
    s
}

// ---- RFC 3394 key unwrap

/// input: n bytes (a multiple of 8, >= 24), out: n - 8 bytes; true if the integrity check holds.
pub fn aes_unwrap(kek: &[u8; 16], input: &[u8], out: &mut [u8]) -> bool {
    let n = input.len();
    if n % 8 != 0 || n < 24 {
        return false;
    }
    let blocks = n / 8 - 1;
    let mut a = [0u8; 8];
    a.copy_from_slice(&input[..8]);
    out[..blocks * 8].copy_from_slice(&input[8..8 + blocks * 8]);
    for j in (0..6).rev() {
        for i in (1..=blocks).rev() {
            let t = (blocks * j + i) as u32;
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[7] ^= t as u8;
            b[6] ^= (t >> 8) as u8;
            b[5] ^= (t >> 16) as u8;
            b[4] ^= (t >> 24) as u8;
            b[8..].copy_from_slice(&out[8 * (i - 1)..8 * i]);
            let b = aes128_decrypt(kek, &b);
            a.copy_from_slice(&b[..8]);
            out[8 * (i - 1)..8 * i].copy_from_slice(&b[8..]);
        }
    }
    a.iter().all(|&v| v == 0xA6)
}
