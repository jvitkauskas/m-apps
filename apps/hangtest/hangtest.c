/* SPDX-License-Identifier: MIT */
/* Hangs on purpose, to test the guard: make run APP=hangtest. The watchdog should reset the unit into download mode
 * after 32 s, and the log should end with the line below. */
#include "app.h"

int app_main(unsigned lsb_mhz) {
    (void)lsb_mhz;
    app_log("about to hang");
    for (;;) {
    }
}
