/* SPDX-License-Identifier: MIT */
#include "fat.h"
#include "card.h"
#include "core.h"
#include <string.h>
#define SECTOR 512u
#define ENTRY 32u
#define FAT_PER_SECTOR 128u
/* FAT32's limit for a directory: 2 MiB. */
#define MOST_ENTRIES 65536u
#define NO_SECTOR 0xffffffffu
#define READ_ERROR "CARD READ ERROR"
#define NOT_FAT32 "CARD IS NOT FAT32"
#define BAD_FOLDER "BAD FOLDER ON CARD"
void app_feed(void);
static uint8_t block[SECTOR] __attribute__((aligned(4)));
static uint8_t fat_block[SECTOR] __attribute__((aligned(4)));
static uint32_t fat_at, fat_cached, data_at, clusters, root;
static unsigned per_cluster;

static bool looks_fat32(const uint8_t *b) {
    unsigned per = b[13];
    return core_le16(b+510) == 0xaa55 && core_le16(b+11) == SECTOR && per && !(per&(per-1)) && core_le16(b+14)
           && (b[16] == 1 || b[16] == 2) && !core_le16(b+17) && !core_le16(b+19) && !core_le16(b+22) && core_le32(b+36);
}

const char *fat_mount(uint32_t sectors) {
    uint32_t base = 0;
    if (!card_read(0, block))
        return READ_ERROR;
    if (!looks_fat32(block)) {
        if (core_le16(block+510) != 0xaa55)
            return NOT_FAT32;
        base = core_le32(block+454);
        if (base >= sectors)
            return NOT_FAT32;
        if (!card_read(base, block))
            return READ_ERROR;
        if (!looks_fat32(block))
            return NOT_FAT32;
    }
    uint32_t total = core_le32(block+32), reserved = core_le16(block+14), fat_size = core_le32(block+36), fats = block[16];
    if (total > sectors-base || fat_size > total/fats || reserved+fats*fat_size >= total)
        return NOT_FAT32;
    per_cluster = block[13];
    fat_at = base+reserved;
    data_at = fat_at+fats*fat_size;
    clusters = (total-reserved-fats*fat_size)/per_cluster;
    /* Past FAT32's 28 bits, a cluster number can't be told from its end-of-chain mark. */
    if (!clusters || clusters > 0x0ffffff0u || fat_size > 0xffffffffu/FAT_PER_SECTOR || fat_size*FAT_PER_SECTOR < clusters+2)
        return NOT_FAT32;
    root = core_le32(block+44);
    fat_cached = NO_SECTOR;
    return 0;
}

uint32_t fat_root(void) {
    return root;
}

static bool cluster_ok(uint32_t c) {
    return c >= 2 && c-2 < clusters;
}

static bool is_end(uint32_t c) {
    return c >= 0x0ffffff8u;
}

/* The next cluster, a value that isn't one, or 0 if that can't be read. */
static uint32_t next(uint32_t c) {
    uint32_t sector = fat_at+c/FAT_PER_SECTOR;
    if (sector != fat_cached) {
        fat_cached = NO_SECTOR;
        if (!card_read(sector, fat_block))
            return 0;
        fat_cached = sector;
    }
    return core_le32(fat_block+c%FAT_PER_SECTOR*4)&0x0fffffffu;
}

struct walk {
    const char *match;
    uint32_t found;
    bool is_found;
    struct fat_entry *entries;
    unsigned max, count;
    bool more;
};

static uint32_t entry_start(const uint8_t *e) {
    return core_le16(e+26) | (uint32_t)core_le16(e+20) << 16;
}

static void visit_folder(struct walk *w, const uint8_t *e) {
    if (e[11]&0x10 && !memcmp(e, w->match, 11)) {
        w->found = entry_start(e);
        w->is_found = true;
    }
}

static void visit_file(struct walk *w, const uint8_t *e) {
    if (e[11]&0x10 || memcmp(e+8, w->match, 3))
        return;
    unsigned at = 0;
    while (at < w->count && memcmp(w->entries[at].name, e, 8) < 0)
        ++at;
    if (w->count == w->max) {
        w->more = true;
        if (at == w->max)
            return;
        --w->count;
    }
    memmove(w->entries+at+1, w->entries+at, (w->count-at)*sizeof(struct fat_entry));
    memcpy(w->entries[at].name, e, 8);
    w->entries[at].start = entry_start(e);
    w->entries[at].size = core_le32(e+28);
    ++w->count;
}

static const char *walk(uint32_t c, void (*visit)(struct walk *, const uint8_t *), struct walk *w) {
    unsigned seen = 0;
    if (!cluster_ok(c))
        return BAD_FOLDER;
    for (;;) {
        for (unsigned s = 0; s < per_cluster; ++s) {
            if (!card_read(data_at+(c-2)*per_cluster+s, block))
                return READ_ERROR;
            for (unsigned at = 0; at < SECTOR; at += ENTRY) {
                const uint8_t *e = block+at;
                if (!e[0])
                    return 0;
                if (++seen > MOST_ENTRIES)
                    return BAD_FOLDER;
                /* Deleted, or a volume label or a piece of a long name. */
                if (e[0] != 0xe5 && !(e[11]&0x08)) {
                    visit(w, e);
                    if (w->is_found)
                        return 0;
                }
            }
        }
        app_feed();
        uint32_t following = next(c);
        if (!following)
            return READ_ERROR;
        if (is_end(following))
            return 0;
        if (!cluster_ok(following))
            return BAD_FOLDER;
        c = following;
    }
}

const char *fat_folder(uint32_t *cluster, const char name[11], const char *missing) {
    struct walk w = {.match = name};
    const char *error = walk(*cluster, visit_folder, &w);
    if (error)
        return error;
    if (!w.is_found)
        return missing;
    *cluster = w.found;
    return 0;
}

const char *fat_list(uint32_t cluster, const char extension[3], struct fat_entry *entries, unsigned max, unsigned *count,
                     bool *more) {
    struct walk w = {.match = extension, .entries = entries, .max = max};
    const char *error = walk(cluster, visit_file, &w);
    *count = w.count;
    *more = w.more;
    return error;
}

bool fat_read(const struct fat_entry *file, uint32_t offset, void *to, uint32_t n) {
    if (offset > file->size || n > file->size-offset)
        return false;
    uint8_t *out = to;
    uint32_t c = file->start, cluster_bytes = per_cluster*SECTOR;
    if (!cluster_ok(c))
        return !n;
    for (uint32_t skip = offset/cluster_bytes; skip; --skip) {
        c = next(c);
        if (!cluster_ok(c))
            return false;
    }
    offset %= cluster_bytes;
    while (n) {
        uint32_t sector = data_at+(c-2)*per_cluster+offset/SECTOR;
        uint32_t in_sector = offset%SECTOR, part = SECTOR-in_sector;
        if (part > n)
            part = n;
        if (part == SECTOR && !((uintptr_t)out&3)) {
            /* The card's DMA goes straight to the target. */
            if (!card_read(sector, out))
                return false;
        } else {
            if (!card_read(sector, block))
                return false;
            memcpy(out, block+in_sector, part);
        }
        out += part;
        offset += part;
        n -= part;
        if (offset == cluster_bytes && n) {
            app_feed();
            offset = 0;
            c = next(c);
            if (!cluster_ok(c))
                return false;
        }
    }
    return true;
}
