/* SPDX-License-Identifier: MIT */
#include "live_audio.h"
#include "profile_override.h"
#include "profiles.h"
#include <assert.h>
#include <limits.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
static struct live_fifo fifo;
static void *producer(void *unused) {
    (void)unused;
    for (unsigned int i = 0; i < 200000; i++) {
        float frame[2] = {(float)i, -(float)i};
        while (!live_push(&fifo, frame, 1)) {
        }
    }
    return NULL;
}
int main(void) {
    assert(a16_profile_find("7.1.4") == 0 && a16_profile_find("9.1.6") == 1);
    assert(a16_profile_find("invalid-layout") == -1);
    assert(A16_PROFILE_COUNT > 40);
    assert(a16_profile_find("7.1.4t") == 0 && a16_profile_find("9.1.6t") == 1);
    assert(a16_profile_find("7.1.4h") >= 0 && a16_profile_find("9.1") >= 0);
    assert(a16_profiles[a16_profile_find("9.1")].advanced);
    assert(a16_profiles[0].channels == 12 && !a16_profiles[0].advanced);
    assert(a16_profiles[1].advanced);
    struct a16_missing missing = {0};
    assert(a16_missing_parse(&missing, "Chr"));
    assert(!a16_missing_parse(&missing, "NotASpeaker"));
    struct a16_missing parsed = {0};
    const char *json = "{\"missing_speakers\":[\"Chr\"]}";
    assert(a16_missing_json(&parsed, json, strlen(json)) && parsed.count == 1);
    assert(!a16_missing_json(&parsed, "{\"missing_speakers\":12}", 23));
    struct a16_effective effective;
    for (unsigned int bank = 1; bank <= 4; bank++) {
        char id[16];
        snprintf(id, sizeof(id), "custom-%u", bank);
        const struct a16_profile *base = &a16_profiles[a16_profile_find(id)];
        assert(a16_apply_missing(&effective, base, &missing));
        assert(effective.profile.channels == (bank == 1 ? 15 : 16));
        for (unsigned int c = 0; c < effective.profile.channels; c++) {
            unsigned int original = 0;
            while (original < base->channels && base->usb[original] != effective.profile.usb[c])
                original++;
            assert(original < base->channels && base->position[original] == effective.profile.position[c]);
        }
        float input[16], output[16];
        for (unsigned int i = 0; i < 16; i++)
            input[i] = 1;
        a16_profile_expand(&effective.profile, output, input);
        if (bank == 1)
            assert(output[13] == 0 && output[14] == 1 && output[15] == 1);

        assert(base->channels == 16 && a16_word_in(base->speakers, "Chr") == (bank == 1));
    }
    struct a16_missing all = {0};
    assert(a16_missing_parse(&all, "L,R"));
    assert(a16_apply_missing(&effective, &a16_profiles[a16_profile_find("2.0")], &all));
    assert(!effective.profile.available && effective.profile.channels == 0);
    unsigned int custom_count = 0;
    for (unsigned int mode = 0; mode < A16_PROFILE_COUNT; mode++) {
        const struct a16_profile *p = &a16_profiles[mode];
        assert(p->channels <= 16);
        assert(p->available == (p->channels > 0));
        assert(p->available || p->custom);
        assert(!p->custom || p->pulse_compatible);
        if (p->custom) {
            custom_count++;
            assert(!strncmp(p->name, "custom-", 7));
            assert(p->advanced);
        }
        if (p->pulse_compatible) {
            const uint32_t allowed[] = {SPA_AUDIO_CHANNEL_FL,  SPA_AUDIO_CHANNEL_FR,  SPA_AUDIO_CHANNEL_FC,
                                        SPA_AUDIO_CHANNEL_LFE, SPA_AUDIO_CHANNEL_SL,  SPA_AUDIO_CHANNEL_SR,
                                        SPA_AUDIO_CHANNEL_RL,  SPA_AUDIO_CHANNEL_RR,  SPA_AUDIO_CHANNEL_RC,
                                        SPA_AUDIO_CHANNEL_FLC, SPA_AUDIO_CHANNEL_FRC, SPA_AUDIO_CHANNEL_TC,
                                        SPA_AUDIO_CHANNEL_TFL, SPA_AUDIO_CHANNEL_TFC, SPA_AUDIO_CHANNEL_TFR,
                                        SPA_AUDIO_CHANNEL_TRL, SPA_AUDIO_CHANNEL_TRC, SPA_AUDIO_CHANNEL_TRR};
            for (unsigned int c = 0; c < p->channels; c++) {
                bool found = false;
                for (unsigned int a = 0; a < sizeof(allowed) / sizeof(allowed[0]); a++)
                    if (allowed[a] == p->position[c])
                        found = true;
                assert(found);
            }
        }
        assert(a16_profile_find(p->name) == (int)mode);
        for (unsigned int i = 0; i < p->channels; i++) {
            assert(p->usb[i] < 16);
            for (unsigned int j = i + 1; j < p->channels; j++) {
                assert(p->usb[i] != p->usb[j]);
                assert(p->position[i] != p->position[j]);
            }
        }
        for (unsigned int j = mode + 1; j < A16_PROFILE_COUNT; j++)
            assert(strcmp(p->name, a16_profiles[j].name));
        for (unsigned int channel = 0; channel < p->channels; channel++) {
            float in[16] = {0}, out[16];
            in[channel] = 0.5f;
            a16_profile_expand(p, out, in);
            for (unsigned int usb = 0; usb < 16; usb++)
                assert(out[usb] == (usb == p->usb[channel] ? 0.5f : 0.0f));
        }
    }

    assert(custom_count == 4);
    for (unsigned int bank = 1; bank <= 4; bank++) {
        char name[16];
        snprintf(name, sizeof(name), "custom-%u", bank);
        int index = a16_profile_find(name);
        assert(index >= 0);
        const struct a16_profile *p = &a16_profiles[index];
        assert(p->available && p->custom && p->pulse_compatible && p->channels == 16);
        bool overhead = false, inner_left = false, inner_right = false;
        for (unsigned int c = 0; c < p->channels; c++) {
            if (p->position[c] == SPA_AUDIO_CHANNEL_TC)
                overhead = true;
            if (p->position[c] == SPA_AUDIO_CHANNEL_FLC)
                inner_left = true;
            if (p->position[c] == SPA_AUDIO_CHANNEL_FRC)
                inner_right = true;
        }
        assert(overhead == (bank != 4));
        assert((inner_left && inner_right) == (bank != 1));
    }
    fifo.channels = 2;
    atomic_store(&fifo.read, UINT_MAX - 10000);
    atomic_store(&fifo.write, UINT_MAX - 10000);
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, producer, NULL));
    for (unsigned int i = 0; i < 200000; i++) {
        float frame[2];
        while (!live_peek(&fifo, frame, 1)) {
        }
        assert(frame[0] == (float)i && frame[1] == -(float)i);
        live_consume(&fifo, 1);
    }
    assert(!pthread_join(thread, NULL));
    assert(!live_available(&fifo));
    float *block = calloc(LIVE_CAPACITY * 2, sizeof(float));
    assert(block);
    assert(live_push(&fifo, block, LIVE_CAPACITY) == LIVE_CAPACITY);
    assert(!live_push(&fifo, block, 1));
    live_discard(&fifo);
    assert(!live_available(&fifo));
    free(block);
    const float values[] = {-1, 0, 0.5f, 0.99999988f, NAN, INFINITY, -INFINITY, 2, -2};
    for (unsigned int i = 0; i < sizeof(values) / sizeof(values[0]); i++) {
        uint8_t bytes[3];
        live_pack(bytes, values[i]);
        float v = live_unpack(bytes);
        assert(isfinite(v) && v >= -1 && v < 1);
        if (isfinite(values[i]) && fabsf(values[i]) <= 1)
            assert(fabsf(v - values[i]) < 0.00000013f);
    }
    /* Simulate 10 minutes at both drift extremes with a 1 ms control step. */
    for (int sign = -1; sign <= 1; sign += 2) {
        struct live_clock c = {0};
        double level = LIVE_TARGET, ratio = 1;
        for (unsigned int i = 0; i < 600000; i++) {
            ratio = live_ratio(&c, (uint32_t)level, 0.001);
            level += 48 * (1 + sign * 0.0003) - 48 / ratio;
            assert(level > 0 && level < LIVE_CAPACITY && ratio >= 0.995 && ratio <= 1.005);
        }
        assert(fabs(level - LIVE_TARGET) < 5);
    }
    puts("SPSC concurrent ordering/wrap/bounds, PCM clipping, drift controller: ok");
}
