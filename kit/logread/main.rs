// SPDX-License-Identifier: MIT
//! A program for run-ram that answers m-flasher's --read from RAM, so a host can read an app's log, or any of its
//! RAM, after a reset into download mode. It follows the flash loader's contract: m-bootloader's docs/SPEC.md §10.1.
#![no_std]
use mboot::hw::sync_cpu;
use mboot::platform::p33_and;

const RAM_START: u32 = 0x01c0_0000;
const RAM_END: u32 = 0x01c8_0000;
const MAX_SEND: u32 = 512;

/// The ROM's: sends `n` bytes from `data` to the host.
type UsbSend = unsafe extern "C" fn(data: *const u8, n: u32) -> i32;

static USB_SEND: mboot::Global<Option<UsbSend>> = mboot::Global::new(None);

/// The ROM calls this with each block the host sent, and `answer` for what to send back (unused here).
unsafe extern "C" fn handler(block: *const u8, _answer: *mut u8) -> i32 {
    // Safety: the ROM's block has the command at 15, eight bytes of it.
    let c = unsafe { &*block.add(15).cast::<[u8; 8]>() };
    if c[0] != 0xfd || c[1] != 0x61 {
        return 0;
    }
    let mut address = u32::from_be_bytes([c[2], c[3], c[4], c[5]]);
    let mut size = u32::from(u16::from_be_bytes([c[6], c[7]]));
    if !(RAM_START..RAM_END).contains(&address) || size > RAM_END - address {
        return 0;
    }
    p33_and(0x80, 0xef);
    let Some(send) = USB_SEND.read() else { return 0 };
    while size != 0 {
        let n = size.min(MAX_SEND);
        // Safety: the ROM's own function, and RAM of the chip that it reads from.
        unsafe { send(core::ptr::with_exposed_provenance(address as usize), n) };
        address += n;
        size -= n;
    }
    1
}

/// The flash loader's entry: `params` is the ROM's table, with its send at 0 and the handler's slot at 2.
///
/// # Safety
/// As the ROM calls it.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".entry")]
pub unsafe extern "C" fn logread_entry(params: *const u32) -> i32 {
    let (send, slot) = unsafe { (params.read(), params.add(2).read()) };
    // Safety: the ROM gives its function's address as a word.
    USB_SEND.set(Some(unsafe { core::mem::transmute::<usize, UsbSend>(send as usize) }));
    sync_cpu();
    let slot: *mut u32 = core::ptr::with_exposed_provenance_mut(slot as usize);
    unsafe { slot.write_volatile(handler as *const () as usize as u32) };
    sync_cpu();
    0
}
