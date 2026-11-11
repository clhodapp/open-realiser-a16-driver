/* SPDX-License-Identifier: MIT */
#include "wav.h"
#include <string.h>
#include <limits.h>

static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | (uint16_t)p[1] << 8); }
static uint32_t le32(const uint8_t *p) { return le16(p) | (uint32_t)le16(p + 2) << 16; }
static void put16(uint8_t *p, uint16_t v) { p[0] = v; p[1] = v >> 8; }
static void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }

bool wav_read(const char *path, uint32_t rate, uint16_t channels,
              uint8_t *dst, uint32_t capacity_frames, uint32_t *frames)
{
    FILE *f = fopen(path, "rb");
    if (!f) return false;
    uint8_t h[40];
    bool ok = false, format = false;
    if (fread(h, 1, 12, f) != 12 || memcmp(h, "RIFF", 4) || memcmp(h + 8, "WAVE", 4)) goto done;
    uint64_t left = le32(h + 4);
    if (left < 4) goto done;
    left -= 4;
    while (left >= 8) {
        if (fread(h, 1, 8, f) != 8) goto done;
        uint32_t n = le32(h + 4);
        left -= 8;
        uint64_t padded = (uint64_t)n + (n & 1);
        if (padded > left) goto done;
        if (!memcmp(h, "fmt ", 4)) {
            if (format || n < 16 || n > sizeof(h) || fread(h, 1, n, f) != n) goto done;
            bool pcm = le16(h) == 1;
            /* WAVE_FORMAT_EXTENSIBLE with PCM subtype, 24 valid bits. */
            if (le16(h) == 0xfffe && n == 40) {
                static const uint8_t pcm_guid[16] = { 1,0,0,0,0,0,0x10,0,0x80,0,0,0xaa,0,0x38,0x9b,0x71 };
                pcm = le16(h + 16) == 22 && le16(h + 18) == 24 && !memcmp(h + 24, pcm_guid, 16);
            }
            if (!pcm || !channels || le16(h + 2) != channels || le32(h + 4) != rate ||
                le16(h + 14) != 24 || le16(h + 12) != channels * 3 ||
                le32(h + 8) != rate * channels * 3) goto done;
            format = true;
        } else if (!memcmp(h, "data", 4)) {
            uint32_t width = channels * 3;
            if (!format || !width || n % width || n / width > capacity_frames ||
                fread(dst, 1, n, f) != n) goto done;
            *frames = n / width;
            ok = true;
            break;
        } else {
#if LONG_MAX < UINT32_MAX
            if (n > LONG_MAX) goto done;
#endif
            if (fseek(f, (long)n, SEEK_CUR)) goto done;
        }
        if ((n & 1) && fgetc(f) == EOF) goto done;
        left -= padded;
    }
done:
    fclose(f);
    return ok;
}

bool wav_write(FILE *f, uint32_t rate, uint16_t channels, const uint8_t *src, uint32_t frames)
{
    uint64_t size = (uint64_t)frames * channels * 3;
    if (!channels || size > UINT32_MAX - 36) return false;
    uint8_t h[44] = {0};
    memcpy(h, "RIFF", 4); put32(h + 4, (uint32_t)size + 36);
    memcpy(h + 8, "WAVEfmt ", 8); put32(h + 16, 16); put16(h + 20, 1);
    put16(h + 22, channels); put32(h + 24, rate); put32(h + 28, rate * channels * 3);
    put16(h + 32, channels * 3); put16(h + 34, 24);
    memcpy(h + 36, "data", 4); put32(h + 40, (uint32_t)size);
    return fwrite(h, 1, sizeof(h), f) == sizeof(h) && fwrite(src, 1, (size_t)size, f) == size && !fflush(f);
}
