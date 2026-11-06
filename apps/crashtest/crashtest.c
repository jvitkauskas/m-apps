/* SPDX-License-Identifier: MIT */
/* Crashes on purpose, to test the guard: make run APP=crashtest. The log should end with the crash. */
#include "app.h"

int app_main(unsigned lsb_mhz) {
    (void)lsb_mhz;
    app_log("about to read from an odd address");
    volatile uint32_t *odd = (volatile uint32_t *)(uintptr_t)0x01c30002u;
    app_log("read %08x, and it should have crashed", (unsigned)*odd);
    return 1;
}
