/* SPDX-License-Identifier: MIT */
/* C start of an app: the two ways in, from m-bootloader's docs/SPEC.md §11.8. */
#include "app.h"
#include "usb_midi.h"
#include <string.h>
/* From here up, the entry's argument is the ROM's pointer, not a clock. */
#define ROM_POINTER 0x01000000u
extern uint8_t __bss_start[], __bss_end[];

int app_start(uint32_t argument);
int app_start(uint32_t argument) {
    bool from_rom = argument >= ROM_POINTER;
    memset(__bss_start, 0, (size_t)(__bss_end-__bss_start));
    if (from_rom) {
        hal_early();
        usb_midi_stop();
        hal_clock_b();
        argument = 48;
    } else
        hal_timer_clock(argument);
    int result = app_main(argument);
    if (from_rom)
        hal_reset();
    return result;
}
