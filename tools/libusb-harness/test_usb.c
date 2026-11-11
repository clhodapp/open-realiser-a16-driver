/* SPDX-License-Identifier: MIT */
#include "usb_bus.h"
#include <assert.h>
#include <stdlib.h>

/* Linker-wrapped libusb I/O: exercise the actual backend without opening USB. */
static struct libusb_transfer *queued[32];
static unsigned int count, delivered;
static bool fail_submit, short_control;
static int control_result = -1;
static int last_status;
int __wrap_libusb_submit_transfer(struct libusb_transfer *t)
{
    if (fail_submit) return LIBUSB_ERROR_NO_DEVICE;
    assert(count < 32); queued[count++] = t; return 0;
}
int __wrap_libusb_cancel_transfer(struct libusb_transfer *t)
{
    t->status = LIBUSB_TRANSFER_CANCELLED; return 0;
}
int __wrap_libusb_handle_events_timeout(libusb_context *ctx, struct timeval *tv)
{
    (void)ctx; (void)tv;
    if (count) {
        struct libusb_transfer *t = queued[--count];
        t->callback(t);
    }
    return 0;
}
int __wrap_libusb_control_transfer(libusb_device_handle *h, uint8_t type, uint8_t request,
    uint16_t value, uint16_t index, unsigned char *data, uint16_t length, unsigned int timeout)
{
    (void)h; (void)type; (void)request; (void)value; (void)index; (void)timeout;
    if (data) memset(data, 0xaa, length);
    return control_result >= 0 ? control_result : short_control && length ? length - 1 : length;
}
static void completion(struct ozzy_engine *e, uint8_t pipe, void *token, int status, uint32_t bytes)
{
    (void)e; (void)pipe; (void)token; (void)bytes;
    delivered++; last_status = status;
}
static const struct ozzy_engine_ops engine_ops = { .complete = completion };
int main(void)
{
    struct ozzy_engine e = { .ops = &engine_ops };
    struct usb_bus b;
    uint8_t data[512] = {0};
    usb_bus_init(&b, NULL, NULL, &e, NULL);
    b.types[0x86] = LIBUSB_TRANSFER_TYPE_BULK;
    e.pipe[OZZY_PIPE_PCM_IN].address = 0x86;
    assert(b.bus.ops->submit(&b, 0x83, data, 9, NULL));
    assert(!b.pending_count); /* Missing optional MIDI is not an audio failure. */
    for (unsigned int i = 0; i < 8; i++) assert(b.bus.ops->submit(&b, 0x86, data, 512, (void *)(uintptr_t)i));
    assert(b.pending_count == 8);
    b.bus.ops->abort(&b);
    assert(!b.pending_count && !b.pending && !count && delivered == 8);
    assert(last_status == OZZY_STATUS_ABORTED && !b.failed);

    usb_bus_init(&b, NULL, NULL, &e, NULL);
    b.types[0x83] = LIBUSB_TRANSFER_TYPE_BULK;
    assert(b.bus.ops->submit(&b, 0x83, data, 9, NULL));
    assert(queued[0]->timeout == 0); /* MIDI can legitimately remain idle. */
    b.bus.ops->abort(&b);

    usb_bus_init(&b, NULL, NULL, &e, NULL);
    b.types[0x86] = LIBUSB_TRANSFER_TYPE_INTERRUPT;
    assert(b.bus.ops->submit(&b, 0x86, data, 512, NULL));
    queued[0]->status = LIBUSB_TRANSFER_COMPLETED;
    queued[0]->actual_length = 511;
    usb_bus_poll(&b, 0);
    assert(b.failed && !b.pending_count && last_status == OZZY_STATUS_ABORTED);

    usb_bus_init(&b, NULL, NULL, &e, NULL);
    b.types[0x86] = LIBUSB_TRANSFER_TYPE_BULK;
    assert(b.bus.ops->submit(&b, 0x86, data, 512, NULL));
    queued[0]->status = LIBUSB_TRANSFER_NO_DEVICE;
    usb_bus_poll(&b, 0);
    assert(b.failed && !b.pending_count && last_status == OZZY_STATUS_ABORTED);

    usb_bus_init(&b, NULL, NULL, &e, NULL);
    b.types[0x86] = LIBUSB_TRANSFER_TYPE_BULK;
    fail_submit = true;
    assert(!b.bus.ops->submit(&b, 0x86, data, 512, NULL));
    assert(b.failed && !b.pending_count && !b.pending);
    uint8_t reply[3];
    short_control = true;
    assert(!b.bus.ops->control(&b, 0xa2, 0x81, 0x100, 0x86, reply, 3));
    short_control = false;
    assert(b.bus.ops->control(&b, 0xa2, 0x81, 0x100, 0x86, reply, 3));
    uint8_t firmware[15];
    control_result = 3;
    assert(b.bus.ops->control(&b, 0xc0, 0x56, 0, 0, firmware, sizeof(firmware)));
    control_result = 2;
    assert(!b.bus.ops->control(&b, 0xc0, 0x56, 0, 0, firmware, sizeof(firmware)));
    puts("libusb cancellation, disconnect, short transfer and submit failures: ok");
    return 0;
}
