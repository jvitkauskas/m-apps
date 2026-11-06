/* SPDX-License-Identifier: MIT */
/* A program for run-ram that answers m-flasher's --read from RAM, so a host can read an app's log, or any of its
 * RAM, after a reset into download mode. It follows the flash loader's contract: m-bootloader's docs/SPEC.md §10.1. */
#include "platform.h"
#define RAM_START 0x01c00000u
#define RAM_END 0x01c80000u
#define MAX_SEND 512u
static int (*usb_send)(const void *, uint32_t);

static int handler(const uint8_t *block, uint8_t *answer) {
    const uint8_t *c = block+15;
    (void)answer;
    if (c[0] != 0xfd || c[1] != 0x61)
        return 0;
    uint32_t address = (uint32_t)c[2] << 24 | (uint32_t)c[3] << 16 | (uint32_t)c[4] << 8 | c[5];
    uint32_t size = (uint32_t)c[6] << 8 | c[7];
    if (address < RAM_START || address >= RAM_END || size > RAM_END-address)
        return 0;
    p33_and(0x80, 0xef);
    while (size) {
        uint32_t n = size < MAX_SEND ? size:MAX_SEND;
        usb_send((const void *)(uintptr_t)address, n);
        address += n;
        size -= n;
    }
    return 1;
}

__attribute__((section(".entry"), used)) int logread_entry(const uint32_t *params) {
    uint32_t *slot = (void *)(uintptr_t)params[2];
    usb_send = (int (*)(const void *, uint32_t))(uintptr_t)params[0];
    sync_cpu();
    *slot = (uint32_t)(uintptr_t)handler;
    sync_cpu();
    return 0;
}
