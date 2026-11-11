/* SPDX-License-Identifier: MIT */
#define _POSIX_C_SOURCE 200809L
#include "usb_bus.h"
#include <limits.h>
#include <stdlib.h>
#include <time.h>

struct usb_pending {
    struct usb_pending *next;
    struct usb_bus *bus;
    struct libusb_transfer *transfer;
    void *token;
};

uint64_t usb_now(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000ULL + (uint64_t)t.tv_nsec;
}

static uint64_t bus_time(void *ctx) { (void)ctx; return usb_now(); }
static uint64_t bus_hz(void *ctx) { (void)ctx; return 1000000000ULL; }
static void bus_sleep(void *ctx, unsigned int ms)
{
    (void)ctx;
    struct timespec t = { ms / 1000, (long)(ms % 1000) * 1000000L };
    while (nanosleep(&t, &t) && errno == EINTR) {}
}

static void LIBUSB_CALL completed(struct libusb_transfer *t)
{
    struct usb_pending *p = t->user_data;
    struct usb_bus *b = p->bus;
    struct usb_pending **link = &b->pending;
    while (*link && *link != p) link = &(*link)->next;
    if (*link) *link = p->next;
    b->pending_count--;
    int status = 0;
    if (t->status != LIBUSB_TRANSFER_COMPLETED) {
        status = OZZY_STATUS_ABORTED;
        if (!b->draining) {
            fprintf(stderr, "USB endpoint %02x failed: transfer status %d\n", t->endpoint, t->status);
            b->failed = true;
        }
    } else if (!b->draining) {
        unsigned int dir = (t->endpoint & 0x80) ? OZZY_PCM_IN : OZZY_PCM_OUT;
        b->completed[dir]++;
        b->bytes[dir] += (unsigned int)t->actual_length;
        /* Fail closed on partial audio packets: the engine otherwise pads some input. */
        if ((t->endpoint == b->engine->pipe[OZZY_PIPE_PCM_IN].address ||
             t->endpoint == b->engine->pipe[OZZY_PIPE_PCM_OUT].address) &&
            t->actual_length != t->length) {
            fprintf(stderr, "short PCM transfer on %02x: %d/%d\n", t->endpoint, t->actual_length, t->length);
            b->failed = true;
            status = OZZY_STATUS_ABORTED;
        }
    }
    if (b->trace)
        fprintf(b->trace, "%llu complete ep=%02x status=%d bytes=%d/%d\n",
                (unsigned long long)usb_now(), t->endpoint, t->status, t->actual_length, t->length);
    /* Remove from the list before the engine resubmits the same token. */
    if (b->draining || b->failed) status = OZZY_STATUS_ABORTED;
    if (!status && b->before_complete) b->before_complete(b->audio_data, t->endpoint);
    ozzy_engine_complete(b->engine, t->endpoint, p->token, status,
                         status ? 0 : (uint32_t)t->actual_length);
    libusb_free_transfer(t);
    free(p);
}

static bool bus_submit(void *ctx, uint8_t ep, void *buf, uint32_t size, void *token)
{
    struct usb_bus *b = ctx;
    /* The shared Xone engine always polls MIDI IN, even for audio-only profiles.
     * There is no MIDI consumer here; an absent optional endpoint is not an audio failure. */
    if (!b->types[ep] && ep == 0x83 && !b->draining && !b->failed) return true;
    if (b->draining || b->failed || !size || size > INT_MAX ||
        (b->types[ep] != LIBUSB_TRANSFER_TYPE_BULK && b->types[ep] != LIBUSB_TRANSFER_TYPE_INTERRUPT)) {
        b->failed = true;
        return false;
    }
    struct usb_pending *p = calloc(1, sizeof(*p));
    struct libusb_transfer *t = libusb_alloc_transfer(0);
    if (!p || !t) { free(p); if (t) libusb_free_transfer(t); b->failed = true; return false; }
    p->bus = b; p->transfer = t; p->token = token;
    bool pcm = ep == b->engine->pipe[OZZY_PIPE_PCM_IN].address ||
               ep == b->engine->pipe[OZZY_PIPE_PCM_OUT].address;
    /* Idle MIDI has no completion deadline; audio must make progress. */
    unsigned int timeout = pcm ? 2000 : 0;
    if (b->types[ep] == LIBUSB_TRANSFER_TYPE_BULK)
        libusb_fill_bulk_transfer(t, b->handle, ep, buf, (int)size, completed, p, timeout);
    else
        libusb_fill_interrupt_transfer(t, b->handle, ep, buf, (int)size, completed, p, timeout);
    int ret = libusb_submit_transfer(t);
    if (ret) {
        fprintf(stderr, "submit %02x: %s\n", ep, libusb_error_name(ret));
        libusb_free_transfer(t); free(p); b->failed = true; return false;
    }
    p->next = b->pending; b->pending = p; b->pending_count++;
    if (b->trace) fprintf(b->trace, "%llu submit ep=%02x bytes=%u\n",
                           (unsigned long long)usb_now(), ep, size);
    return true;
}

static bool bus_control(void *ctx, uint8_t type, uint8_t req, uint16_t value,
                        uint16_t index, void *data, uint16_t size)
{
    struct usb_bus *b = ctx;
    if ((type & 0x80) && size) memset(data, 0, size);
    int ret = libusb_control_transfer(b->handle, type, req, value, index, data, size, 2000);
    FILE *log = b->trace ? b->trace : stderr;
    fprintf(log, "%llu control type=%02x req=%02x value=%04x index=%04x length=%u result=%d data=",
            (unsigned long long)usb_now(), type, req, value, index, size, ret);
    unsigned int got = ret < 0 ? 0 : (unsigned int)ret;
    for (unsigned int i = 0; i < got && i < size; i++) fprintf(log, "%02x", ((uint8_t *)data)[i]);
    fputc('\n', log);
    /* 'V' is variable length: the engine requests 15 bytes but parses only the
     * mandatory three-byte prefix. The attached A16 returns exactly three. */
    if (type == 0xc0 && req == 0x56 && value == 0 && index == 0 && size == 15)
        return ret >= 3 && ret <= size;
    /* Fixed-size status/rate responses must be complete. */
    return ret == size;
}

static int bus_clear(void *ctx, uint8_t ep)
{
    struct usb_bus *b = ctx;
    int ret = libusb_clear_halt(b->handle, ep);
    fprintf(b->trace ? b->trace : stderr, "%llu clear-halt ep=%02x result=%d\n",
            (unsigned long long)usb_now(), ep, ret);
    return ret;
}
static bool bus_timer(void *ctx, uint32_t us)
{
    struct usb_bus *b = ctx;
    b->timer_us = us; b->timer_next = usb_now() + (uint64_t)us * 1000;
    return us != 0;
}
static void bus_timer_stop(void *ctx) { ((struct usb_bus *)ctx)->timer_us = 0; }
static void bus_progress(void *ctx, enum ozzy_pcm_dir dir, uint64_t frames)
{
    struct usb_bus *b = ctx;
    b->progress[dir] = frames;
    if (!b->draining && !b->failed && b->on_progress)
        b->on_progress(b->audio_data, dir, frames);
}

static void bus_abort(void *ctx)
{
    struct usb_bus *b = ctx;
    b->draining = true;
    for (struct usb_pending *p = b->pending; p; p = p->next) {
        int ret = libusb_cancel_transfer(p->transfer);
        if (ret && ret != LIBUSB_ERROR_NOT_FOUND)
            fprintf(stderr, "cancel: %s\n", libusb_error_name(ret));
    }
    uint64_t deadline = usb_now() + 5000000000ULL;
    while (b->pending_count) {
        struct timeval wait = { 0, 10000 };
        int ret = libusb_handle_events_timeout(b->usb, &wait);
        if ((ret && ret != LIBUSB_ERROR_INTERRUPTED) || usb_now() > deadline) {
            /* Never free a live transfer/buffer: exit and let the OS reap the process. */
            fprintf(stderr, "USB cancellation did not drain; terminating with buffers intact\n");
            _Exit(1);
        }
    }
}

static const struct ozzy_bus_ops ops = {
    .submit = bus_submit, .control = bus_control, .clear_halt = bus_clear,
    .start_timer = bus_timer, .stop_timer = bus_timer_stop, .pcm_progress = bus_progress,
    .sleep_ms = bus_sleep, .time = bus_time, .abort = bus_abort, .time_hz = bus_hz,
};

void usb_bus_init(struct usb_bus *b, libusb_context *usb, libusb_device_handle *handle,
                  struct ozzy_engine *engine, FILE *trace)
{
    memset(b, 0, sizeof(*b));
    b->bus.ops = &ops; b->bus.ctx = b; b->usb = usb; b->handle = handle;
    b->engine = engine; b->trace = trace;
    for (unsigned int i = 0; i < 256; i++) b->previous_alt[i] = -1;
}

/* Some vendor-specific firmware stalls GET_INTERFACE. Linux still tracks the
 * selected setting in sysfs; use that observed value rather than assuming zero. */
static int sysfs_alt(libusb_device *dev, unsigned int config, unsigned int intf)
{
    uint8_t ports[8];
    int n = libusb_get_port_numbers(dev, ports, sizeof(ports));
    if (n <= 0) return -1;
    char path[192];
    int at = snprintf(path, sizeof(path), "/sys/bus/usb/devices/%u-", libusb_get_bus_number(dev));
    for (int i = 0; i < n; i++)
        at += snprintf(path + at, sizeof(path) - (size_t)at, "%s%u", i ? "." : "", ports[i]);
    snprintf(path + at, sizeof(path) - (size_t)at, ":%u.%u/bAlternateSetting", config, intf);
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    unsigned int alt;
    int read = fscanf(f, "%u", &alt);
    fclose(f);
    return read == 1 && alt <= 255 ? (int)alt : -1;
}

bool usb_bus_configure(struct usb_bus *b, bool detach)
{
    struct libusb_config_descriptor *config = NULL;
    int ret = libusb_get_active_config_descriptor(libusb_get_device(b->handle), &config);
    if (ret) { fprintf(stderr, "active configuration: %s\n", libusb_error_name(ret)); return false; }
    b->config_value = config->bConfigurationValue;
    struct ozzy_usb_endpoint eps[32];
    unsigned int n = 0;
    bool ok = false;
    const struct ozzy_device_profile *profile = &b->engine->profile;
    for (unsigned int i = 0; i < profile->interface_count; i++) {
        unsigned int number = profile->interfaces[i], alt = profile->alt_settings[i];
        const struct libusb_interface_descriptor *desc = NULL;
        for (unsigned int j = 0; j < config->bNumInterfaces; j++)
            for (int k = 0; k < config->interface[j].num_altsetting; k++) {
                const struct libusb_interface_descriptor *d = &config->interface[j].altsetting[k];
                if (d->bInterfaceNumber == number && d->bAlternateSetting == alt) desc = d;
            }
        if (!desc) { fprintf(stderr, "missing interface %u alt %u\n", number, alt); goto done; }
        ret = libusb_kernel_driver_active(b->handle, (int)number);
        if (ret == 1) {
            if (!detach) { fprintf(stderr, "interface %u has a driver; use --detach explicitly\n", number); goto done; }
            ret = libusb_detach_kernel_driver(b->handle, (int)number);
            if (ret) goto usb_error;
            b->detached[number] = true;
        } else if (ret < 0 && ret != LIBUSB_ERROR_NOT_SUPPORTED) goto usb_error;
        ret = libusb_claim_interface(b->handle, (int)number);
        if (ret) goto usb_error;
        b->claimed[number] = true;
        unsigned char previous = 0;
        ret = libusb_control_transfer(b->handle, 0x81, LIBUSB_REQUEST_GET_INTERFACE,
                                     0, (uint16_t)number, &previous, 1, 2000);
        fprintf(b->trace ? b->trace : stderr, "GET_INTERFACE interface=%u result=%d\n", number, ret);
        if (ret != 1) {
            int observed = sysfs_alt(libusb_get_device(b->handle), config->bConfigurationValue, number);
            if (observed < 0) { fprintf(stderr, "cannot read previous alternate setting on %u\n", number); goto done; }
            previous = (unsigned char)observed;
            fprintf(stderr, "GET_INTERFACE failed (%d); sysfs reports interface %u alt %u\n", ret, number, previous);
        }
        b->previous_alt[number] = previous;
        fprintf(b->trace ? b->trace : stderr, "interface %u alt %u -> %u\n", number, previous, alt);
        ret = libusb_set_interface_alt_setting(b->handle, (int)number, (int)alt);
        if (ret) goto usb_error;
        for (unsigned int k = 0; k < desc->bNumEndpoints; k++) {
            const struct libusb_endpoint_descriptor *ep = &desc->endpoint[k];
            if (n == 32) { fprintf(stderr, "too many endpoints\n"); goto done; }
            eps[n++] = (struct ozzy_usb_endpoint){ (uint8_t)number, ep->bEndpointAddress,
                ep->bmAttributes, ep->bInterval, ep->wMaxPacketSize, NULL };
            b->types[ep->bEndpointAddress] = ep->bmAttributes & 3;
        }
    }
    const struct ozzy_usb_endpoint *bound[OZZY_PIPE_ROLES];
    ozzy_engine_bind_endpoints(b->engine, eps, n, bound);
    if (!bound[OZZY_PIPE_MIDI_IN]) fprintf(stderr, "No optional MIDI IN endpoint; audio-only validation\n");
    for (unsigned int role = OZZY_PIPE_PCM_OUT; role <= OZZY_PIPE_PCM_IN; role++) {
        if (!bound[role] || (bound[role]->attributes & 3) < 2 ||
            (bound[role]->max_packet != 512 &&
             bound[role]->max_packet != (role == OZZY_PIPE_PCM_IN ? 1024 : 964))) {
            fprintf(stderr, "missing or unsupported PCM endpoint layout (role %u)\n", role); goto done;
        }
    }
    ok = true;
    goto done;
usb_error:
    fprintf(stderr, "configure: %s\n", libusb_error_name(ret));
done:
    libusb_free_config_descriptor(config);
    return ok;
}

int usb_bus_poll(struct usb_bus *b, unsigned int ms)
{
    struct timeval wait = { ms / 1000, (int)(ms % 1000) * 1000 };
    int ret = libusb_handle_events_timeout(b->usb, &wait);
    if (ret && ret != LIBUSB_ERROR_INTERRUPTED) b->failed = true;
    if (b->timer_us && usb_now() >= b->timer_next && !b->failed) {
        ozzy_engine_timer(b->engine);
        b->timer_next = usb_now() + (uint64_t)b->timer_us * 1000;
    }
    return ret;
}

void usb_bus_release(struct usb_bus *b)
{
    bus_timer_stop(b);
    bus_abort(b);
    for (int i = 255; i >= 0; i--) {
        if (b->claimed[i]) {
            int restore = 0;
            if (b->previous_alt[i] >= 0) {
                restore = libusb_set_interface_alt_setting(b->handle, i, b->previous_alt[i]);
            }
            int ret = libusb_release_interface(b->handle, i);
            if (ret) { fprintf(stderr, "release interface %d: %s\n", i, libusb_error_name(ret)); b->failed = true; }
            /* USBDEVFS_RELEASEINTERFACE can finish resetting the A16 after
             * SET_INTERFACE returned an error. Verify after that cleanup. */
            if (b->previous_alt[i] >= 0) {
                int observed = sysfs_alt(libusb_get_device(b->handle), b->config_value, (unsigned int)i);
                if (observed != b->previous_alt[i]) {
                    fprintf(stderr, "restore interface %d failed: requested alt %d, observed %d (SET_INTERFACE %d)\n",
                            i, b->previous_alt[i], observed, restore);
                    b->failed = true;
                } else if (restore) {
                    fprintf(stderr, "SET_INTERFACE %d returned %s; interface release restored alt %d (sysfs verified)\n",
                            i, libusb_error_name(restore), observed);
                }
            }
        }
        if (b->detached[i]) {
            int ret = libusb_attach_kernel_driver(b->handle, i);
            if (ret) { fprintf(stderr, "reattach interface %d: %s\n", i, libusb_error_name(ret)); b->failed = true; }
        }
        b->claimed[i] = b->detached[i] = false;
    }
}
