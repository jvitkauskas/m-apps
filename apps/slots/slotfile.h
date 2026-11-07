/* SPDX-License-Identifier: MIT */
#ifndef SLOTFILE_H
#define SLOTFILE_H
#include "fat.h"
/* Whole flash images on the card, in a folder of the unit's own: /MBOOT/SLOTS/<8 hex digits>/NAME.BIN. A slot is the same
 * 1 MiB file as an M-Flasher backup. Loading one writes what a restore would: not the boot area, the key area, or the
 * SMK-37's Bluetooth and calibration sectors. */
#define SLOT_SIZE 0x100000u
#define MOST_SLOTS 40u
#define NO_SLOT (-1)
struct slots {
    uint32_t folder;
    bool has_folder;
    struct fat_entry list[MOST_SLOTS];
    unsigned count;
    bool more;
    /* Names, with no slot as all zeros. */
    char active[8], pending[8];
    bool has_state;
    struct fat_entry state_file;
    char unit_name[11];
    void (*progress)(const char *what, unsigned done, unsigned total);
    /* Whether an image holds firmware that would start: read reads it like flash. */
    bool (*valid)(bool (*read)(void *ctx, uint32_t address, void *out, uint32_t size), void *ctx);
};
/* Mounts, lists this unit's slots, and reads what the last load left. */
const char *slots_open(struct slots *s, uint32_t card_sectors, const uint8_t uid[8]);
/* The slot holding what's on the unit, or NO_SLOT. It leaves out what a load doesn't write. */
int slots_on_unit(struct slots *s);
/* Saves what's on the unit as a new slot, and says which name it took, in name. */
const char *slots_save_new(struct slots *s, char name[8]);
/* Saves what's on the unit, then writes this slot to it. Only returns with no error once it's all done. */
const char *slots_load(struct slots *s, unsigned index);
const char *slots_delete(struct slots *s, unsigned index);
/* The slot an interrupted load was writing, or NO_SLOT. */
int slots_pending(const struct slots *s);
#endif
