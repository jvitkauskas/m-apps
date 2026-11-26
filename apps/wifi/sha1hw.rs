// SPDX-License-Identifier: MIT
//! SHA-1 blocks on the chip's SHA unit (JL_SHA at 0x11700), as JieLi's AC79 SDK drives it (lib/net/mbedtls_2_26_0/todo/
//! algorithm_alt.c, mbedtls_sha1_process, Apache-2.0): the block read by DMA from 32-byte aligned RAM, the state in
//! RESULT7..RESULT3, CON bit 3 SHA-1, bit 1 a block from that state, bit 15 done, bit 14 to clear it. A WPA2 key is
//! 4096 rounds of HMAC-SHA-1, about 33 000 blocks, so this is most of a join's time. It isn't trusted until check()
//! has seen it give what the software does.
use crate::wifi_crypto::sha1_block_sw;
use kit::hw::{Reg, bit, dma_address, sync_cpu};
use kit::{Global, log};

const SHA_CON: Reg = Reg::at::<0x11700>();
const SHA_ADR: Reg = Reg::at::<0x11704>();
const SHA_RESULT0: Reg = Reg::at::<0x11708>();
const WAIT: u32 = 200_000;
const TIMER_COUNT: Reg = Reg::at::<0x10804>();

const fn result(n: u32) -> Reg {
    SHA_RESULT0.add(4 * n)
}

/// In the low RAM other DMA reaches (kit/app.ld.S), aligned as the unit wants.
#[repr(C, align(32))]
struct Block([u8; 64]);

// Safety: as net.rs's DMA buffers: a NOLOAD section for buffers alone.
#[unsafe(link_section = ".dma")]
static BLOCK: Global<Block> = Global::new(Block([0; 64]));

const UNTRIED: u8 = 0;
const WORKS: u8 = 1;
const FAILS: u8 = 2;
static STATE: Global<u8> = Global::new(UNTRIED);

#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
fn run(h: &mut [u32; 5], p: &[u8; 64]) -> bool {
    // Safety: only run() uses the block, which it doesn't hold past the unit's finishing with it.
    let block = unsafe { BLOCK.get() };
    // As words, which the DMA reads: a 64-byte copy was a call to the byte-at-a-time memcpy.
    let words: *mut u32 = block.0.as_mut_ptr().cast();
    for (i, w) in p.as_chunks::<4>().0.iter().enumerate() {
        // Safety: word i of the 32-byte-aligned block, which run() alone uses.
        unsafe { words.add(i).write_volatile(u32::from_ne_bytes(*w)) };
    }
    for (i, &v) in h.iter().enumerate() {
        result(7 - i as u32).write(v);
    }
    result(2).write(0);
    result(1).write(0);
    result(0).write(0);
    SHA_ADR.write(dma_address(block.0.as_mut_ptr()));
    sync_cpu();
    SHA_CON.set(bit(14));
    SHA_CON.set(bit(3));
    SHA_CON.set(bit(1));
    let mut n = WAIT;
    while SHA_CON.read() & bit(15) == 0 {
        n -= 1;
        if n == 0 {
            return false;
        }
    }
    SHA_CON.set(bit(14));
    for (i, v) in h.iter_mut().enumerate() {
        *v = result(7 - i as u32).read();
    }
    true
}

/// wifi_crypto.rs's hook: true if it did the block.
#[cfg_attr(target_arch = "pi32v2", optimize(speed))]
pub fn block(h: &mut [u32; 5], p: &[u8; 64]) -> bool {
    if STATE.read() != WORKS {
        return false;
    }
    if run(h, p) {
        return true;
    }
    // It stopped finishing: software for the rest of the run, rather than a timeout on every block.
    STATE.set(FAILS);
    log!("SHA-1 unit stopped finishing: software from here");
    false
}

/// Whether the chip's SHA unit gives what the software does: checked once, and then wifi_crypto.rs uses it.
pub fn check() -> bool {
    if STATE.read() != UNTRIED {
        return STATE.read() == WORKS;
    }
    const IV: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut data = [0u8; 64];
    let (mut seed, mut hw, mut sw) = (0x2545_F491u32, [0u32; 5], [0u32; 5]);
    let mut same = true;
    // From the start, and from a state of no hash's.
    let mut t = 0;
    while t < 4 && same {
        for d in data.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *d = (seed >> 24) as u8;
        }
        for i in 0..5 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            hw[i] = if t == 0 { IV[i] } else { seed };
            sw[i] = hw[i];
        }
        sha1_block_sw(&mut sw, &data);
        same = run(&mut hw, &data) && hw == sw;
        t += 1;
    }
    STATE.set(if same { WORKS } else { FAILS });
    if same {
        // How much faster: 256 blocks each way, on the 24 MHz timer.
        let t0 = TIMER_COUNT.read();
        for _ in 0..256 {
            run(&mut hw, &data);
        }
        let t1 = TIMER_COUNT.read();
        for _ in 0..256 {
            sha1_block_sw(&mut sw, &data);
        }
        // (Used, so the compiler can't leave the software's blocks out.)
        core::hint::black_box(&sw);
        let t2 = TIMER_COUNT.read();
        log!("SHA-1 unit works: 256 blocks in ", t1.wrapping_sub(t0) / 24, " us, ", t2.wrapping_sub(t1) / 24,
            " us in software");
    } else {
        log!("SHA-1 unit gave something else, or didn't finish: software it is");
    }
    same
}
