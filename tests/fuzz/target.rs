// SPDX-License-Identifier: MIT
//! libFuzzer's entry, for the harnesses here: `fuzz!(|data| { ... });` is LLVMFuzzerTestOneInput, which libFuzzer's
//! main (clang's -fsanitize=fuzzer) calls with each input. A harness's crate is `#![no_main]`, so that main is the
//! program's. A panic aborts (-C panic=abort), which libFuzzer reports as a crash, keeping the input.

/// The body runs as a closure on the input, so `return` in it ends this input.
macro_rules! fuzz {
    (|$data:ident| $body:block) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, n: usize) -> i32 {
            // Safety: libFuzzer gives n bytes at data, which stay for the call; data may be null when n is 0.
            let input: &[u8] = if n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(data, n) } };
            (|$data: &[u8]| $body)(input);
            0
        }
    };
}
