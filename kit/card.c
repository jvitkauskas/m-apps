/* SPDX-License-Identifier: MIT */
/* The card shares the panel's SPI1, with DO on the panel's D/C and CS the inverse of the panel's CS, so the card is
 * selected while that's high (m-bootloader's docs/SPEC.md §11.5 for the FM-1).
 * FM-1: CLK PC9, DI PC10, DO PC8, CS = NOT PC7. SMK-37: CLK PB6, DI PB7, DO PB5, CS = NOT PC8. */
#include "card.h"
#include "log.h"
#include "platform.h"
#define SPI1 0x11d00u
#if APP_BOARD == 2
#define CS_PORT 2
#define CS_PIN 8
#define DO_PORT 1
#define DO_PIN 5
#else
#define CS_PORT 2
#define CS_PIN 7
#define DO_PORT 2
#define DO_PIN 8
#endif
#define RETRIES 3u
/* Single-block commands, and the wait for a block's start or for a write to finish. */
#define TOKEN_WAIT 200000u
#define BUSY_WAIT 2000000u
/* The log only holds so much. */
#define FAILURES_LOGGED 12u
/* A card that has stopped answering can take two restarts: seen on a 32 GB SDHC card, below. */
#define START_ATTEMPTS 3u
static uint16_t crc_table[256];
static bool block_addressed;
static unsigned failures_logged, clock_mhz;
static uint8_t probe_block[512] __attribute__((aligned(4)));

static uint8_t x(uint8_t b) {
    REG(SPI1) |= BIT(14);
    REG(SPI1+8) = b;
    sync_cpu();
    for (unsigned n = 0; n < 400000 && !(REG(SPI1)&BIT(15)); ++n)
        ;
    return (uint8_t)REG(SPI1+8);
}

static void select_card(bool on) {
    gpio_level(CS_PORT, CS_PIN, on);
}

/* A card needs 8 more clocks to finish a transaction, and they may not come soon from the next command. */
static void end_transaction(void) {
    select_card(false);
    x(0xff);
}

static uint8_t crc7(const uint8_t *p, unsigned n) {
    uint8_t c = 0;
    while (n--) {
        uint8_t d = *p++;
        for (unsigned i = 0; i < 8; ++i, d = (uint8_t)(d << 1)) {
            c = (uint8_t)(c << 1);
            if ((d^c)&0x80)
                c ^= 0x09;
        }
    }
    return c&0x7f;
}

static uint16_t crc16(const uint8_t *p, unsigned n) {
    uint16_t c = 0;
    while (n--)
        c = (uint16_t)(c << 8)^crc_table[(uint8_t)(c >> 8)^*p++];
    return c;
}

static uint8_t command(uint8_t c, uint32_t argument) {
    uint8_t frame[5] = {(uint8_t)(0x40|c), (uint8_t)(argument >> 24), (uint8_t)(argument >> 16), (uint8_t)(argument >> 8),
                        (uint8_t)argument};
    select_card(false);
    x(0xff);
    select_card(true);
    x(0xff);
    for (unsigned i = 0; i < 5; ++i)
        x(frame[i]);
    x((uint8_t)(crc7(frame, 5) << 1 | 1));
    uint8_t r = 0xff;
    for (unsigned i = 0; i < 16 && ((r = x(0xff))&0x80); ++i)
        ;
    return r;
}

void card_open(unsigned lsb_mhz) {
    /* A panel NOP (D/C low, CS low), so it ignores what follows; then D/C is the card's DO, at about 400 kHz. */
    gpio_level(DO_PORT, DO_PIN, false);
    select_card(false);
    REG(SPI1) = 0x4021;
    x(0);
    gpio_input(DO_PORT, DO_PIN, true, false);
    REG(SPI1) = 0x4029;
    REG(SPI1+4) = lsb_mhz*5/2-1;
}

void card_close(void) {
    end_transaction();
    REG(SPI1) = 0x4021;
    REG(SPI1+4) = 4;
    gpio_output(DO_PORT, DO_PIN, true);
}

/* Wiring decides what works: an FM-1 prototype with long wires managed 8 MHz but not 12, and an SMK-37 16 but not 24.
 * CRCs catch a clock that's too fast, so startup takes the fastest that reads cleanly, and later errors step it down.
 * There's no step at 16, as it didn't make usbdisk faster, and it would run closer to where a card fails. */
static const uint8_t clocks[] = {24, 12, 8, 6, 4};
#define CLOCKS (sizeof(clocks)/sizeof(clocks[0]))
static unsigned clock_step;
static bool clock_settled;

/* SPI1's clock is LSB/(baud+1). This divides LSB by enough to stay at or below a step, so 24 gives 20 MHz from LSB 60. */
static unsigned divider(unsigned lsb_mhz) {
    unsigned mhz = clocks[clock_step];
    return (lsb_mhz+mhz-1)/mhz;
}

void card_fast(unsigned lsb_mhz) {
    REG(SPI1+4) = divider(lsb_mhz)-1;
}

static unsigned real_mhz(void) {
    return clock_mhz/divider(clock_mhz);
}

static void slow_down(void) {
    if (clock_step+1 < CLOCKS) {
        ++clock_step;
        card_fast(clock_mhz);
        app_log("card: too many errors, now %u MHz", real_mhz());
    }
}

static void log_failure(const char *what, uint32_t sector, unsigned detail, unsigned more) {
    if (failures_logged < FAILURES_LOGGED) {
        ++failures_logged;
        app_log("card %s failed: sector %x, %x %x", what, (unsigned)sector, detail, more);
    }
}

/* Keeping the card's DI high between bytes, as it was left by the last x(0xff). */
static void receive_start(uint8_t *p, unsigned n) {
    REG(SPI1) |= BIT(14) | BIT(12);
    REG(SPI1+0x0c) = DMA_ADDRESS(p);
    sync_cpu();
    REG(SPI1+0x10) = n;
}

static void receive_finish(void) {
    for (unsigned i = 0; i < 400000 && !(REG(SPI1)&BIT(15)); ++i)
        ;
    REG(SPI1) &= ~BIT(12);
    REG(SPI1) |= BIT(14);
    __asm__ volatile("ssync" ::: "memory");
}

static bool block_token(uint32_t sector) {
    uint8_t r = 0xff;
    for (unsigned i = 0; i < TOKEN_WAIT && (r = x(0xff)) == 0xff; ++i)
        ;
    if (r != 0xfe)
        log_failure("token", sector, r, 0);
    return r == 0xfe;
}

/* After the data: its CRC, then the check. */
static bool block_check(const uint8_t *p, unsigned n, uint32_t sector) {
    uint16_t got = (uint16_t)(x(0xff) << 8);
    got |= x(0xff);
    uint16_t want = crc16(p, n);
    if (got != want) {
        log_failure("crc", sector, got, want);
        if (clock_settled)
            slow_down();
    }
    return got == want;
}

/* A block of n bytes after the command's answer, with its CRC checked. */
static bool read_block(uint8_t *p, unsigned n, uint32_t sector) {
    if (!block_token(sector))
        return false;
    receive_start(p, n);
    receive_finish();
    return block_check(p, n, sector);
}

static uint32_t read_size(void) {
    uint8_t csd[16];
    uint8_t answer = command(9, 0);
    if (answer)
        log_failure("size", 0, answer, 0);
    bool ok = !answer && read_block(csd, 16, 0);
    end_transaction();
    if (!ok)
        return 0;
    if (csd[0] >> 6 == 1)
        return (((uint32_t)(csd[7]&0x3f) << 16 | (uint32_t)csd[8] << 8 | csd[9])+1)*1024;
    uint32_t size = ((uint32_t)(csd[6]&3) << 10 | (uint32_t)csd[7] << 2 | csd[8] >> 6)+1;
    unsigned multiply = ((csd[9]&3u) << 1 | csd[10] >> 7)+2, block = csd[5]&15u;
    return size << multiply << block >> 9;
}

static bool start_once(void) {
    select_card(false);
    REG(SPI1+4) = clock_mhz*5/2-1;
    for (unsigned i = 0; i < 10; ++i)
        x(0xff);
    uint8_t r = 0xff;
    for (unsigned i = 0; i < 20 && (r = command(0, 0)) != 1; ++i)
        ;
    if (r != 1) {
        app_log("card: reset answered %x", r);
        return false;
    }
    /* One lost answer here would treat an SDHC card as an old one, and it would never leave idle. */
    bool v2 = false;
    for (unsigned i = 0; i < 5 && !v2; ++i) {
        uint8_t answer = command(8, 0x1aa), echo = 0;
        for (unsigned b = 0; b < 4; ++b)
            echo = x(0xff);
        v2 = answer == 1 && echo == 0xaa;
        if (answer == 5)
            break;                              /* an old card, for which this command is illegal */
    }
    for (unsigned i = 0; i < 1000; ++i) {
        if (command(55, 0) <= 1 && !(r = command(41, v2 ? BIT(30):0)))
            break;
        hal_delay_us(1000);
    }
    if (r) {
        app_log("card: start answered %x, %s", r, v2 ? "v2":"v1");
        return false;
    }
    block_addressed = false;
    if (v2 && !command(58, 0)) {
        block_addressed = (x(0xff)&0x40) != 0;
        for (unsigned i = 0; i < 3; ++i)
            x(0xff);
    }
    bool ok = !command(59, 1) && (block_addressed || !command(16, 512));
    end_transaction();
    if (!ok) {
        app_log("card: CRC or block size refused");
        return false;
    }
    card_fast(clock_mhz);
    return true;
}

static bool start(void) {
    for (unsigned attempt = 0; attempt < START_ATTEMPTS; ++attempt)
        if (start_once())
            return true;
    return false;
}

static bool probe_read(uint32_t sector) {
    bool ok = !command(17, block_addressed ? sector:sector << 9) && read_block(probe_block, 512, sector);
    end_transaction();
    return ok;
}

static uint32_t probe_le32(unsigned at) {
    return probe_block[at] | (uint32_t)probe_block[at+1] << 8 | (uint32_t)probe_block[at+2] << 16
           | (uint32_t)probe_block[at+3] << 24;
}

/* Whether this clock reads cleanly. Blank sectors prove nothing, as all zeros pass a CRC however they're misread, and
 * most of a card's first sectors are blank. So this reads what's likely to hold data: the partition table, the first
 * partition's boot sector, and the start of its FAT. A card with no filesystem must still pass, so it can be formatted,
 * and these reads are then just more checked data. */
static bool probe_reads(uint32_t size) {
    if (!probe_read(0))
        return false;
    uint32_t partition = probe_block[510] == 0x55 && probe_block[511] == 0xaa && probe_block[450] ? probe_le32(454):0;
    /* Without a partition table, those bytes mean nothing, and sector 0 may be the boot sector itself. */
    if (partition >= size)
        partition = 0;
    if (!probe_read(partition))
        return false;
    uint32_t reserved = probe_block[14] | (uint32_t)probe_block[15] << 8;
    unsigned with_data = 0;
    for (uint32_t i = 0; i < 16 && partition+reserved+i < size; ++i) {
        if (!probe_read(partition+reserved+i))
            return false;
        for (unsigned b = 0; b < 512; ++b)
            if (probe_block[b] && probe_block[b] != 0xff) {
                ++with_data;
                break;
            }
    }
    app_log("card: %u of 16 test sectors held data", with_data);
    return true;
}

uint32_t card_start(unsigned lsb_mhz) {
    clock_mhz = lsb_mhz;
    for (unsigned i = 0; i < 256; ++i) {
        uint16_t c = (uint16_t)(i << 8);
        for (unsigned b = 0; b < 8; ++b)
            c = (uint16_t)(c&0x8000 ? (uint16_t)(c << 1)^0x1021:(uint16_t)(c << 1));
        crc_table[i] = c;
    }
    clock_settled = false;
    for (clock_step = 0; clock_step < CLOCKS; ++clock_step) {
        app_log("card: trying %u MHz", real_mhz());
        if (!start())
            continue;
        uint32_t size = read_size();
        if (size && probe_reads(size)) {
            clock_settled = true;
            app_log("card: %u MHz", real_mhz());
            return size;
        }
    }
    clock_step = CLOCKS-1;
    return 0;
}

void card_finish(void) {
    /* The restart clears whatever the card was left holding, as the bootloader starts it only once. Its commands
     * have no CRCs, so that goes too. */
    if (start()) {
        command(59, 0);
        end_transaction();
    }
}

/* A rejected read of the card's last sector, after reading up to it with gaps of a few ms, leaves this card answering
 * "illegal command" to everything until it's restarted. It looks like read-ahead running off the end of the card. A
 * restart costs little, so any failure gets one, and the host never sees it. */
bool card_read(uint32_t sector, uint8_t *data) {
    for (unsigned attempt = 0; attempt < RETRIES; ++attempt) {
        uint8_t answer = command(17, block_addressed ? sector:sector << 9);
        if (answer)
            log_failure("read", sector, answer, 0);
        bool ok = !answer && read_block(data, 512, sector);
        end_transaction();
        if (ok)
            return true;
        if (answer && !start())
            return false;
    }
    return false;
}

static uint32_t stream_at;

bool card_stream_start(uint32_t sector) {
    uint8_t answer = command(18, block_addressed ? sector:sector << 9);
    if (answer) {
        log_failure("stream", sector, answer, 0);
        end_transaction();
        if (answer&0x7e)
            start();
        return false;
    }
    stream_at = sector;
    return true;
}

bool card_stream_begin(uint8_t *data) {
    if (!block_token(stream_at))
        return false;
    receive_start(data, 512);
    return true;
}

bool card_stream_finish(uint8_t *data) {
    receive_finish();
    return block_check(data, 512, stream_at++);
}

void card_stream_stop(void) {
    /* CMD12's answer comes after a stuffing byte, then the card may be busy for a while. The card stays selected. */
    uint8_t frame[5] = {0x40|12, 0, 0, 0, 0};
    for (unsigned i = 0; i < 5; ++i)
        x(frame[i]);
    x((uint8_t)(crc7(frame, 5) << 1 | 1));
    x(0xff);
    uint8_t r = 0xff;
    for (unsigned i = 0; i < 16 && ((r = x(0xff))&0x80); ++i)
        ;
    bool ready = false;
    for (unsigned i = 0; i < BUSY_WAIT && !ready; ++i)
        ready = x(0xff) == 0xff;
    end_transaction();
    if ((r&0x7e) || !ready) {
        log_failure("stream stop", stream_at, r, ready);
        start();
    }
}

static bool wait_ready(void) {
    for (unsigned i = 0; i < BUSY_WAIT; ++i)
        if (x(0xff) == 0xff)
            return true;
    return false;
}

bool card_write(uint32_t sector, const uint8_t *data) {
    uint16_t crc = crc16(data, 512);
    for (unsigned attempt = 0; attempt < RETRIES; ++attempt) {
        bool ok = false;
        uint8_t answer = command(24, block_addressed ? sector:sector << 9);
        if (!answer) {
            x(0xff);
            x(0xfe);
            for (unsigned i = 0; i < 512; ++i)
                x(data[i]);
            x((uint8_t)(crc >> 8));
            x((uint8_t)crc);
            uint8_t accepted = x(0xff)&0x1f;
            ok = accepted == 0x05 && wait_ready();
            if (!ok) {
                log_failure("write", sector, accepted, 0);
                slow_down();
            }
        } else
            log_failure("write command", sector, answer, 0);
        /* The card can report a failure only after the write, so ask. */
        if (ok) {
            uint8_t first = command(13, 0), second = x(0xff);
            ok = !first && !second;
            if (!ok)
                log_failure("write status", sector, first, second);
        }
        end_transaction();
        if (ok)
            return true;
        if (!start())
            return false;
    }
    return false;
}

static uint16_t write_crc;
static uint32_t write_at;

bool card_write_stream_start(uint32_t sector) {
    uint8_t answer = command(25, block_addressed ? sector:sector << 9);
    if (answer) {
        log_failure("write stream", sector, answer, 0);
        end_transaction();
        start();
        return false;
    }
    x(0xff);
    write_at = sector;
    return true;
}

void card_write_stream_begin(const uint8_t *data) {
    write_crc = crc16(data, 512);
    x(0xfc);
    REG(SPI1) |= BIT(14);
    REG(SPI1+0x0c) = DMA_ADDRESS(data);
    sync_cpu();
    REG(SPI1+0x10) = 512;
}

bool card_write_stream_finish(void) {
    for (unsigned i = 0; i < 400000 && !(REG(SPI1)&BIT(15)); ++i)
        ;
    REG(SPI1) |= BIT(14);
    x((uint8_t)(write_crc >> 8));
    x((uint8_t)write_crc);
    uint8_t accepted = x(0xff)&0x1f;
    bool ok = accepted == 0x05 && wait_ready();
    if (!ok) {
        log_failure("write stream data", write_at, accepted, 0);
        slow_down();
    }
    ++write_at;
    return ok;
}

bool card_write_stream_stop(void) {
    x(0xfd);
    x(0xff);
    bool ok = wait_ready();
    if (ok) {
        uint8_t first = command(13, 0), second = x(0xff);
        ok = !first && !second;
        if (!ok)
            log_failure("write stream status", write_at, first, second);
    } else
        log_failure("write stream stop", write_at, 0, 0);
    end_transaction();
    if (!ok)
        start();
    return ok;
}
