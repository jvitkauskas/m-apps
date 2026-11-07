/* SPDX-License-Identifier: MIT */
/* Bulk-only mass storage on USB0's MUSB controller. The controller's register sequences are those of m-bootloader's
 * src/usb_midi.c, whose comments say what they're from. Its accesses have to be kept apart, or it babbles on the bus. */
#include "msc.h"
#include "app.h"
#include "card.h"
#define CON0 0x11800u
#define CON1 0x11804u
#define EP_COUNT(n) (0x11808u+4u*(n))
#define EP0_ADDRESS 0x11818u
#define EP1_TX_ADDRESS 0x1181cu
#define EP_RX_ADDRESS(n) (0x11820u+8u*((n)-1u))
#define EP4_RX_ADDRESS 0x1183cu
#define IO_CON0 0x51000u
#define CLOCK_CON1 0x10010u
enum { FADDR = 0, POWER = 1, INT_TX1 = 2, INT_USB = 6, INT_TX1_ENABLE = 7, INT_TX2_ENABLE = 8, INT_RX1_ENABLE = 9,
    INT_RX2_ENABLE = 10, INT_USB_ENABLE = 11, INDEX = 14, TX_MAX = 16, CSR0 = 17, TX_CSR1 = 17, TX_CSR2 = 18, RX_MAX = 19,
    RX_CSR1 = 20, RX_CSR2 = 21, RX_COUNT1 = 22, RX_COUNT2 = 23 };
#define WAIT_LIMIT 20000u
#define PACKET 64u
#define EP_IN 0x81u
#define EP_OUT 0x01u
#define SECTOR 512u
void settle(unsigned n);

static const uint8_t device_descriptor[18] = {
    18, 1, 0x10, 0x01, 0, 0, 0, PACKET, 0x09, 0x12, 0x03, 0x00, 0x00, 0x01, 0, 1, 2, 1
};
static const uint8_t config_descriptor[32] = {
    9, 2, 32, 0, 1, 1, 0, 0x80, 50,
    9, 4, 0, 0, 2, 8, 6, 0x50, 0,                  /* mass storage, SCSI, bulk-only */
    7, 5, EP_IN, 2, PACKET, 0, 0,
    7, 5, EP_OUT, 2, PACKET, 0, 0
};
static const uint8_t language_string[4] = { 4, 3, 0x09, 0x04 };
static const char product[] = "FM-1 USB DISK";
static const char serial[] = "000000000001";
static const uint8_t inquiry[36] = {
    0, 0x80, 0x02, 0x02, 31, 0, 0, 0,
    'M', '-', 'A', 'P', 'P', 'S', ' ', ' ',
    'F', 'M', '-', '1', ' ', 'U', 'S', 'B', ' ', 'D', 'I', 'S', 'K', ' ', ' ', ' ',
    '1', '.', '0', ' '
};

/* The controller's DMA writes past a full packet when receiving, so these have room to spare. */
static uint8_t ep0[PACKET+4] __attribute__((aligned(4)));
static uint8_t command_block[PACKET+4] __attribute__((aligned(4)));
static uint8_t sector[SECTOR+4] __attribute__((aligned(4)));
static uint8_t sector2[SECTOR+4] __attribute__((aligned(4)));
static uint8_t reply[32] __attribute__((aligned(4)));
static uint8_t status_block[13] __attribute__((aligned(4)));
static uint8_t string_descriptor[2+2*26];
static const uint8_t *ep0_data;
static uint32_t ep0_left, total_sectors;
static uint8_t pending_address, configuration, sense_key, sense_code;
static bool address_pending, ep0_sending, zero_end, stalled_in, stalled_out, ejected, aborted, needs_reset;
static struct msc_stats stats;

static void bus_poll(void);

static __attribute__((noinline)) void sie_start(uint32_t command) {
    settle(40);
    REG(CON1) = command;
    sync_cpu();
}

static void sie_write(uint32_t r, uint32_t v) {
    if (!(REG(CON0)&4))
        return;
    sie_start((r << 8 & 0xffff00u) | (v&0xffu));
    for (uint32_t n = WAIT_LIMIT; n && !(REG(CON1)&0x8000u); --n)
        ;
}

static uint32_t sie_read(uint32_t r) {
    if (!(REG(CON0)&4))
        return 0;
    sie_start((r << 8 & 0xffbf00u) | 0x4000u);
    for (uint32_t n = WAIT_LIMIT; n; --n)
        if (REG(CON1)&0x8000u)
            return REG(CON1)&0xffu;
    return 0;
}

/* The controller's selected endpoint, so that the data path doesn't reselect it for every packet. */
static uint32_t selected = 0xff;

static void select_endpoint(uint32_t n) {
    if (selected != n) {
        selected = n;
        sie_write(INDEX, n);
    }
}

static void ep0_chunk(void) {
    uint32_t n = ep0_left > PACKET ? PACKET:ep0_left;
    for (uint32_t i = 0; i < n; ++i)
        ep0[i] = ep0_data[i];
    sync_cpu();
    REG(EP0_ADDRESS) = DMA_ADDRESS(ep0);
    REG(EP_COUNT(0)) = n;
    ep0_data += n;
    ep0_left -= n;
    bool last = !ep0_left && !(n == PACKET && zero_end);
    if (!ep0_left && n == PACKET)
        zero_end = false;
    select_endpoint(0);
    sie_write(CSR0, last ? 0x0a:0x02);
    ep0_sending = !last;
}

static void ep0_send(const uint8_t *data, uint32_t size, uint32_t asked) {
    if (size > asked)
        size = asked;
    ep0_data = data;
    ep0_left = size;
    zero_end = size < asked && !(size%PACKET);
    select_endpoint(0);
    sie_write(CSR0, 0x40);
    ep0_chunk();
}

/* Ready for the next packet, which the controller's DMA will put here. */
static void rx_arm(uint8_t *to) {
    REG(EP_RX_ADDRESS(1)) = DMA_ADDRESS(to);
    select_endpoint(1);
    uint32_t csr = sie_read(RX_CSR1) | sie_read(RX_CSR2) << 8;
    csr = (csr&~0x164u) | 0x10u;
    sie_write(RX_CSR1, csr&0xff);
    sie_write(RX_CSR2, csr >> 8);
}

static void endpoints_reset(void) {
    REG(EP1_TX_ADDRESS) = DMA_ADDRESS(reply);
    REG(EP_RX_ADDRESS(1)) = DMA_ADDRESS(command_block);
    select_endpoint(1);
    sie_write(TX_MAX, 0xff);
    sie_write(TX_CSR1, 0x48);
    sie_write(TX_CSR2, 0);
    sie_write(RX_MAX, 0xff);
    sie_write(RX_CSR1, 0x90);
    sie_write(RX_CSR2, 0);
    stalled_in = stalled_out = false;
}

static void ep1_configure(void) {
    endpoints_reset();
    sie_write(INT_RX1_ENABLE, 0x02);
    REG(CON0) &= ~(1u << 20);
}

static void stall_in(void) {
    select_endpoint(1);
    sie_write(TX_CSR1, 0x10);
    stalled_in = true;
}

static void stall_out(void) {
    select_endpoint(1);
    sie_write(RX_CSR1, 0x20);
    stalled_out = true;
}

static void clear_halt(uint32_t endpoint) {
    select_endpoint(1);
    if (endpoint == EP_IN) {
        sie_write(TX_CSR1, 0x48);
        stalled_in = false;
    } else if (endpoint == EP_OUT) {
        sie_write(RX_CSR1, 0x90);
        stalled_out = false;
    }
}

static const uint8_t *string(const char *text, uint32_t *size) {
    uint32_t n = 0;
    for (; text[n]; ++n) {
        string_descriptor[2+2*n] = (uint8_t)text[n];
        string_descriptor[3+2*n] = 0;
    }
    string_descriptor[0] = (uint8_t)(2+2*n);
    string_descriptor[1] = 3;
    *size = 2+2*n;
    return string_descriptor;
}

static const uint8_t *descriptor(uint32_t value, uint32_t *size) {
    switch (value >> 8) {
    case 1:
        *size = sizeof(device_descriptor);
        return device_descriptor;
    case 2:
        *size = sizeof(config_descriptor);
        return config_descriptor;
    case 3:
        if (!(value&255)) {
            *size = sizeof(language_string);
            return language_string;
        }
        if ((value&255) == 1)
            return string(product, size);
        if ((value&255) == 2)
            return string(serial, size);
        return 0;
    default:
        return 0;
    }
}

static void ep0_service(void) {
    static const uint8_t zero[2];
    static const uint8_t halted[2] = {1, 0};
    if (address_pending) {
        sie_write(FADDR, pending_address);
        address_pending = false;
    }
    select_endpoint(0);
    uint32_t csr = sie_read(CSR0);
    if (csr&0x04) {                             /* we sent a stall */
        sie_write(CSR0, 0);
        ep0_sending = false;
        return;
    }
    if (csr&0x10) {                             /* the host abandoned the transfer */
        sie_write(CSR0, 0x80);
        ep0_sending = false;
    }
    if (ep0_sending) {
        if (!(csr&0x02))
            ep0_chunk();
        return;
    }
    if (!(csr&0x01))
        return;
    __asm__ volatile("ssync" ::: "memory");
    uint32_t value = ep0[2] | (uint32_t)ep0[3] << 8, index = ep0[4], asked = ep0[6] | (uint32_t)ep0[7] << 8;
    switch ((uint32_t)ep0[0] << 8 | ep0[1]) {
    case 0x0005:                                /* set address */
        pending_address = ep0[2]&0x7f;
        address_pending = true;
        break;
    case 0x8006: {                              /* get descriptor */
        uint32_t size;
        const uint8_t *d = descriptor(value, &size);
        if (!d)
            goto stall;
        if (!asked)
            break;
        ep0_send(d, size, asked);
        return;
    }
    case 0x0009:                                /* set configuration */
        if (ep0[2] > 1)
            goto stall;
        configuration = ep0[2];
        if (configuration)
            ep1_configure();
        aborted = true;
        break;
    case 0x8008:
        ep0_send(&configuration, 1, asked);
        return;
    case 0x8000:
    case 0x8100:
        ep0_send(zero, 2, asked);
        return;
    case 0x8200:                                /* an endpoint's status */
        ep0_send((index == EP_IN && stalled_in) || (index == EP_OUT && stalled_out) ? halted:zero, 2, asked);
        return;
    case 0x0201:                                /* clear an endpoint's halt */
        if (!needs_reset)
            clear_halt(index);
        break;
    case 0x0203:                                /* set an endpoint's halt */
        if (index == EP_IN)
            stall_in();
        else if (index == EP_OUT)
            stall_out();
        break;
    case 0xa1fe:                                /* the highest logical unit */
        ep0_send(zero, 1, asked);
        return;
    case 0x21ff:                                /* bulk-only reset */
        needs_reset = false;
        aborted = true;
        endpoints_reset();
        break;
    case 0x0102:
    case 0x0302:
    case 0x000b:
    case 0x010b:
        break;
    default:
        goto stall;
    }
    select_endpoint(0);
    sie_write(CSR0, 0x48);
    return;
stall:
    select_endpoint(0);
    sie_write(CSR0, 0x60);
}

static void bus_poll(void) {
    if (REG(CON0)&BIT(13))                      /* a frame's start is pending: acknowledge it */
        REG(CON0) |= BIT(12);
    uint32_t bus = sie_read(INT_USB), tx = sie_read(INT_TX1);
    if (bus&0x04) {                             /* bus reset */
        selected = 0xff;
        sie_write(FADDR, 0);
        configuration = 0;
        ep0_sending = address_pending = needs_reset = false;
        aborted = true;
        REG(EP0_ADDRESS) = DMA_ADDRESS(ep0);
        sie_write(INT_USB_ENABLE, 0x07);
        sie_write(INT_TX1_ENABLE, 0x01);
        sie_write(INT_RX1_ENABLE, 0);
    }
    if (tx&0x01)
        ep0_service();
}

/* Waits for the IN endpoint to be free. False if the transfer was given up, by a reset or a lost host. */
static uint32_t tx_status;

static bool tx_wait(void) {
    /* Sending would clear a stall the host hasn't. */
    if (stalled_in)
        return false;
    for (uint32_t n = 0; !aborted && configuration; ++n) {
        select_endpoint(1);
        uint32_t csr = sie_read(TX_CSR1);
        if (!(csr&0x01)) {
            if (csr&0x80)
                sie_write(TX_CSR1, csr&~0x80u);
            tx_status = csr&~0x80u;
            return true;
        }
        if ((n&15) == 15)
            bus_poll();
        if ((n&0xfff) == 0xfff)
            app_feed();
    }
    return false;
}

static bool tx_send(const uint8_t *data, uint32_t n) {
    if (!tx_wait())
        return false;
    sync_cpu();
    REG(EP1_TX_ADDRESS) = DMA_ADDRESS(data);
    REG(EP_COUNT(1)) = n;
    sie_write(TX_CSR1, tx_status | 0x01);
    return true;
}

/* Waits for a packet, and returns its size, or -1 if the transfer was given up. */
static int rx_wait(void) {
    for (uint32_t n = 0; !aborted && configuration; ++n) {
        select_endpoint(1);
        if (sie_read(RX_CSR1)&1) {
            uint32_t size = sie_read(RX_COUNT1) | sie_read(RX_COUNT2) << 8;
            __asm__ volatile("ssync" ::: "memory");
            return (int)(size > PACKET ? PACKET:size);
        }
        if ((n&15) == 15)
            bus_poll();
        if ((n&0xfff) == 0xfff)
            app_feed();
    }
    return -1;
}

static uint32_t le32(const uint8_t *p) {
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

static uint32_t be32(const uint8_t *p) {
    return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3];
}

static void put_be32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)(v >> 24);
    p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);
    p[3] = (uint8_t)v;
}

static void put_le32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
    p[3] = (uint8_t)(v >> 24);
}

/* One command's state. */
static uint32_t tag, expected, moved;
static bool to_host;

static void fail(uint8_t key, uint8_t code) {
    sense_key = key;
    sense_code = code;
    /* The rest are hosts trying what isn't supported, which is normal. */
    if (key == 3)
        ++stats.errors;
}

/* The end of the data: if the host expected more than a short packet tells it, it gets a stall. */
static void finish_in(uint32_t last_packet) {
    if (moved < expected && (!moved || last_packet == PACKET))
        stall_in();
}

static bool send_reply(const uint8_t *data, uint32_t size) {
    if (size > expected)
        size = expected;
    uint32_t sent = 0, last = 0;
    while (sent < size) {
        last = size-sent > PACKET ? PACKET:size-sent;
        if (!tx_send(data+sent, last))
            return false;
        sent += last;
    }
    moved = sent;
    finish_in(last);
    return true;
}

/* One read at a time near the end of the card: reading ahead up to its end upsets some cards (card.c). */
#define NEAR_THE_END 256u

static bool send_sector(const uint8_t *data) {
    for (uint32_t at = 0; at < SECTOR; at += PACKET)
        if (!tx_send(data+at, PACKET))
            return false;
    return true;
}

static void read_singly(uint32_t first, uint32_t count) {
    for (uint32_t s = 0; s < count; ++s) {
        /* The last packet may still be going out of this buffer. */
        if (!tx_wait())
            return;
        if (!card_read(first+s, sector)) {
            fail(3, 0x11);
            break;
        }
        ++stats.sectors_read;
        if (!send_sector(sector))
            return;
        moved += SECTOR;
        app_feed();
    }
    finish_in(PACKET);
}

/* The card and USB can work at once, so while one sector goes out, DMA fetches the next into the other buffer. */
static void read_stream(uint32_t first, uint32_t count) {
    uint8_t *out = sector, *next = sector2;
    if (!card_stream_begin(out) || !card_stream_finish(out)) {
        card_stream_stop();
        read_singly(first, count);
        return;
    }
    bool streaming = true;
    for (uint32_t s = 0; s < count; ++s) {
        bool more = s+1 < count;
        /* The last packet may still be going out of the buffer this overwrites. */
        if (!tx_wait()) {
            if (streaming)
                card_stream_stop();
            return;
        }
        bool started = false;
        if (more && streaming) {
            started = card_stream_begin(next);
            if (!started) {
                card_stream_stop();
                streaming = false;
            }
        }
        ++stats.sectors_read;
        bool sent = send_sector(out);
        bool good = true;
        if (started && !card_stream_finish(next)) {
            good = false;
            card_stream_stop();
            streaming = false;
        }
        if (!sent) {
            if (streaming)
                card_stream_stop();
            return;
        }
        moved += SECTOR;
        app_feed();
        if (more && (!started || !good) && !card_read(first+s+1, next)) {
            fail(3, 0x11);
            finish_in(PACKET);
            return;
        }
        uint8_t *swap = out;
        out = next;
        next = swap;
    }
    if (streaming)
        card_stream_stop();
    finish_in(PACKET);
}

static void read_sectors(uint32_t first, uint32_t count) {
    if (count > 1 && total_sectors-(first+count) >= NEAR_THE_END && card_stream_start(first))
        read_stream(first, count);
    else
        read_singly(first, count);
}

/* A sector from the host. False, having dealt with it, if it won't come. */
static bool receive_sector(uint8_t *to) {
    for (uint32_t at = 0; at < SECTOR; ) {
        rx_arm(to+at);
        int got = rx_wait();
        if (got < 0)
            return false;
        at += (uint32_t)got;
        moved += (uint32_t)got;
        if ((uint32_t)got < PACKET) {
            fail(5, 0x24);                      /* a short packet inside a sector */
            stall_out();
            return false;
        }
    }
    return true;
}

static void write_failed(void) {
    fail(3, 0x0c);
    /* With all the data in, the host has nothing to send, so it wouldn't find a stall and clear it. */
    if (moved < expected)
        stall_out();
}

static void write_singly(uint32_t first, uint32_t count) {
    for (uint32_t s = 0; s < count; ++s) {
        if (!receive_sector(sector))
            return;
        if (!card_write(first+s, sector)) {
            write_failed();
            return;
        }
        ++stats.sectors_written;
        app_feed();
    }
}

/* While one sector goes to the card, DMA receives the next. The host only hears of success after the card's status. */
static void write_stream(uint32_t first, uint32_t count) {
    uint8_t *in = sector, *out = sector2;
    bool pending = false;
    uint32_t s = 0;
    for (; s < count; ++s) {
        bool got = receive_sector(in);
        if (pending) {
            pending = false;
            if (!card_write_stream_finish()) {
                /* A failed stop leaves the stream's earlier sectors unconfirmed, and they're gone from here, so the host
                 * has to hear about it and write them again. Otherwise this one can be retried, as it's still here. */
                if (!card_write_stream_stop()) {
                    write_failed();
                    return;
                }
                if (!card_write(first+s-1, out)) {
                    write_failed();
                    return;
                }
                ++stats.sectors_written;
                if (got && !card_write(first+s, in)) {
                    write_failed();
                    return;
                }
                if (got) {
                    ++stats.sectors_written;
                    write_singly(first+s+1, count-s-1);
                }
                return;
            }
            ++stats.sectors_written;
        }
        if (!got) {
            if (!card_write_stream_stop() && !sense_key)
                write_failed();
            return;
        }
        card_write_stream_begin(in);
        pending = true;
        uint8_t *swap = in;
        in = out;
        out = swap;
        app_feed();
    }
    bool last = card_write_stream_finish();
    if (last)
        ++stats.sectors_written;
    if (!card_write_stream_stop() || !last)
        write_failed();
}

static void write_sectors(uint32_t first, uint32_t count) {
    if (count > 1 && total_sectors-(first+count) >= NEAR_THE_END && card_write_stream_start(first))
        write_stream(first, count);
    else
        write_singly(first, count);
}

static void no_data_expected(void) {
    if (expected) {
        fail(5, 0x20);
        if (to_host)
            stall_in();
        else
            stall_out();
    }
}

static void command_run(const uint8_t *c) {
    uint8_t asked_key = sense_key, asked_code = sense_code;
    if (c[0] != 0x03)
        sense_key = sense_code = 0;
    switch (c[0]) {
    case 0x00:                                  /* test unit ready */
        if (ejected)
            fail(2, 0x3a);
        no_data_expected();
        break;
    case 0x03: {                                /* request sense */
        for (unsigned i = 0; i < 18; ++i)
            reply[i] = 0;
        reply[0] = 0x70;
        reply[2] = asked_key;
        reply[7] = 10;
        reply[12] = asked_code;
        sense_key = sense_code = 0;
        send_reply(reply, 18);
        break;
    }
    case 0x12:                                  /* inquiry */
        if (c[1]&1) {
            fail(5, 0x24);
            moved = 0;
            finish_in(0);
            break;
        }
        send_reply(inquiry, sizeof(inquiry));
        break;
    case 0x1a:                                  /* mode sense (6) */
        reply[0] = 3;
        reply[1] = reply[2] = reply[3] = 0;
        send_reply(reply, 4);
        break;
    case 0x5a:                                  /* mode sense (10) */
        for (unsigned i = 0; i < 8; ++i)
            reply[i] = 0;
        reply[1] = 6;
        send_reply(reply, 8);
        break;
    case 0x1b:                                  /* start stop unit */
        if ((c[4]&3) == 2)
            ejected = true;
        no_data_expected();
        break;
    case 0x1e:                                  /* prevent or allow removal: we don't hold a disk in */
    case 0x2f:                                  /* verify */
    case 0x35:                                  /* synchronize cache: there's no cache here */
        no_data_expected();
        break;
    case 0x23:                                  /* read format capacities */
        for (unsigned i = 0; i < 12; ++i)
            reply[i] = 0;
        reply[3] = 8;
        put_be32(reply+4, total_sectors);
        reply[8] = 2;
        reply[10] = SECTOR >> 8;
        send_reply(reply, 12);
        break;
    case 0x25:                                  /* read capacity (10) */
        if (ejected) {
            fail(2, 0x3a);
            moved = 0;
            finish_in(0);
            break;
        }
        put_be32(reply, total_sectors-1);
        put_be32(reply+4, SECTOR);
        send_reply(reply, 8);
        break;
    case 0x28:                                  /* read (10) */
    case 0x2a: {                                /* write (10) */
        uint32_t first = be32(c+2), count = (uint32_t)c[7] << 8 | c[8];
        bool write = c[0] == 0x2a;
        if (ejected)
            fail(2, 0x3a);
        else if (first > total_sectors || count > total_sectors-first)
            fail(5, 0x21);
        else if (expected != count*SECTOR || to_host == write)
            fail(5, 0x24);
        if (sense_key) {
            if (to_host) {
                moved = 0;
                finish_in(0);
            } else if (expected)
                stall_out();
            break;
        }
        if (write)
            write_sectors(first, count);
        else
            read_sectors(first, count);
        break;
    }
    default:
        fail(5, 0x20);
        if (to_host) {
            moved = 0;
            finish_in(0);
        } else if (expected)
            stall_out();
        break;
    }
}

/* Waits out a stall the host has to clear. */
static bool stall_cleared(void) {
    for (uint32_t n = 0; (stalled_in || stalled_out) && !aborted && configuration; ++n) {
        bus_poll();
        if ((n&0xff) == 0xff)
            app_feed();
    }
    return !aborted && configuration;
}

static void command_handle(void) {
    uint8_t c[16];
    tag = le32(command_block+4);
    expected = le32(command_block+8);
    to_host = (command_block[12]&0x80) != 0;
    moved = 0;
    for (unsigned i = 0; i < sizeof(c); ++i)
        c[i] = command_block[15+i];
    /* The next packet after this one is either the data, or this endpoint's next command. */
    if (to_host || !expected)
        rx_arm(command_block);
    aborted = false;
    command_run(c);
    if (aborted || !stall_cleared())
        return;
    if (!to_host && expected)
        rx_arm(command_block);
    put_le32(status_block, 0x53425355);
    put_le32(status_block+4, tag);
    put_le32(status_block+8, expected-moved);
    status_block[12] = sense_key ? 1:0;
    tx_send(status_block, sizeof(status_block));
    app_feed();
}

void msc_poll(void) {
    bus_poll();
    if (!configuration || needs_reset)
        return;
    select_endpoint(1);
    if (!(sie_read(RX_CSR1)&1))
        return;
    uint32_t size = sie_read(RX_COUNT1) | sie_read(RX_COUNT2) << 8;
    __asm__ volatile("ssync" ::: "memory");
    if (size != 31 || le32(command_block) != 0x43425355 || (command_block[13]&15) || !command_block[14]
        || command_block[14] > 16) {
        /* A bad command block: bulk-only says stall both until the host resets the device. */
        needs_reset = true;
        ++stats.errors;
        rx_arm(command_block);
        stall_in();
        stall_out();
        return;
    }
    command_handle();
}

bool msc_configured(void) {
    return configuration;
}

bool msc_ejected(void) {
    return ejected;
}

const struct msc_stats *msc_stats(void) {
    return &stats;
}

void msc_start(uint32_t sectors) {
    total_sectors = sectors;
    REG(CON0) = 0;
    REG(CON1) = 0;
    REG(IO_CON0) = 0x0c;
    hal_delay_us(25000);
    REG(CLOCK_CON1) &= ~3u;                     /* USB clock: PLL 48 MHz */
    REG(EP0_ADDRESS) = DMA_ADDRESS(ep0);
    for (unsigned n = 1; n <= 3; ++n)
        REG(EP_RX_ADDRESS(n)) = DMA_ADDRESS(ep0);
    REG(EP4_RX_ADDRESS) = DMA_ADDRESS(ep0);
    REG(IO_CON0) |= BIT(10) | BIT(9);
    REG(IO_CON0) &= ~BIT(11);
    REG(IO_CON0) |= BIT(12);
    REG(CON0) &= ~0x4035u;
    REG(CON1) = 0;
    REG(CON0) |= 1;
    REG(IO_CON0) &= ~0xf0u;
    REG(IO_CON0) |= 0x40;                       /* D+ pull-up: on the bus */
    REG(CON0) |= 0x3c;
    sie_write(POWER, 0x60);
    sie_write(INT_USB_ENABLE, 0x07);
    sie_write(INT_TX1_ENABLE, 0x01);
    sie_write(INT_TX2_ENABLE, 0);
    sie_write(INT_RX1_ENABLE, 0);
    sie_write(INT_RX2_ENABLE, 0);
    REG(CON0) &= ~(0x1fu << 19);
    configuration = 0;
}

void msc_stop(void) {
    REG(CON0) = 0;
    REG(IO_CON0) = 0x0c;
}
