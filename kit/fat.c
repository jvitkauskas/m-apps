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
#define WRITE_ERROR "CARD WRITE ERROR"
#define CARD_FULL "CARD IS FULL"
#define FSINFO_SIGNATURE 0x41615252u
#define FSINFO_SIGNATURE2 0x61417272u
#define NO_COUNT 0xffffffffu
#define END_OF_CHAIN 0x0fffffffu
/* No clock on the unit: 2026-01-01. */
#define DOS_DATE 0x5c21u
void app_feed(void);
static uint8_t block[SECTOR] __attribute__((aligned(4)));
static uint8_t fat_block[SECTOR] __attribute__((aligned(4)));
static uint32_t fat_at, fat_cached, data_at, clusters, root, fat_sectors, info_sector, first_free;
static unsigned per_cluster, fat_copies;
static bool fat_dirty;
static uint32_t cursor_start, cursor_index, cursor_cluster;
static int freed;

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
    fat_sectors = fat_size;
    fat_copies = fats;
    info_sector = core_le16(block+48) ? base+core_le16(block+48):0;
    fat_cached = NO_SECTOR;
    fat_dirty = false;
    cursor_cluster = 0;
    first_free = 2;
    freed = 0;
    if (info_sector) {
        if (!card_read(info_sector, block))
            return READ_ERROR;
        uint32_t hint = core_le32(block+492);
        if (core_le32(block) == FSINFO_SIGNATURE && core_le32(block+484) == FSINFO_SIGNATURE2 && hint >= 2 && hint-2 < clusters)
            first_free = hint;
    }
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

/* The cached sector, in every copy of the FAT. */
static bool flush_fat(void) {
    if (!fat_dirty)
        return true;
    for (unsigned copy = 0; copy < fat_copies; ++copy)
        if (!card_write(fat_cached+copy*fat_sectors, fat_block))
            return false;
    fat_dirty = false;
    return true;
}

/* The next cluster, a value that isn't one, or 0 if that can't be read. */
static uint32_t next(uint32_t c) {
    uint32_t sector = fat_at+c/FAT_PER_SECTOR;
    if (sector != fat_cached) {
        if (!flush_fat())
            return 0;
        fat_cached = NO_SECTOR;
        if (!card_read(sector, fat_block))
            return 0;
        fat_cached = sector;
    }
    return core_le32(fat_block+c%FAT_PER_SECTOR*4)&0x0fffffffu;
}

/* Keeps the top four bits, which FAT32 reserves. */
static bool set_next(uint32_t c, uint32_t value) {
    if (c == 0 || (next(c) == 0 && fat_cached != fat_at+c/FAT_PER_SECTOR))
        return false;
    uint8_t *e = fat_block+c%FAT_PER_SECTOR*4;
    core_put_le32(e, (core_le32(e)&0xf0000000u)|value);
    fat_dirty = true;
    return true;
}

struct walk {
    const char *match;
    uint32_t found, size, sector;
    unsigned at;
    bool is_found;
    struct fat_entry *entries;
    unsigned max, count;
    bool more;
};

static uint32_t entry_start(const uint8_t *e) {
    return core_le16(e+26) | (uint32_t)core_le16(e+20) << 16;
}

static void visit_folder(struct walk *w, const uint8_t *e) {
    if (e[11]&FAT_ATTR_FOLDER && !memcmp(e, w->match, 11)) {
        w->found = entry_start(e);
        w->is_found = true;
    }
}

static void visit_name(struct walk *w, const uint8_t *e) {
    if (!memcmp(e, w->match, 11)) {
        w->found = entry_start(e);
        w->size = core_le32(e+28);
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
            w->sector = data_at+(c-2)*per_cluster+s;
            if (!card_read(w->sector, block))
                return READ_ERROR;
            for (unsigned at = 0; at < SECTOR; at += ENTRY) {
                w->at = at;
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
    uint32_t c = file->start, cluster_bytes = per_cluster*SECTOR, index = offset/cluster_bytes, skip = index;
    if (!cluster_ok(c))
        return !n;
    /* Sequential reads go on from the last, as walking a long file's chain each time would get slower and slower. */
    if (cursor_start == file->start && cursor_index <= index && cursor_cluster) {
        c = cursor_cluster;
        skip = index-cursor_index;
    }
    for (; skip; --skip) {
        c = next(c);
        if (!cluster_ok(c))
            return false;
    }
    cursor_start = file->start;
    cursor_index = index;
    cursor_cluster = c;
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

const char *fat_reserve(uint32_t size, uint32_t *start) {
    uint32_t cluster_bytes = per_cluster*SECTOR, need = size/cluster_bytes+(size%cluster_bytes != 0);
    /* An empty file has no chain. */
    *start = 0;
    if (!need)
        return 0;
    /* First, without changing anything, whether there's room. */
    uint32_t found = 0, c = first_free;
    for (uint32_t looked = 0; looked < clusters && found < need; ++looked) {
        if (!(looked&0xfff))
            app_feed();
        uint32_t value = next(c);
        if (!value && fat_cached != fat_at+c/FAT_PER_SECTOR)
            return READ_ERROR;
        if (!value)
            ++found;
        if (++c >= clusters+2)
            c = 2;
    }
    if (found < need)
        return CARD_FULL;
    uint32_t first = 0, last = 0;
    c = first_free;
    uint32_t looked = 0;
    for (found = 0; found < need; ) {
        if (!(looked++&0xfff))
            app_feed();
        uint32_t value = next(c);
        if (!value && fat_cached != fat_at+c/FAT_PER_SECTOR)
            return READ_ERROR;
        if (!value) {
            if (!set_next(c, END_OF_CHAIN) || (last && !set_next(last, c)))
                return WRITE_ERROR;
            if (!first)
                first = c;
            last = c;
            ++found;
        }
        if (++c >= clusters+2)
            c = 2;
    }
    first_free = c;
    freed -= (int)need;
    if (!flush_fat())
        return WRITE_ERROR;
    *start = first;
    return 0;
}

bool fat_write(uint32_t start, uint32_t offset, const void *from, uint32_t n) {
    if (offset%SECTOR || n%SECTOR || !cluster_ok(start))
        return false;
    const uint8_t *in = from;
    uint32_t c = start, cluster_bytes = per_cluster*SECTOR;
    for (uint32_t skip = offset/cluster_bytes; skip; --skip) {
        c = next(c);
        if (!cluster_ok(c))
            return false;
    }
    offset %= cluster_bytes;
    while (n) {
        if (!card_write(data_at+(c-2)*per_cluster+offset/SECTOR, in))
            return false;
        in += SECTOR;
        offset += SECTOR;
        n -= SECTOR;
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

const char *fat_release(uint32_t start) {
    uint32_t c = start;
    cursor_cluster = 0;
    /* Bounded by the volume, in case the chain loops. It stops at the first cluster already free. */
    for (uint32_t n = 0; n < clusters && cluster_ok(c); ++n) {
        if (!(n&0xfff))
            app_feed();
        uint32_t following = next(c);
        if (!following)
            break;
        if (!set_next(c, 0))
            return WRITE_ERROR;
        ++freed;
        if (c < first_free)
            first_free = c;
        c = following;
    }
    return flush_fat() ? 0:WRITE_ERROR;
}

static void put_entry(uint8_t *e, const char name[11], unsigned attributes, uint32_t start, uint32_t size) {
    memset(e, 0, ENTRY);
    memcpy(e, name, 11);
    e[11] = (uint8_t)attributes;
    core_put_le16(e+16, DOS_DATE);
    core_put_le16(e+18, DOS_DATE);
    core_put_le16(e+24, DOS_DATE);
    core_put_le16(e+20, (uint16_t)(start >> 16));
    core_put_le16(e+26, (uint16_t)start);
    core_put_le32(e+28, size);
}

/* A cluster for a directory: all zeros, as a zero entry ends a directory. */
static const char *blank_cluster(uint32_t c) {
    memset(block, 0, SECTOR);
    for (unsigned s = 0; s < per_cluster; ++s)
        if (!card_write(data_at+(c-2)*per_cluster+s, block))
            return WRITE_ERROR;
    return 0;
}

const char *fat_add(uint32_t directory, const char name[11], unsigned attributes, uint32_t start, uint32_t size) {
    if (!cluster_ok(directory))
        return BAD_FOLDER;
    uint32_t c = directory, seen = 0;
    for (;;) {
        app_feed();
        for (unsigned s = 0; s < per_cluster; ++s) {
            uint32_t sector = data_at+(c-2)*per_cluster+s;
            if (!card_read(sector, block))
                return READ_ERROR;
            for (unsigned at = 0; at < SECTOR; at += ENTRY) {
                /* As walk() does, so a chain that loops ends with an error, and its FAT stays as it is. */
                if (++seen > MOST_ENTRIES)
                    return BAD_FOLDER;
                if (block[at] && block[at] != 0xe5)
                    continue;
                /* A zero entry ends the directory, so what follows it may never have been looked at. */
                bool was_end = !block[at];
                put_entry(block+at, name, attributes, start, size);
                if (was_end && at+ENTRY < SECTOR)
                    memset(block+at+ENTRY, 0, SECTOR-at-ENTRY);
                return card_write(sector, block) ? 0:WRITE_ERROR;
            }
        }
        uint32_t following = next(c);
        if (!following)
            return READ_ERROR;
        if (is_end(following))
            break;
        if (!cluster_ok(following))
            return BAD_FOLDER;
        c = following;
    }
    /* Full: a blank cluster goes on the end first, so the directory is never without its end. */
    uint32_t more = 0;
    const char *error = fat_reserve(per_cluster*SECTOR, &more);
    if (!error)
        error = blank_cluster(more);
    if (error)
        return error;
    if (!set_next(c, more) || !flush_fat())
        return WRITE_ERROR;
    uint32_t sector = data_at+(more-2)*per_cluster;
    memset(block, 0, SECTOR);
    put_entry(block, name, attributes, start, size);
    return card_write(sector, block) ? 0:WRITE_ERROR;
}

const char *fat_find(uint32_t directory, const char name[11], struct fat_entry *entry, bool *found) {
    struct walk w = {.match = name};
    const char *error = walk(directory, visit_name, &w);
    *found = !error && w.is_found;
    if (*found && entry) {
        memcpy(entry->name, name, sizeof(entry->name));
        entry->start = w.found;
        entry->size = w.size;
    }
    return error;
}

const char *fat_swap(uint32_t directory, const char name[11], uint32_t start, uint32_t size) {
    struct walk w = {.match = name};
    const char *error = walk(directory, visit_name, &w);
    if (error)
        return error;
    if (!w.is_found)
        return BAD_FOLDER;
    /* block still holds the entry's sector. This one write is what changes the file. */
    uint8_t *e = block+w.at;
    core_put_le16(e+20, (uint16_t)(start >> 16));
    core_put_le16(e+26, (uint16_t)start);
    core_put_le32(e+28, size);
    if (!card_write(w.sector, block))
        return WRITE_ERROR;
    return fat_release(w.found);
}

const char *fat_remove(uint32_t directory, const char name[11]) {
    struct walk w = {.match = name};
    const char *error = walk(directory, visit_name, &w);
    if (error)
        return error;
    if (!w.is_found)
        return BAD_FOLDER;
    block[w.at] = 0xe5;
    /* A name given on a computer can have long-name pieces just before it. Those in this sector go with it. */
    for (unsigned at = w.at; at >= ENTRY && block[at-ENTRY+11] == 0x0f; at -= ENTRY)
        block[at-ENTRY] = 0xe5;
    if (!card_write(w.sector, block))
        return WRITE_ERROR;
    return fat_release(w.found);
}

const char *fat_make_folder(uint32_t *cluster, const char name[11]) {
    struct walk w = {.match = name};
    const char *error = walk(*cluster, visit_folder, &w);
    if (error)
        return error;
    if (w.is_found) {
        *cluster = w.found;
        return 0;
    }
    uint32_t made = 0;
    error = fat_reserve(per_cluster*SECTOR, &made);
    if (!error)
        error = blank_cluster(made);
    if (error)
        return error;
    /* Its own entries, where a parent that's the root counts as cluster 0. */
    memset(block, 0, SECTOR);
    put_entry(block, ".          ", FAT_ATTR_FOLDER, made, 0);
    put_entry(block+ENTRY, "..         ", FAT_ATTR_FOLDER, *cluster == root ? 0:*cluster, 0);
    if (!card_write(data_at+(made-2)*per_cluster, block))
        return WRITE_ERROR;
    error = fat_add(*cluster, name, FAT_ATTR_FOLDER, made, 0);
    if (error)
        return error;
    *cluster = made;
    return 0;
}

const char *fat_commit(void) {
    if (!flush_fat())
        return WRITE_ERROR;
    if (!info_sector)
        return 0;
    if (!card_read(info_sector, block))
        return READ_ERROR;
    if (core_le32(block) != FSINFO_SIGNATURE || core_le32(block+484) != FSINFO_SIGNATURE2)
        return 0;
    uint32_t count = core_le32(block+488);
    if (count != NO_COUNT)
        core_put_le32(block+488, count+(uint32_t)freed);
    core_put_le32(block+492, first_free);
    freed = 0;
    return card_write(info_sector, block) ? 0:WRITE_ERROR;
}
