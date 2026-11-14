/* SPDX-License-Identifier: MIT */
/*
 * USB MIDI class engine. MIDI uses the shared rings also used by Ploytec devices.
 * The engine publishes zero audio channels for MIDI-only devices.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#ifndef USB_MIDI_ENGINE_H
#define USB_MIDI_ENGINE_H

#include "../../core/ozzy_engine.h"
#include "../../../common/usb_midi.h"

#ifdef __cplusplus
extern "C" {
#endif

#define USB_MIDI_ENGINE_READS 2

/*
 * struct usb_midi_engine
 * @encoder:  only touched while @out_busy is held
 * @out_busy: one OUT transfer in flight; hands the sender role between the MIDI client's kick
 *            and the OUT completion
 */
struct usb_midi_engine {
	struct ozzy_engine base;
	struct usb_midi_encoder encoder;
	bool running;
	bool out_busy;
	uint32_t read_errors[USB_MIDI_ENGINE_READS];
};

void usb_midi_engine_init(struct usb_midi_engine *me, const struct ozzy_device *model);

#ifdef __cplusplus
}
#endif

#endif /* USB_MIDI_ENGINE_H */
