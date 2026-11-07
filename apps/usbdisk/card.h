/* SPDX-License-Identifier: MIT */
#ifndef CARD_H
#define CARD_H
#include <stdbool.h>
#include <stdint.h>
/* The FM-1's SD card on the panel's SPI1, in SPI mode with CRCs on both ways. Between card_open() and card_close() the
 * screen can't be drawn. */
void card_open(unsigned lsb_mhz);
void card_close(void);
/* Starts the card, and returns its size in 512-byte sectors, or 0 if there's no usable card. */
uint32_t card_start(unsigned lsb_mhz);
void card_fast(unsigned lsb_mhz);
/* Leaves the card as a bootloader expects to find it. */
void card_finish(void);
bool card_read(uint32_t sector, uint8_t *data);
bool card_write(uint32_t sector, const uint8_t *data);
/* Many sectors in a row, which is much faster than one at a time. The card stays selected until the stop. */
bool card_stream_start(uint32_t sector);
bool card_stream_read(uint8_t *data);
void card_stream_stop(void);
#endif
