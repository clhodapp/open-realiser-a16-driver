/* SPDX-License-Identifier: MIT */
#ifndef OZZY_HARNESS_WAV_H
#define OZZY_HARNESS_WAV_H
#include <stdint.h>
#include <stdio.h>
#include <stdbool.h>
/* Only PCM S24_3LE, with an explicit rate and channel match. No conversion. */
bool wav_read(const char *path, uint32_t rate, uint16_t channels,
              uint8_t *dst, uint32_t capacity_frames, uint32_t *frames);
/* The caller opens with "wbx" to avoid overwriting an existing recording. */
bool wav_write(FILE *f, uint32_t rate, uint16_t channels, const uint8_t *src, uint32_t frames);
#endif
