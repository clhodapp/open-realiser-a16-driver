/* SPDX-License-Identifier: MIT */
/*
 * Ploytec USB Protocol Definitions
 *
 * Constants of the Ploytec USB audio chipset in the Realiser A16.
 * Used by the Linux userspace driver.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef OZZY_PLOYTEC_DEFS_H
#define OZZY_PLOYTEC_DEFS_H

#ifdef __cplusplus
extern "C" {
#endif

/* Ploytec GmbH's USB vendor ID (also used for OEM boards). Devices are listed in common/ozzy_devices.def */
#define PLOYTEC_VENDOR_ID           0x0A4A

/* USB endpoint addresses (low nibble) */
#define PLOYTEC_EP_PCM_OUT          0x05
#define PLOYTEC_EP_PCM_IN           0x06

/* PCM topology */
#define PLOYTEC_MAX_OUT_CHANNELS    16  /* audio lanes in an output frame */
#define PLOYTEC_MAX_IN_CHANNELS     16  /* audio lanes in an input frame */
#define PLOYTEC_OUT_FRAME_SIZE      48  /* bytes per device output frame */
#define PLOYTEC_IN_FRAME_SIZE       64  /* bytes per device input frame */

/* USB sub-packet sizes (minimal transfer unit) */
#define PLOYTEC_BULK_OUT_SUBPKT_SIZE  512   /* bytes per bulk output sub-packet */
#define PLOYTEC_INT_OUT_SUBPKT_SIZE   482   /* bytes per interrupt output sub-packet */
#define PLOYTEC_IN_SUBPKT_SIZE        512   /* bytes per input sub-packet (bulk & interrupt) */
#define PLOYTEC_FRAMES_PER_OUT_SUBPKT 10    /* audio frames per output sub-packet */
#define PLOYTEC_FRAMES_PER_IN_SUBPKT  8     /* audio frames per input sub-packet */

/*
 * PCM transfers: any whole number of sub-packets (the device takes and sends them one by one;
 * .claude/re/ploytec_bulk_devices.md). Ozzy goes up to 200 frames, the official driver's size.
 */
#define PLOYTEC_MAX_XFER_FRAMES     200
#define PLOYTEC_MAX_OUT_SUBPKTS     (PLOYTEC_MAX_XFER_FRAMES / PLOYTEC_FRAMES_PER_OUT_SUBPKT)   /* 20 */
#define PLOYTEC_MAX_IN_SUBPKTS      (PLOYTEC_MAX_XFER_FRAMES / PLOYTEC_FRAMES_PER_IN_SUBPKT)    /* 25 */

/* USB interface configuration */
#define PLOYTEC_NUM_INTERFACES      2
#define PLOYTEC_ALT_SETTING         1

/* bmRequestType for vendor requests ('V', 'I') */
#define PLOYTEC_REQTYPE_VENDOR_IN   0xC0  /* device-to-host, vendor, device */
#define PLOYTEC_REQTYPE_VENDOR_OUT  0x40  /* host-to-device, vendor, device */

/* Vendor request command bytes */
#define PLOYTEC_CMD_FIRMWARE        0x56  /* 'V' - read firmware version (15 bytes) */
#define PLOYTEC_CMD_STATUS          0x49  /* 'I' - read/write hardware status */
#define PLOYTEC_CMD_SET_RATE_REQ    0x01  /* bRequest for SET_CUR sample rate */
#define PLOYTEC_CMD_SET_RATE_TYPE   0x22  /* bmRequestType for SET_CUR sample rate */
#define PLOYTEC_CMD_GET_RATE_REQ    0x81  /* bRequest for GET_CUR sample rate */
#define PLOYTEC_CMD_GET_RATE_TYPE   0xA2  /* bmRequestType for GET_CUR sample rate */

/* SET_CUR sample rate endpoint addresses (wIndex field) */
#define PLOYTEC_EP_RATE_IN          0x0086
#define PLOYTEC_EP_RATE_OUT         0x0005

/* MIDI idle/sync byte (sent when no MIDI data is pending) */
#define PLOYTEC_MIDI_IDLE_BYTE      0xFD

/* Control byte after each MIDI out slot when there is no control data (see ploytec_midi.h) */
#define PLOYTEC_MIDI_CONTROL_IDLE   0xFF

#ifdef __cplusplus
}
#endif

#endif /* OZZY_PLOYTEC_DEFS_H */
