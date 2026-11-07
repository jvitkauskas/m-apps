/* SPDX-License-Identifier: MPL-2.0 */
/* Audio out: I2S to the external codec, double-buffered DMA, polled. Adapted from felucca-bsp's boards/fm1/fm1_audio.h. */
#ifndef AUDIO_H
#define AUDIO_H
#include <stdbool.h>
#include <stdint.h>
/* Stereo frames, L then R, signed 24-bit in the low bits of an int32_t. A half buffer is AUDIO_HALF_FRAMES frames. */
#define AUDIO_HALF_FRAMES 1152u
/* 24 MHz / 544, close to 44100, but only with the stock audio PLL. */
#define AUDIO_RATE_NUM 24000000u
#define AUDIO_RATE_DEN 544u
/* Whether the audio PLL is as firmware and m-bootloader leave it. */
bool audio_clock_is_stock(void);
void audio_start(void);
/* A half buffer to fill, if one has come free, or 0. Call audio_filled() when it's full. */
int32_t *audio_free_half(void);
/* Which half is which, 0 or 1. */
int32_t *audio_half(unsigned which);
void audio_filled(void);
void audio_stop(void);
#endif
