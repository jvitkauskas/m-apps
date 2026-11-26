// SPDX-License-Identifier: MIT
//! The smallest useful app: says what it can see, and goes back after a key or 10 seconds.
#![no_std]
use kit::board::{board_init, board_keys, board_restore};
use kit::display::line;
use kit::platform::hal_delay_us;
use kit::{APP_BOARD, app, log};

const WHITE: u16 = 0xffff;
const GREEN: u16 = 0x07e0;
const YELLOW: u16 = 0xffe0;
const WAIT_MS: u32 = 10_000;

kit::app!(main);

fn main(lsb_mhz: u32) -> i32 {
    let board = app::board();
    log!("hello on board ", board, " (built for ", APP_BOARD, "), ",
        if app::folder() != 0 { "from its folder" } else { "not from a folder" });
    if board == 0 {
        return 1;
    }
    board_init(board == 2);
    let screen = kit::display::init(board);
    log!("screen ", if screen { "found" } else { "not found" });
    if screen {
        for row in 0..15 {
            line(row, WHITE, b"");
        }
        line(1, GREEN, b"HELLO FROM RAM");
        line(3, WHITE, if board == 2 { b"SMK-37" } else { b"FM-1" });
        line(9, GREEN, if APP_BOARD != 0 { b"BUILT FOR IT" } else { b"ONE FILE FOR BOTH" });
        let mut mhz = *b"LSB 00 MHZ";
        mhz[4] = b'0' + (lsb_mhz / 10 % 10) as u8;
        mhz[5] = b'0' + (lsb_mhz % 10) as u8;
        line(4, WHITE, &mhz);
        if app::folder() != 0 {
            line(5, WHITE, b"FROM ITS FOLDER");
        }
        line(6, YELLOW, b"ANY KEY: BACK");
        line(7, WHITE, b"OR WAIT 10 SECONDS");
    }
    while board_keys() != 0 {
        hal_delay_us(1000);
    }
    let mut ms = 0;
    while ms < WAIT_MS && board_keys() == 0 {
        hal_delay_us(1000);
        ms += 1;
    }
    if ms < WAIT_MS {
        log!("a key after ", ms, " ms");
    } else {
        log!("no key in ", ms, " ms");
    }
    board_restore();
    0
}
