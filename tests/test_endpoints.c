/* SPDX-License-Identifier: MIT */
/*
 * ozzy_engine_bind_endpoints on the Realiser A16's endpoint list (fixed addresses, with endpoints
 * it doesn't use around them), one with PCM in on the wrong interface, and a High Speed
 * high-bandwidth wMaxPacketSize.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#include "fake_bus.h"
#include "check.h"
#include <stdlib.h>

#define N(a) (sizeof(a) / sizeof((a)[0]))

static struct ozzy_engine *engine(uint16_t vid, uint16_t pid)
{
	const struct ozzy_device *model = ozzy_device_lookup(vid, pid);

	return ozzy_engine_init(malloc(ozzy_engine_size(model)), model);
}

/* The endpoint @bound picked, as an address (0: none) */
static uint8_t addr(const struct ozzy_usb_endpoint *ep)
{
	return ep ? ep->address : 0;
}

int main(void)
{
	/* The A16's pipes at alt 1, with endpoints it doesn't use next to them */
	static const struct ozzy_usb_endpoint a16[] = {
		{ 0, 0x02, 0x01, 1, 4, NULL },
		{ 0, 0x83, 0x03, 1, 64, NULL },
		{ 0, 0x04, 0x02, 0, 512, NULL },
		{ 0, 0x05, 0x03, 1, 964, NULL },
		{ 1, 0x81, 0x01, 1, 4, NULL },
		{ 1, 0x86, 0x03, 1, 1024, NULL },
	};
	/* The PCM IN endpoint missing (wrong interface) */
	static const struct ozzy_usb_endpoint a16_partial[] = {
		{ 0, 0x05, 0x03, 1, 964, NULL },
		{ 0, 0x83, 0x03, 1, 64, NULL },
		{ 0, 0x86, 0x03, 1, 1024, NULL },
	};
	const struct ozzy_usb_endpoint *bound[OZZY_PIPE_ROLES];
	struct ozzy_engine *e;

	e = engine(0x0A4A, 0x5A16);
	ozzy_engine_bind_endpoints(e, a16, N(a16), bound);
	CHECK(addr(bound[OZZY_PIPE_PCM_OUT]) == 0x05, "A16 PCM out 0x%02x", addr(bound[OZZY_PIPE_PCM_OUT]));
	CHECK(addr(bound[OZZY_PIPE_PCM_IN]) == 0x86, "A16 PCM in 0x%02x", addr(bound[OZZY_PIPE_PCM_IN]));
	CHECK(e->pipe[OZZY_PIPE_PCM_OUT].max_packet_size == 964 && e->pipe[OZZY_PIPE_PCM_OUT].transfer_type == 3,
	      "A16 PCM out %u type %u", e->pipe[OZZY_PIPE_PCM_OUT].max_packet_size,
	      e->pipe[OZZY_PIPE_PCM_OUT].transfer_type);
	CHECK(e->pipe[OZZY_PIPE_PCM_IN].max_packet_size == 1024 && e->pipe[OZZY_PIPE_PCM_IN].transfer_type == 3,
	      "A16 PCM in %u type %u", e->pipe[OZZY_PIPE_PCM_IN].max_packet_size,
	      e->pipe[OZZY_PIPE_PCM_IN].transfer_type);
	free(e);

	e = engine(0x0A4A, 0x5A16);
	ozzy_engine_bind_endpoints(e, a16_partial, N(a16_partial), bound);
	CHECK(addr(bound[OZZY_PIPE_PCM_OUT]) == 0x05 && !bound[OZZY_PIPE_PCM_IN],
	      "PCM in on the wrong interface must not bind (0x%02x)", addr(bound[OZZY_PIPE_PCM_IN]));
	CHECK(e->pipe[OZZY_PIPE_PCM_IN].address == 0, "unbound role told to the engine");
	free(e);

	{
		const struct ozzy_usb_endpoint hb = { 0, 0x02, 0x01, 1, 0x0800 | 312, NULL };

		CHECK(ozzy_usb_endpoint_bytes(&hb) == 624, "high-bandwidth bytes per interval %u",
		      ozzy_usb_endpoint_bytes(&hb));
	}

	return CHECK_DONE("endpoints");
}
