/* SPDX-License-Identifier: MIT */
#define _POSIX_C_SOURCE 200809L
#include "live_audio.h"
#include "profile_override.h"
#include "profiles.h"
#include "usb_bus.h"
#include <limits.h>
#include <pipewire/pipewire.h>
#include <samplerate.h>
#include <signal.h>
#include <spa/monitor/utils.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/profile.h>
#include <spa/pod/filter.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define RATE 48000u
#define BLOCK 80u
struct live;
struct endpoint {
    struct live *owner;
    struct pw_stream *stream;
    struct spa_hook listener;
    struct live_fifo fifo;
    _Atomic bool active, reset;
    _Atomic uint64_t frames, underruns, overruns;
    bool source;
    uint32_t channels;
};
struct live {
    struct pw_thread_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct spa_hook core_listener;
    struct endpoint sink, source;
    struct spa_device device;
    struct spa_hook_list device_hooks;
    struct pw_proxy *device_proxy;
    struct spa_hook proxy_listener;
    struct spa_param_info device_params[2];
    uint32_t device_id;
    int profile;
    struct a16_effective effective[A16_PROFILE_COUNT];
    struct a16_missing missing;
    bool profile_save;
    struct usb_bus bus;
    struct ozzy_engine *engine;
    struct ozzy_shared_memory *shm;
    SRC_STATE *out_src, *in_src;
    struct live_clock out_clock, in_clock;
    uint8_t output[BLOCK * 48], input[BLOCK * 6];
    bool output_primed, capture_primed;
    _Atomic bool failed;
    _Atomic double output_ratio, capture_ratio;
};
static volatile sig_atomic_t stopping;
static void stop_signal(int sig) {
    (void)sig;
    stopping = 1;
}
static void core_error(void *data, uint32_t id, int seq, int res, const char *message) {
    (void)seq;
    struct live *l = data;
    fprintf(stderr, "PipeWire error id=%u: %s (%d)\n", id, message, res);
    if (id == PW_ID_CORE || res == -EPIPE)
        atomic_store(&l->failed, true);
}
static const struct pw_core_events core_events = {PW_VERSION_CORE_EVENTS, .error = core_error};
static void state_changed(void *data, enum pw_stream_state old, enum pw_stream_state state,
                          const char *error) {
    (void)old;
    struct endpoint *e = data;
    if (state == PW_STREAM_STATE_STREAMING && e->source)
        atomic_store(&e->reset, true);
    atomic_store(&e->active, state == PW_STREAM_STATE_STREAMING);
    fprintf(stderr, "%s: %s%s%s\n", e->source ? "A16 return" : "A16 playback",
            pw_stream_state_as_string(state), error ? ": " : "", error ? error : "");
    if (state == PW_STREAM_STATE_ERROR)
        atomic_store(&e->owner->failed, true);
}
/* Runs on PipeWire's RT data thread: bounded copies and atomics only. */
static void process(void *data) {
    struct endpoint *e = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(e->stream);
    if (!b)
        return;
    struct spa_buffer *buf = b->buffer;
    if (buf->n_datas != 1 || !buf->datas[0].data || !buf->datas[0].chunk) {
        atomic_store(&e->owner->failed, true);
        pw_stream_queue_buffer(e->stream, b);
        return;
    }
    struct spa_data *d = &buf->datas[0];
    uint32_t stride = e->channels * sizeof(float), n;
    if (e->source) {
        if (atomic_exchange(&e->reset, false)) {
            live_discard(&e->fifo);
            e->owner->capture_primed = false;
        }
        n = d->maxsize / stride;
        if (b->requested && b->requested < n)
            n = b->requested;
        uint32_t available = live_available(&e->fifo), got = 0;
        if (!e->owner->capture_primed && available >= LIVE_TARGET)
            e->owner->capture_primed = true;
        if (e->owner->capture_primed) {
            got = live_peek(&e->fifo, d->data, n);
            live_consume(&e->fifo, got);
            if (got < n) {
                atomic_fetch_add(&e->underruns, 1);
                e->owner->capture_primed = false;
            }
        }
        memset((float *)d->data + got * 2, 0, (n - got) * stride);
        d->chunk->offset = 0;
        d->chunk->stride = stride;
        d->chunk->size = n * stride;
        b->size = n;
    } else {
        uint32_t offset = d->chunk->offset;
        if (offset > d->maxsize || d->chunk->size > d->maxsize - offset ||
            (d->chunk->stride != 0 && d->chunk->stride != (int32_t)stride) || d->chunk->size % stride) {
            atomic_store(&e->owner->failed, true);
            pw_stream_queue_buffer(e->stream, b);
            return;
        }
        n = d->chunk->size / stride;
        const struct a16_profile *profile = &e->owner->effective[e->owner->profile].profile;
        const float *input = (float *)((uint8_t *)d->data + offset);
        uint32_t got = 0;
        float expanded[128 * 16];
        while (got < n) {
            uint32_t block = SPA_MIN(n - got, 128u);
            for (uint32_t i = 0; i < block; i++)
                a16_profile_expand(profile, expanded + i * 16, input + (got + i) * e->channels);
            uint32_t written = live_push(&e->fifo, expanded, block);
            got += written;
            if (written < block)
                break;
        }
        if (got < n)
            atomic_fetch_add(&e->overruns, 1);
    }
    atomic_fetch_add(&e->frames, n);
    pw_stream_queue_buffer(e->stream, b);
}
static const struct pw_stream_events stream_events = {PW_VERSION_STREAM_EVENTS,
                                                      .state_changed = state_changed, .process = process};
static bool make_stream(struct live *l, struct endpoint *e, bool source) {
    e->owner = l;
    e->source = source;
    e->fifo.channels = source ? 2 : 16;
    const struct a16_profile *profile = &l->effective[l->profile].profile;
    e->channels = source ? 2 : profile->channels;
    char device_id[24], description[96], channel_count[12];
    snprintf(device_id, sizeof(device_id), "%u", l->device_id);
    snprintf(description, sizeof(description), "Realiser A16 USB %s", profile->name);
    snprintf(channel_count, sizeof(channel_count), "%u", e->channels);
    const char *positions = source ? "[ FL FR ]" : profile->positions;
    struct pw_properties *props = pw_properties_new(
        PW_KEY_NODE_NAME, source ? "ozzy.a16.capture" : "ozzy.a16.playback", PW_KEY_NODE_DESCRIPTION,
        source ? "Realiser A16 USB Return" : description, PW_KEY_DEVICE_ID, device_id, "card.profile.device",
        source ? "1" : "0", PW_KEY_MEDIA_CLASS, source ? "Audio/Source" : "Audio/Sink", PW_KEY_MEDIA_TYPE,
        "Audio", PW_KEY_MEDIA_CATEGORY, source ? "Capture" : "Playback", PW_KEY_NODE_VIRTUAL, "true",
        PW_KEY_NODE_PAUSE_ON_IDLE, "true", PW_KEY_NODE_RATE, "1/48000", PW_KEY_NODE_LATENCY, "1024/48000",
        "priority.session", "0", "node.autoconnect", "false", "audio.position", positions, "audio.rate",
        "48000", "audio.channels", channel_count, "stream.dont-remix", "true", "node.group", "ozzy.a16",
        "resample.quality", "4", NULL);
    if (!source) {
        pw_properties_set(props, "a16.profile", profile->name);
        pw_properties_set(props, "a16.room.required", profile->room);
        pw_properties_set(props, "a16.speakers", profile->speakers);
        pw_properties_set(props, "a16.missing-speakers", l->missing.text);
        pw_properties_set(props, "a16.profile.category",
                          profile->custom ? "custom" : (profile->advanced ? "advanced" : "compatibility"));
        pw_properties_set(props, "a16.profile.pulse-compatible",
                          profile->pulse_compatible ? "true" : "false");
        pw_properties_set(props, "a16.profile.advanced", profile->advanced ? "true" : "false");
    }
    e->stream = pw_stream_new(l->core, source ? "A16 return" : "A16 playback", props);
    if (!e->stream)
        return false;
    pw_stream_add_listener(e->stream, &e->listener, &stream_events, e);
    struct spa_audio_info_raw format = {
        .format = SPA_AUDIO_FORMAT_F32_LE, .rate = RATE, .channels = e->channels};
    for (uint32_t c = 0; c < format.channels; c++)
        format.position[c] =
            source ? (c ? SPA_AUDIO_CHANNEL_FR : SPA_AUDIO_CHANNEL_FL) : profile->position[c];
    uint8_t buffer[1024];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
    const struct spa_pod *param = spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &format);
    return pw_stream_connect(e->stream, source ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT, PW_ID_ANY,
                             PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS, &param, 1) >= 0;
}
#include "profile_device.inc"

static bool open_pipewire(struct live *l) {
    pw_init(NULL, NULL);
    l->loop = pw_thread_loop_new("a16-pipewire", NULL);
    if (!l->loop)
        return false;
    l->context = pw_context_new(pw_thread_loop_get_loop(l->loop), NULL, 0);
    if (!l->context)
        return false;
    l->core = pw_context_connect(l->context, NULL, 0);
    if (!l->core)
        return false;
    pw_core_add_listener(l->core, &l->core_listener, &core_events, l);
    if (!export_device(l))
        return false;
    return pw_thread_loop_start(l->loop) >= 0;
}
static void close_pipewire(struct live *l) {
    if (l->loop)
        pw_thread_loop_stop(l->loop);
    if (l->sink.stream)
        pw_stream_destroy(l->sink.stream);
    if (l->source.stream)
        pw_stream_destroy(l->source.stream);
    if (l->device_proxy)
        pw_proxy_destroy(l->device_proxy);
    if (l->core)
        pw_core_disconnect(l->core);
    if (l->context)
        pw_context_destroy(l->context);
    if (l->loop)
        pw_thread_loop_destroy(l->loop);
    pw_deinit();
}
/* Before OUT completion re-encodes the next transfer, prepare its 80-frame ring. */
static void before_complete(void *data, uint8_t endpoint) {
    struct live *l = data;
    if (endpoint != 0x05)
        return;
    memset(l->output, 0, sizeof(l->output));
    if (atomic_exchange(&l->sink.reset, false)) {
        live_discard(&l->sink.fifo);
        l->output_primed = false;
        memset(&l->out_clock, 0, sizeof(l->out_clock));
        src_reset(l->out_src);
    }
    if (!atomic_load(&l->sink.active)) {
        live_discard(&l->sink.fifo);
        l->output_primed = false;
        memset(&l->out_clock, 0, sizeof(l->out_clock));
        src_reset(l->out_src);
        return;
    }
    uint32_t available = live_available(&l->sink.fifo);
    if (!l->output_primed) {
        if (available < LIVE_TARGET)
            return;
        l->output_primed = true;
    }
    float input[256 * 16], output[BLOCK * 16] = {0};
    uint32_t n = live_peek(&l->sink.fifo, input, 256);
    double ratio = live_ratio(&l->out_clock, available, (double)BLOCK / RATE);
    atomic_store(&l->output_ratio, ratio);
    SRC_DATA block = {
        .data_in = input, .data_out = output, .input_frames = n, .output_frames = BLOCK, .src_ratio = ratio};
    int error = src_process(l->out_src, &block);
    if (error) {
        atomic_store(&l->failed, true);
        return;
    }
    live_consume(&l->sink.fifo, block.input_frames_used);
    if (block.output_frames_gen < BLOCK) {
        atomic_fetch_add(&l->sink.underruns, 1);
        l->output_primed = false;
        src_reset(l->out_src);
    }
    for (unsigned int i = 0; i < BLOCK * 16; i++)
        live_pack(l->output + i * 3, output[i]);
}
static void on_progress(void *data, enum ozzy_pcm_dir dir, uint64_t frames) {
    (void)frames;
    struct live *l = data;
    if (dir != OZZY_PCM_IN)
        return;
    /* Only the capture consumer owns its read index. While idle, do not produce. */
    if (!atomic_load(&l->source.active)) {
        src_reset(l->in_src);
        memset(&l->in_clock, 0, sizeof(l->in_clock));
        return;
    }
    float input[BLOCK * 2], output[256 * 2];
    for (unsigned int i = 0; i < BLOCK * 2; i++)
        input[i] = live_unpack(l->input + i * 3);
    double ratio = live_ratio(&l->in_clock, live_available(&l->source.fifo), (double)BLOCK / RATE);
    atomic_store(&l->capture_ratio, ratio);
    SRC_DATA block = {.data_in = input,
                      .data_out = output,
                      .input_frames = BLOCK,
                      .output_frames = 256,
                      .src_ratio = ratio};
    if (src_process(l->in_src, &block) || block.input_frames_used != BLOCK) {
        atomic_store(&l->failed, true);
        return;
    }
    uint32_t n = block.output_frames_gen;
    if (live_push(&l->source.fifo, output, n) < n)
        atomic_fetch_add(&l->source.overruns, 1);
}
int main(int argc, char **argv) {
    unsigned int seconds = 0;
    bool detach = false;
    int profile = 0;
    bool list_profiles = false;
    const char *config = NULL, *missing_arg = NULL;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--help")) {
            puts("a16-driver [--seconds 1..86400] [--detach] [--profile NAME] "
                 "[--list-profiles] [--config FILE] [--missing-speakers Chr,...]\n48 kHz, selectable layout "
                 "sink + stereo source. "
                 "Does not change default routing.\nRuns until SIGINT/SIGTERM unless --seconds is supplied.");
            return 0;
        }
        if (!strcmp(argv[i], "--list-profiles")) {
            list_profiles = true;
            continue;
        }
        if (!strcmp(argv[i], "--config") && i + 1 < argc) {
            config = argv[++i];
            continue;
        }
        if (!strcmp(argv[i], "--missing-speakers") && i + 1 < argc) {
            missing_arg = argv[++i];
            continue;
        }
        if (!strcmp(argv[i], "--detach"))
            detach = true;
        else if (!strcmp(argv[i], "--profile") && i + 1 < argc) {
            profile = a16_profile_find(argv[++i]);
            if (profile < 0 || !a16_profiles[profile].available)
                return 2;
        } else if (!strcmp(argv[i], "--seconds") && i + 1 < argc) {
            char *end;
            errno = 0;
            unsigned long value = strtoul(argv[++i], &end, 10);
            if (errno || *end || !value || value > 86400)
                return 2;
            seconds = value;
        } else
            return 2;
    }
    struct a16_missing missing = {0};
    char config_path[PATH_MAX];
    bool explicit_config = config != NULL;
    if (!config) {
        const char *xdg = getenv("XDG_CONFIG_HOME"), *home = getenv("HOME");
        int n = xdg && *xdg ? snprintf(config_path, sizeof(config_path), "%s/ozzy-a16/config.json", xdg)
                : home && *home
                    ? snprintf(config_path, sizeof(config_path), "%s/.config/ozzy-a16/config.json", home)
                    : -1;
        if (n < 0 || (size_t)n >= sizeof(config_path))
            return 2;
        config = config_path;
    }
    if (!a16_missing_load(&missing, config, !explicit_config)) {
        fprintf(stderr, "Invalid or unreadable A16 config: %s\n", config);
        return 2;
    }
    if (missing_arg) {
        memset(&missing, 0, sizeof(missing));
        if (!a16_missing_parse(&missing, missing_arg)) {
            fprintf(stderr, "Invalid missing speaker identity list: %s\n", missing_arg);
            return 2;
        }
    }
    signal(SIGINT, stop_signal);
    signal(SIGTERM, stop_signal);
    int result = 1, error = 0;
    bool configured = false, attached = false;
    libusb_context *usb = NULL;
    libusb_device **devices = NULL;
    libusb_device_handle *handle = NULL;
    struct live *l = calloc(1, sizeof(*l));
    if (!l)
        return 1;
    l->profile = profile;
    l->missing = missing;
    for (unsigned int i = 0; i < A16_PROFILE_COUNT; i++) {
        if (!a16_apply_missing(&l->effective[i], &a16_profiles[i], &missing)) {
            free(l);
            return 2;
        }
    }
    if (list_profiles) {
        for (unsigned int i = 0; i < A16_PROFILE_COUNT; i++) {
            const struct a16_profile *p = &l->effective[i].profile;
            printf("%s: %s (%u channels)\n  Speakers: %s\n  Required A16 room: %s\n", p->name, p->description,
                   p->channels, p->speakers, p->room);
        }
        free(l);
        return 0;
    }
    if (!l->effective[profile].profile.available) {
        fprintf(stderr, "Profile has no available speakers\n");
        free(l);
        return 2;
    }
    if (*missing.text)
        fprintf(stderr, "User missing speakers: %s\n", missing.text);
    l->device_id = SPA_ID_INVALID;
    l->out_src = src_new(SRC_SINC_FASTEST, 16, &error);
    l->in_src = src_new(SRC_SINC_FASTEST, 2, &error);
    if (!l->out_src || !l->in_src)
        goto done;
    /* Verify daemon connectivity before touching the USB device. */
    if (!open_pipewire(l)) {
        fprintf(stderr, "Cannot connect to PipeWire: %s\n", strerror(errno));
        goto done;
    }
    if (libusb_init(&usb))
        goto done;
    ssize_t count = libusb_get_device_list(usb, &devices);
    libusb_device *chosen = NULL;
    unsigned int matches = 0;
    for (ssize_t i = 0; i < count; i++) {
        struct libusb_device_descriptor d;
        if (!libusb_get_device_descriptor(devices[i], &d) && d.idVendor == 0x0a4a && d.idProduct == 0x5a16) {
            chosen = devices[i];
            matches++;
        }
    }
    if (matches != 1 || libusb_get_device_speed(chosen) != LIBUSB_SPEED_HIGH) {
        fprintf(stderr, "Expected exactly one high-speed A16\n");
        goto done;
    }
    if (libusb_open(chosen, &handle)) {
        fprintf(stderr, "Cannot open A16\n");
        goto done;
    }
    const struct ozzy_device *model = ozzy_device_lookup(0x0a4a, 0x5a16);
    l->engine = ozzy_engine_init(calloc(1, ozzy_engine_size(model)), model);
    if (posix_memalign((void **)&l->shm, _Alignof(struct ozzy_shared_memory), sizeof(*l->shm)))
        l->shm = NULL;
    if (!l->engine || !l->shm)
        goto done;
    memset(l->shm, 0, sizeof(*l->shm));
    usb_bus_init(&l->bus, usb, handle, l->engine, NULL);
    configured = true;
    if (!usb_bus_configure(&l->bus, detach))
        goto done;
    ozzy_engine_attach(l->engine, &l->bus.bus, l->shm);
    attached = true;
    ozzy_engine_set_usb_speed(l->engine, OZZY_USB_SPEED_HIGH);
    ozzy_engine_set_rate(l->engine, RATE);
    struct ozzy_pcm_ring out = {l->output, BLOCK, 16, 0}, in = {l->input, BLOCK, 2, 0};
    ozzy_engine_set_pcm_ring(l->engine, OZZY_PCM_OUT, &out);
    ozzy_engine_set_pcm_ring(l->engine, OZZY_PCM_IN, &in);
    l->bus.before_complete = before_complete;
    l->bus.on_progress = on_progress;
    l->bus.audio_data = l;
    if (!ozzy_engine_start(l->engine) || l->bus.failed)
        goto done;
    if (l->shm->audio.sample_rate != RATE ||
        l->engine->ops->transfer_frames(l->engine, OZZY_PCM_OUT) != BLOCK ||
        l->engine->ops->transfer_frames(l->engine, OZZY_PCM_IN) != BLOCK) {
        fprintf(stderr, "Unexpected USB rate or transfer size\n");
        goto done;
    }
    uint64_t start = usb_now(), last = start;
    fprintf(stderr, "READY: USB 16ch out/2ch in, 48000 Hz; selectable card profiles (default 7.1.4t)\n");
    while (!stopping && !atomic_load(&l->failed) && !l->bus.failed &&
           (!seconds || usb_now() - start < (uint64_t)seconds * 1000000000ULL)) {
        usb_bus_poll(&l->bus, 1);
        if (usb_now() - last >= 5000000000ULL) {
            fprintf(
                stderr,
                "stats usb=%llu/%llu pw=%llu/%llu queues=%u/%u xruns=%llu,%llu/%llu,%llu ratios=%.7f/%.7f\n",
                (unsigned long long)l->bus.progress[0], (unsigned long long)l->bus.progress[1],
                (unsigned long long)atomic_load(&l->sink.frames),
                (unsigned long long)atomic_load(&l->source.frames), live_available(&l->sink.fifo),
                live_available(&l->source.fifo), (unsigned long long)atomic_load(&l->sink.underruns),
                (unsigned long long)atomic_load(&l->sink.overruns),
                (unsigned long long)atomic_load(&l->source.underruns),
                (unsigned long long)atomic_load(&l->source.overruns), atomic_load(&l->output_ratio),
                atomic_load(&l->capture_ratio));
            last = usb_now();
        }
    }
    result = l->bus.failed || atomic_load(&l->failed) ? 1 : 0;
done:
    /* Stop callbacks before freeing queues; USB cancellation stays on this thread. */
    close_pipewire(l);
    if (attached)
        ozzy_engine_stop(l->engine);
    if (configured) {
        usb_bus_release(&l->bus);
        if (l->bus.failed)
            result = 1;
    }
    src_delete(l->out_src);
    src_delete(l->in_src);
    free(l->engine);
    free(l->shm);
    if (handle)
        libusb_close(handle);
    if (devices)
        libusb_free_device_list(devices, 1);
    if (usb)
        libusb_exit(usb);
    free(l);
    fprintf(stderr, "PipeWire/USB shutdown result=%d\n", result);
    return result;
}
