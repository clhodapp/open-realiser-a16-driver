/* SPDX-License-Identifier: MIT */
#ifndef OZZY_LIBUSB_BUS_H
#define OZZY_LIBUSB_BUS_H
#include <libusb.h>
#include "../../engine/core/ozzy_engine.h"

struct usb_pending;
/* All engine and libusb calls run on one thread, including cancellation. */
struct usb_bus {
    struct ozzy_bus bus;
    libusb_context *usb;
    libusb_device_handle *handle;
    struct ozzy_engine *engine;
    struct usb_pending *pending;
    unsigned int pending_count;
    uint8_t types[256];
    bool claimed[256], detached[256];
    int previous_alt[256];
    uint8_t config_value;
    uint32_t timer_us;
    uint64_t timer_next, progress[OZZY_PCM_DIRS];
    uint64_t completed[OZZY_PCM_DIRS], bytes[OZZY_PCM_DIRS];
    bool draining, failed;
    FILE *trace;
};
uint64_t usb_now(void);
void usb_bus_init(struct usb_bus *b, libusb_context *usb, libusb_device_handle *handle,
                  struct ozzy_engine *engine, FILE *trace);
bool usb_bus_configure(struct usb_bus *b, bool detach);
int usb_bus_poll(struct usb_bus *b, unsigned int timeout_ms);
void usb_bus_release(struct usb_bus *b);
#endif
