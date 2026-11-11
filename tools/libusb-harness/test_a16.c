/* SPDX-License-Identifier: MIT */
#define _POSIX_C_SOURCE 200809L
#include "../../tests/fake_bus.h"
#include "../../common/devices/ploytec/ploytec_codec.h"
#include "wav.h"
#include <assert.h>
#include <stdlib.h>
#include <unistd.h>

static void stereo(struct fake_xfer *x, void *arg)
{
    (void)arg;
    memset(x->buf, 0, x->size);
    if (x->pipe != 0x86) return;
    /* Every frame: ch1 = +0x123456, ch2 = -0x123456 (24-bit two's complement). */
    for (uint32_t f = 0; f < x->size / 64; f++)
        for (unsigned int bit = 0; bit < 24; bit++) {
            x->buf[f * 64 + bit] = (0x123456u >> (23 - bit)) & 1;
            x->buf[f * 64 + 32 + bit] = (0xedcbaau >> (23 - bit)) & 1;
        }
}

static void duplex(bool interrupt, unsigned int rate)
{
    const struct ozzy_device *model = ozzy_device_lookup(0x0a4a, 0x5a16);
    assert(model && model->in_channels == 2 && model->out_channels == 16);
    struct ozzy_shared_memory *shm = NULL;
    assert(!posix_memalign((void **)&shm, _Alignof(struct ozzy_shared_memory), sizeof(*shm)));
    memset(shm, 0, sizeof(*shm));
    struct ozzy_engine *e = ozzy_engine_init(malloc(ozzy_engine_size(model)), model);
    assert(shm && e);
    struct fake_bus fb;
    fake_bus_init(&fb, shm, true, NULL);
    ozzy_engine_attach(e, &fb.bus, shm);
    ozzy_engine_set_endpoint(e, OZZY_PIPE_PCM_OUT, 0x05, interrupt ? 3 : 2, interrupt ? 964 : 512, 1);
    ozzy_engine_set_endpoint(e, OZZY_PIPE_PCM_IN, 0x86, interrupt ? 3 : 2, interrupt ? 1024 : 512, 1);
    uint8_t input[1024 * 6] = {0}, output[1024 * 48] = {0};
    struct ozzy_pcm_ring ri = { input, 1024, 2, 0 }, ro = { output, 1024, 16, 0 };
    ozzy_engine_set_pcm_ring(e, OZZY_PCM_IN, &ri);
    ozzy_engine_set_pcm_ring(e, OZZY_PCM_OUT, &ro);
    if (rate) ozzy_engine_set_rate(e, rate);
    assert(ozzy_engine_start(e));
    assert(shm->audio.sample_rate == (rate ? rate : 48000));
    struct fake_xfer x;
    for (unsigned int i = 0; i < 12; i++) {
        assert(fake_bus_pop(&fb, &x));
        fake_bus_complete(&fb, e, &x, stereo, NULL);
    }
    assert(fb.progress[OZZY_PCM_IN] > 0 && fb.progress[OZZY_PCM_OUT] > 0);
    static const uint8_t expected[6] = { 0x56,0x34,0x12,0xaa,0xcb,0xed };
    assert(!memcmp(input, expected, sizeof(expected)));
    ozzy_engine_stop(e);
    assert(!fake_bus_pop(&fb, &x));
    free(e); free(shm);
}

int main(void)
{
    duplex(false, 0); /* No host request: A16 must default to 48 kHz. */
    duplex(true, 0);
    const unsigned int rates[] = {44100, 48000, 88200, 96000};
    for (unsigned int i = 0; i < 4; i++) {
        duplex(false, rates[i]);
        duplex(true, rates[i]); /* Actual A16 interrupt endpoint sizes. */
    }
    static const uint8_t expected[6] = { 0x56,0x34,0x12,0xaa,0xcb,0xed };

    /* Independent single-bit vectors across all 16 channels and all 24 bits. */
    ploytec_frame_fn encode = ploytec_frame_encoder(16);
    assert(encode);
    for (unsigned int ch = 0; ch < 16; ch++)
        for (unsigned int bit = 0; bit < 24; bit++) {
            uint8_t src[48] = {0}, wire[48] = {0}, expected_wire[48] = {0};
            src[ch * 3 + bit / 8] = (uint8_t)(1u << (bit % 8));
            expected_wire[(ch % 2) * 24 + 23 - bit] = (uint8_t)(1u << (ch / 2));
            encode(wire, src);
            assert(!memcmp(wire, expected_wire, 48));
        }
    char path[] = "/tmp/ozzy-wav-XXXXXX";
    int fd = mkstemp(path);
    assert(fd >= 0);
    FILE *f = fdopen(fd, "wb");
    assert(f && wav_write(f, 44100, 2, expected, 1));
    assert(!fclose(f));
    uint8_t dst[6]; uint32_t got = 0;
    assert(wav_read(path, 44100, 2, dst, 1, &got) && got == 1 && !memcmp(dst, expected, 6));
    assert(!wav_read(path, 48000, 2, dst, 1, &got));
    assert(!wav_read(path, 44100, 16, dst, 1, &got));
    assert(!wav_read(path, 44100, 2, dst, 0, &got));
    f = fopen(path, "wb"); assert(f); fputs("RIFF", f); fclose(f);
    assert(!wav_read(path, 44100, 2, dst, 1, &got));
    unlink(path);
    puts("A16 fake-bus duplex, 384 encoder vectors, WAV validation: ok");
    return 0;
}
