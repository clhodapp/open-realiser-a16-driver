/* SPDX-License-Identifier: MIT */
#define _POSIX_C_SOURCE 200809L
#include "usb_bus.h"
#include "wav.h"
#include <signal.h>
#include <stdlib.h>

static volatile sig_atomic_t interrupted;
static void on_signal(int sig) { (void)sig; interrupted = 1; }

static void usage(void)
{
    puts("a16-usb inspect [--device BUS:ADDRESS]\n"
         "a16-usb stream --seconds 1..60 [--device BUS:ADDRESS] [--detach] [--rate 44100|48000|88200|96000]\n"
         "                   [--play input.wav] [--capture output.wav] [--log trace.txt]\n"
         "A16 only (0a4a:5a16), 44100/48000/88200/96000 Hz (default 48000), 16 out / 2 in, 24-bit PCM.\n"
         "inspect reads descriptors without opening/claiming interfaces.\n"
         "stream changes device state; silence unless --play is given. WAV playback is\n"
         "one-shot, requires 16 channels at the selected rate, and pads remaining time with silence.\n"
         "--detach permits temporary kernel driver detachment; reattached on clean exit.");
}

static bool number(const char *s, unsigned int max, unsigned int *out)
{
    if (!s[0] || s[0] == '-') return false;
    char *end;
    errno = 0;
    unsigned long v = strtoul(s, &end, 10);
    if (errno || *end || v > max) return false;
    *out = (unsigned int)v;
    return true;
}

static void descriptors(libusb_device *dev, const struct libusb_device_descriptor *d)
{
    printf("device %u:%u %04x:%04x bcdDevice=%04x speed=%d configurations=%u\n",
           libusb_get_bus_number(dev), libusb_get_device_address(dev), d->idVendor, d->idProduct,
           d->bcdDevice, libusb_get_device_speed(dev), d->bNumConfigurations);
    for (unsigned int c = 0; c < d->bNumConfigurations; c++) {
        struct libusb_config_descriptor *config;
        int ret = libusb_get_config_descriptor(dev, (uint8_t)c, &config);
        if (ret) { fprintf(stderr, "descriptor: %s\n", libusb_error_name(ret)); continue; }
        printf(" configuration %u interfaces=%u\n", config->bConfigurationValue, config->bNumInterfaces);
        for (unsigned int i = 0; i < config->bNumInterfaces; i++)
            for (int a = 0; a < config->interface[i].num_altsetting; a++) {
                const struct libusb_interface_descriptor *id = &config->interface[i].altsetting[a];
                printf("  interface %u alt=%u class=%02x subclass=%02x protocol=%02x\n",
                       id->bInterfaceNumber, id->bAlternateSetting, id->bInterfaceClass,
                       id->bInterfaceSubClass, id->bInterfaceProtocol);
                for (unsigned int e = 0; e < id->bNumEndpoints; e++) {
                    const struct libusb_endpoint_descriptor *ep = &id->endpoint[e];
                    printf("   endpoint %02x type=%u maxPacket=%u interval=%u\n", ep->bEndpointAddress,
                           ep->bmAttributes & 3, ep->wMaxPacketSize, ep->bInterval);
                }
            }
        libusb_free_config_descriptor(config);
    }
}

int main(int argc, char **argv)
{
    if (argc == 2 && !strcmp(argv[1], "--help")) { usage(); return 0; }
    if (argc < 2 || (strcmp(argv[1], "inspect") && strcmp(argv[1], "stream"))) { usage(); return 2; }
    bool stream = !strcmp(argv[1], "stream"), detach = false, select = false;
    unsigned int rate = 48000, seconds = 0, bus_number = 0, address = 0;
    const char *play = NULL, *capture = NULL, *log = NULL;
    for (int i = 2; i < argc; i++) {
        if (stream && !strcmp(argv[i], "--detach")) detach = true;
        else if (!strcmp(argv[i], "--device") && i + 1 < argc) {
            char value[32];
            if (strlen(argv[++i]) >= sizeof(value)) goto bad;
            strcpy(value, argv[i]);
            char *colon = strchr(value, ':');
            if (!colon) goto bad;
            *colon++ = 0;
            if (!number(value, 255, &bus_number) || !number(colon, 127, &address) || !address) goto bad;
            select = true;
        } else if (stream && !strcmp(argv[i], "--seconds") && i + 1 < argc) {
            if (!number(argv[++i], 60, &seconds) || !seconds) goto bad;
        } else if (stream && !strcmp(argv[i], "--rate") && i + 1 < argc) {
            if (!number(argv[++i], 96000, &rate) || (rate != 44100 && rate != 48000 && rate != 88200 && rate != 96000)) goto bad;
        } else if (stream && !strcmp(argv[i], "--play") && i + 1 < argc) play = argv[++i];
        else if (stream && !strcmp(argv[i], "--capture") && i + 1 < argc) capture = argv[++i];
        else if (stream && !strcmp(argv[i], "--log") && i + 1 < argc) log = argv[++i];
        else goto bad;
    }
    if (stream && !seconds) goto bad;
    /* Validate input files before initializing USB, much less claiming the device. */
    uint32_t frames = (seconds + 2) * rate, play_frames = 0;
    uint8_t *out = stream ? calloc(frames, 16 * 3) : NULL;
    uint8_t *in = stream ? calloc(frames, 2 * 3) : NULL;
    if (stream && (!out || !in)) { free(out); free(in); return 1; }
    if (play && !wav_read(play, rate, 16, out, seconds * rate, &play_frames)) {
        fprintf(stderr, "playback must be a valid %u-Hz 16-channel 24-bit PCM WAV no longer than --seconds\n", rate);
        free(out); free(in); return 2;
    }
    FILE *record = NULL, *trace = NULL;
    int result = 1;
    libusb_context *usb = NULL;
    libusb_device **devices = NULL;
    libusb_device_handle *handle = NULL;
    struct ozzy_engine *engine = NULL;
    struct ozzy_shared_memory *shm = NULL;
    struct usb_bus b;
    bool bus_ready = false, attached = false;
    if (capture && !(record = fopen(capture, "wbx"))) { perror(capture); goto done; }
    if (log && !(trace = fopen(log, "wx"))) { perror(log); goto done; }
    int ret = libusb_init(&usb);
    if (ret) { fprintf(stderr, "libusb init: %s\n", libusb_error_name(ret)); goto done; }
    ssize_t count = libusb_get_device_list(usb, &devices);
    if (count < 0) { fprintf(stderr, "list: %s\n", libusb_error_name((int)count)); goto done; }
    libusb_device *chosen = NULL;
    unsigned int matches = 0;
    for (ssize_t i = 0; i < count; i++) {
        struct libusb_device_descriptor d;
        if (libusb_get_device_descriptor(devices[i], &d) || d.idVendor != 0x0a4a || d.idProduct != 0x5a16) continue;
        if (select && (libusb_get_bus_number(devices[i]) != bus_number ||
                       libusb_get_device_address(devices[i]) != address)) continue;
        chosen = devices[i]; matches++;
        descriptors(chosen, &d);
    }
    fflush(stdout);
    if (!stream) { result = 0; if (!matches) puts("No matching A16 found."); goto done; }
    if (matches != 1) { fprintf(stderr, "expected one A16, found %u; select --device BUS:ADDRESS\n", matches); goto done; }
    if (libusb_get_device_speed(chosen) != LIBUSB_SPEED_HIGH) { fprintf(stderr, "A16 must be at USB high speed\n"); goto done; }
    ret = libusb_open(chosen, &handle);
    if (ret) { fprintf(stderr, "open A16: %s (check USB device permissions)\n", libusb_error_name(ret)); goto done; }
    const struct ozzy_device *model = ozzy_device_lookup(0x0a4a, 0x5a16);
    engine = ozzy_engine_init(calloc(1, ozzy_engine_size(model)), model);
    if (posix_memalign((void **)&shm, _Alignof(struct ozzy_shared_memory), sizeof(*shm))) shm = NULL;
    if (shm) memset(shm, 0, sizeof(*shm));
    if (!engine || !shm) goto done;
    usb_bus_init(&b, usb, handle, engine, trace); bus_ready = true;
    if (!usb_bus_configure(&b, detach)) goto done;
    ozzy_engine_attach(engine, &b.bus, shm); attached = true;
    ozzy_engine_set_usb_speed(engine, OZZY_USB_SPEED_HIGH);
    ozzy_engine_set_rate(engine, rate);
    struct ozzy_pcm_ring playback = { out, frames, 16, 0 }, recording = { in, frames, 2, 0 };
    ozzy_engine_set_pcm_ring(engine, OZZY_PCM_OUT, &playback);
    ozzy_engine_set_pcm_ring(engine, OZZY_PCM_IN, &recording);
    signal(SIGINT, on_signal); signal(SIGTERM, on_signal);
    fprintf(stderr, "Starting experimental A16 engine: %u Hz, %u playback frames, %u seconds\n",
            rate, play_frames, seconds);
    if (!ozzy_engine_start(engine) || b.failed) goto done;
    uint64_t start = usb_now(), deadline = start + (uint64_t)seconds * 1000000000ULL, last = start;
    while (!interrupted && !b.failed && usb_now() < deadline) {
        usb_bus_poll(&b, 1);
        if (b.progress[0] >= frames || b.progress[1] >= frames) {
            fprintf(stderr, "PCM exceeded bounded recording buffers\n"); b.failed = true; break;
        }
        if (usb_now() - last >= 1000000000ULL) {
            fprintf(stderr, "PCM frames playback=%llu capture=%llu, pending=%u\n",
                    (unsigned long long)b.progress[0], (unsigned long long)b.progress[1], b.pending_count);
            last = usb_now();
        }
    }
    result = b.failed || !b.progress[0] || !b.progress[1] ? 1 : 0;
done:
    if (attached) ozzy_engine_stop(engine);
    if (bus_ready) {
        if (record) {
            uint64_t got = b.progress[OZZY_PCM_IN];
            if (got > (uint64_t)seconds * rate) got = (uint64_t)seconds * rate;
            if (!wav_write(record, rate, 2, in, (uint32_t)got)) { perror("capture write"); result = 1; }
            fprintf(stderr, "Captured %llu frames; USB transfers out=%llu in=%llu\n",
                    (unsigned long long)got, (unsigned long long)b.completed[0],
                    (unsigned long long)b.completed[1]);
        }
        usb_bus_release(&b);
        if (b.failed) result = 1;
        fprintf(stderr, "Stream/cleanup result=%d\n", result);
    }
    if (record && fclose(record)) result = 1;
    if (trace && fclose(trace)) result = 1;
    free(engine); free(shm);
    if (handle) libusb_close(handle);
    if (devices) libusb_free_device_list(devices, 1);
    if (usb) libusb_exit(usb);
    free(out); free(in);
    return result;
bad:
    usage(); return 2;
}
