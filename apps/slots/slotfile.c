/* SPDX-License-Identifier: MIT */
#include "slotfile.h"
#include "core.h"
#include "platform.h"
#include <string.h>
#define SECTOR 0x1000u
#define CARD_SECTOR 512u
#define FIRST 0x4000u
#define END 0xff000u
#define STATE_MAGIC "MSLT"
#define READ_ERROR "CARD READ ERROR"
#define WRITE_ERROR "CARD WRITE ERROR"
#define FLASH_ERROR "FLASH ERROR"
#define DAMAGED "SLOT IS DAMAGED"
#define PROGRESS_EVERY 16u
void app_feed(void);
static uint8_t flash_data[SECTOR] __attribute__((aligned(4)));
static uint8_t file_data[SECTOR] __attribute__((aligned(4)));
static uint8_t state_block[CARD_SECTOR] __attribute__((aligned(4)));

/* What a load leaves alone, as MIDI mode and M-Flasher's page do. */
static bool kept(uint32_t address) {
#if APP_BOARD == 2
    return address >= 0xc0000u && address < 0xc2000u;
#else
    (void)address;
    return false;
#endif
}

static bool none(const char name[8]) {
    return !name[0];
}

static int slot_named(const struct slots *s, const char name[8]) {
    if (!none(name))
        for (unsigned i = 0; i < s->count; ++i)
            if (!memcmp(s->list[i].name, name, 8))
                return (int)i;
    return NO_SLOT;
}

int slots_pending(const struct slots *s) {
    return slot_named(s, s->pending);
}

static const char *refresh(struct slots *s) {
    if (!s->has_folder) {
        s->count = 0;
        return 0;
    }
    return fat_list(s->folder, "BIN", s->list, MOST_SLOTS, &s->count, &s->more);
}

static void hex_name(char out[11], uint32_t value) {
    memset(out, ' ', 11);
    for (unsigned i = 0; i < 8; ++i)
        out[i] = "0123456789ABCDEF"[value >> (28-4*i)&15];
}

static const char *read_state(struct slots *s) {
    bool found = false;
    const char *error = fat_find(s->folder, "STATE   DAT", &s->state_file, &found);
    if (error || !found || s->state_file.size != CARD_SECTOR)
        return error;
    if (!fat_read(&s->state_file, 0, state_block, CARD_SECTOR))
        return READ_ERROR;
    s->has_state = true;
    if (!memcmp(state_block, STATE_MAGIC, 4) && core_crc16(0, state_block, 24) == core_le16(state_block+24)) {
        memcpy(s->active, state_block+8, 8);
        memcpy(s->pending, state_block+16, 8);
    }
    return 0;
}

/* One sector of the card, so it's either as it was or as it now is. */
static const char *write_state(struct slots *s) {
    memset(state_block, 0, CARD_SECTOR);
    memcpy(state_block, STATE_MAGIC, 4);
    state_block[4] = 1;
    memcpy(state_block+8, s->active, 8);
    memcpy(state_block+16, s->pending, 8);
    core_put_le16(state_block+24, core_crc16(0, state_block, 24));
    if (s->has_state)
        return fat_write(s->state_file.start, 0, state_block, CARD_SECTOR) ? 0:WRITE_ERROR;
    uint32_t start = 0;
    const char *error = fat_reserve(CARD_SECTOR, &start);
    if (error)
        return error;
    if (!fat_write(start, 0, state_block, CARD_SECTOR))
        return WRITE_ERROR;
    error = fat_add(s->folder, "STATE   DAT", FAT_ATTR_ARCHIVE, start, CARD_SECTOR);
    if (error)
        return error;
    s->state_file.start = start;
    s->state_file.size = CARD_SECTOR;
    s->has_state = true;
    return 0;
}

const char *slots_open(struct slots *s, uint32_t card_sectors, const uint8_t uid[8]) {
    s->count = 0;
    s->more = false;
    s->has_folder = false;
    s->has_state = false;
    memset(s->active, 0, 8);
    memset(s->pending, 0, 8);
    hex_name(s->unit_name, core_crc32(0xffffffffu, uid, 8)^0xffffffffu);
    const char *error = fat_mount(card_sectors);
    if (error)
        return error;
    /* Nothing's made until there's something to save. */
    static const char missing[] = "MISSING";
    uint32_t c = fat_root();
    error = fat_folder(&c, "MBOOT      ", missing);
    if (!error)
        error = fat_folder(&c, "SLOTS      ", missing);
    if (!error)
        error = fat_folder(&c, s->unit_name, missing);
    if (error == missing)
        return 0;
    if (error)
        return error;
    s->folder = c;
    s->has_folder = true;
    error = read_state(s);
    return error ? error:refresh(s);
}

static const char *make_folders(struct slots *s) {
    if (s->has_folder)
        return 0;
    uint32_t c = fat_root();
    const char *error = fat_make_folder(&c, "MBOOT      ");
    if (!error)
        error = fat_make_folder(&c, "SLOTS      ");
    if (!error)
        error = fat_make_folder(&c, s->unit_name);
    if (error)
        return error;
    s->folder = c;
    s->has_folder = true;
    return 0;
}

static bool file_read(void *ctx, uint32_t address, void *out, uint32_t size) {
    app_feed();
    return fat_read(ctx, address, out, size);
}

static bool on_flash(void *ctx, uint32_t address, void *out, uint32_t size) {
    app_feed();
    return flash_read(ctx, address, out, size);
}

/* Whether an image and the flash hold the same thing, as far as a load is concerned. */
static const char *same_as_flash(const struct fat_entry *file, bool *same) {
    *same = false;
    if (file->size != SLOT_SIZE)
        return 0;
    for (uint32_t at = FIRST; at < END; at += SECTOR) {
        if (kept(at))
            continue;
        app_feed();
        if (!fat_read(file, at, file_data, SECTOR))
            return READ_ERROR;
        if (!flash_read(0, at, flash_data, SECTOR))
            return FLASH_ERROR;
        if (memcmp(file_data, flash_data, SECTOR))
            return 0;
    }
    *same = true;
    return 0;
}

int slots_on_unit(struct slots *s) {
    /* The active slot is the likely one, and differing slots stop at their first sector. */
    int active = slot_named(s, s->active);
    bool same = false;
    if (active != NO_SLOT && !same_as_flash(&s->list[active], &same) && same)
        return active;
    for (unsigned i = 0; i < s->count; ++i)
        if ((int)i != active && !same_as_flash(&s->list[i], &same) && same)
            return (int)i;
    return NO_SLOT;
}

/* The whole flash into a new chain, read back before anything names it. On error the chain is gone again. */
static const char *copy_flash(struct slots *s, uint32_t *start) {
    const char *error = fat_reserve(SLOT_SIZE, start);
    if (error)
        return error;
    for (uint32_t at = 0; at < SLOT_SIZE && !error; at += SECTOR) {
        if (!(at/SECTOR%PROGRESS_EVERY) && s->progress)
            s->progress("SAVING", at/SECTOR, SLOT_SIZE/SECTOR);
        app_feed();
        if (!flash_read(0, at, flash_data, SECTOR))
            error = FLASH_ERROR;
        else if (!fat_write(*start, at, flash_data, SECTOR))
            error = WRITE_ERROR;
    }
    const struct fat_entry copy = {.start = *start, .size = SLOT_SIZE};
    for (uint32_t at = 0; at < SLOT_SIZE && !error; at += SECTOR) {
        if (!(at/SECTOR%PROGRESS_EVERY) && s->progress)
            s->progress("CHECKING", at/SECTOR, SLOT_SIZE/SECTOR);
        app_feed();
        if (!fat_read(&copy, at, file_data, SECTOR))
            error = READ_ERROR;
        else if (!flash_read(0, at, flash_data, SECTOR))
            error = FLASH_ERROR;
        else if (memcmp(file_data, flash_data, SECTOR))
            error = "CARD WROTE WRONGLY";
    }
    if (error) {
        fat_release(*start);
        fat_commit();
    }
    return error;
}

static void file_name(char out[11], const char name[8]) {
    memcpy(out, name, 8);
    memcpy(out+8, "BIN", 3);
}

static void free_name(const struct slots *s, char name[8]) {
    memcpy(name, "SLOT00  ", 8);
    for (unsigned n = 1; n < 100; ++n) {
        name[4] = (char)('0'+n/10);
        name[5] = (char)('0'+n%10);
        if (slot_named(s, name) == NO_SLOT)
            return;
    }
}

const char *slots_save_new(struct slots *s, char name[8]) {
    if (s->count >= MOST_SLOTS)
        return "TOO MANY SLOTS";
    const char *error = make_folders(s);
    if (error)
        return error;
    uint32_t start = 0;
    error = copy_flash(s, &start);
    if (error)
        return error;
    char full[11];
    free_name(s, name);
    file_name(full, name);
    error = fat_add(s->folder, full, FAT_ATTR_ARCHIVE, start, SLOT_SIZE);
    if (error) {
        /* Nothing names it, so it would only be lost. */
        fat_release(start);
        fat_commit();
        return error;
    }
    memcpy(s->active, name, 8);
    error = write_state(s);
    if (!error)
        error = fat_commit();
    return error ? error:refresh(s);
}

static const char *save_over(struct slots *s, unsigned index) {
    uint32_t start = 0;
    const char *error = copy_flash(s, &start);
    if (error)
        return error;
    char full[11];
    file_name(full, s->list[index].name);
    error = fat_swap(s->folder, full, start, SLOT_SIZE);
    if (!error)
        error = fat_commit();
    return error ? error:refresh(s);
}

/* What's on the unit, saved unless a slot holds it, or it isn't valid: an interrupted load can leave that, and the slot
 * being loaded then holds the only good copy. Settings go over the slot they came from, but firmware that isn't that
 * slot's gets its own. So does what's on the unit when that slot is the one being loaded, or there'd be nothing to go back to. */
static const char *save_first(struct slots *s, const char target[8]) {
    int here = slots_on_unit(s);
    if (here != NO_SLOT || !s->valid(on_flash, 0))
        return 0;
    int active = slot_named(s, s->active);
    if (active != NO_SLOT && memcmp(s->active, target, 8) && s->list[active].size == SLOT_SIZE) {
        if (!fat_read(&s->list[active], FIRST, file_data, SECTOR))
            return READ_ERROR;
        if (!flash_read(0, FIRST, flash_data, SECTOR))
            return FLASH_ERROR;
        if (!memcmp(file_data, flash_data, SECTOR))
            return save_over(s, (unsigned)active);
    }
    char ignored[8];
    return slots_save_new(s, ignored);
}

/* The first firmware sector is erased first and written last, so what's interrupted can't pass as firmware. */
static const char *write_sector(const struct fat_entry *file, uint32_t at, bool erased) {
    if (!fat_read(file, at, file_data, SECTOR))
        return READ_ERROR;
    if (!flash_read(0, at, flash_data, SECTOR))
        return FLASH_ERROR;
    if (!memcmp(file_data, flash_data, SECTOR) && !erased)
        return 0;
    app_feed();
    if (!erased && !flash_erase(0, at, SECTOR))
        return FLASH_ERROR;
    if (!flash_program(0, at, file_data, SECTOR))
        return FLASH_ERROR;
    if (!flash_read(0, at, flash_data, SECTOR) || memcmp(file_data, flash_data, SECTOR))
        return FLASH_ERROR;
    return 0;
}

const char *slots_load(struct slots *s, unsigned index) {
    if (index >= s->count)
        return DAMAGED;
    /* Names, as saving can move things in the list. */
    char target[8];
    memcpy(target, s->list[index].name, 8);
    /* This takes a few seconds, before there's anything else to show. */
    if (s->progress)
        s->progress("CHECKING THE SLOT", 0, 1);
    if (s->list[index].size != SLOT_SIZE || !s->valid(file_read, &s->list[index]))
        return DAMAGED;
    const char *error = save_first(s, target);
    if (error)
        return error;
    int moved = slot_named(s, target);
    if (moved == NO_SLOT)
        return DAMAGED;
    const struct fat_entry file = s->list[moved];
    memcpy(s->pending, target, 8);
    error = write_state(s);
    if (error)
        return error;
    /* Only what differs, with nothing erased until something does. */
    bool any = false;
    for (uint32_t at = FIRST; at < END && !any; at += SECTOR) {
        if (kept(at))
            continue;
        app_feed();
        if (!fat_read(&file, at, file_data, SECTOR))
            return READ_ERROR;
        if (!flash_read(0, at, flash_data, SECTOR))
            return FLASH_ERROR;
        any = memcmp(file_data, flash_data, SECTOR) != 0;
    }
    if (any) {
        app_feed();
        if (!flash_erase(0, FIRST, SECTOR))
            return FLASH_ERROR;
        for (uint32_t at = FIRST+SECTOR; at < END; at += SECTOR) {
            if (kept(at))
                continue;
            if (!(at/SECTOR%PROGRESS_EVERY) && s->progress)
                s->progress("WRITING", at/SECTOR, END/SECTOR);
            error = write_sector(&file, at, false);
            if (error)
                return error;
        }
        error = write_sector(&file, FIRST, true);
        if (error)
            return error;
    }
    memcpy(s->active, target, 8);
    memset(s->pending, 0, 8);
    error = write_state(s);
    return error ? error:fat_commit();
}

const char *slots_delete(struct slots *s, unsigned index) {
    if (index >= s->count)
        return DAMAGED;
    char name[8], full[11];
    memcpy(name, s->list[index].name, 8);
    file_name(full, name);
    const char *error = fat_remove(s->folder, full);
    if (error)
        return error;
    if (!memcmp(s->active, name, 8))
        memset(s->active, 0, 8);
    if (!memcmp(s->pending, name, 8))
        memset(s->pending, 0, 8);
    error = s->has_state ? write_state(s):0;
    if (!error)
        error = fat_commit();
    return error ? error:refresh(s);
}
