// SPDX-License-Identifier: MIT
//! Decodes an MP3 file with the player's decoder (apps/mp3/minimp3.rs), fed as the player feeds it, and writes the
//! samples as raw 16-bit PCM: mp3_decoder_host MP3 OUT [TIMES]. With TIMES, it decodes the file that many times and
//! says how long that took. See tests/mp3_decoder_test.py, which does the same with the C minimp3 and compares.
#[allow(dead_code)]
#[path = "../apps/mp3/minimp3.rs"]
mod minimp3;
use minimp3::{FrameInfo, MAX_SAMPLES_PER_FRAME, Mp3Dec, mp3dec_decode_frame, mp3dec_init};

/// As the player has them (apps/mp3/main.rs).
const CHUNK: usize = 4096;
const INPUT: usize = 16 * 1024;

/// The file, decoded: its samples, and its frames' sample rate, channels and how many there were.
fn decode(file: &[u8], dec: &mut Mp3Dec, out: &mut Vec<u8>) -> (i32, i32, u32) {
    let mut input = vec![0u8; INPUT];
    let mut pcm = [0i16; MAX_SAMPLES_PER_FRAME];
    let (mut have, mut offset, mut frames) = (0, 0, 0);
    let (mut hz, mut channels) = (0, 0);
    *dec = Mp3Dec::new();
    mp3dec_init(dec);
    loop {
        while have < INPUT - CHUNK && offset < file.len() {
            let n = (file.len() - offset).min(CHUNK);
            input[have..have + n].copy_from_slice(&file[offset..offset + n]);
            have += n;
            offset += n;
        }
        let mut info = FrameInfo::default();
        let decoded = mp3dec_decode_frame(dec, &input[..have], Some(&mut pcm), &mut info);
        if info.frame_bytes == 0 {
            break;
        }
        let used = info.frame_bytes as usize;
        input.copy_within(used..have, 0);
        have -= used;
        if decoded == 0 {
            continue;
        }
        frames += 1;
        (hz, channels) = (info.hz, info.channels);
        for s in &pcm[..decoded as usize * info.channels as usize] {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    (hz, channels, frames)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("mp3_decoder_host MP3 OUT [TIMES]");
        std::process::exit(2);
    }
    let file = std::fs::read(&args[1]).unwrap();
    let times: u32 = args.get(3).map_or(1, |t| t.parse().unwrap());
    let mut dec = Box::new(Mp3Dec::new());
    let mut out = Vec::new();
    let started = std::time::Instant::now();
    let mut said = (0, 0, 0);
    for _ in 0..times {
        out.clear();
        said = decode(&file, &mut dec, &mut out);
    }
    let took = started.elapsed();
    std::fs::write(&args[2], &out).unwrap();
    println!(
        "{} Hz, {} channels, {} frames, {} bytes, {:.3} ms each",
        said.0,
        said.1,
        said.2,
        out.len(),
        took.as_secs_f64() * 1000.0 / f64::from(times)
    );
}
