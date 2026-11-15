/* SPDX-License-Identifier: MIT */
/*
 * The engine: one factory for every platform, and what all families share.
 *
 * Copyright (C) 2024 Marcel Bierling <marcel@hackerman.art>
 */

#include "ozzy_engine.h"
#include "../shared/ozzy_log.h"
#include "../devices/ploytec/ploytec_engine.h"

size_t ozzy_engine_size(const struct ozzy_device *model)
{
	switch (model->family) {
	case OZZY_FAMILY_PLOYTEC_XONE:  return sizeof(struct ploytec_engine);
	default:                        return 0;
	}
}

struct ozzy_engine *ozzy_engine_init(void *mem, const struct ozzy_device *model)
{
	size_t size = ozzy_engine_size(model);

	if (!mem || !size)
		return NULL;
	memset(mem, 0, size);
	ploytec_engine_init((struct ploytec_engine *)mem, model);
	return (struct ozzy_engine *)mem;
}

void ozzy_engine_attach(struct ozzy_engine *e, const struct ozzy_bus *bus, volatile struct ozzy_shared_memory *shm)
{
	struct ozzy_engine_memory *m = &e->mem;

	ozzy_engine_free_memory(e);
	e->bus = bus;
	e->shm = shm;
	memset((void *)&shm->stats, 0, sizeof(shm->stats));
	shm->stats.version = OZZY_STATS_VERSION;
	shm->stats.size = sizeof(shm->stats);
	memset(e->stats_last, 0, sizeof(e->stats_last));
	memset(e->stats_late, 0, sizeof(e->stats_late));
	shm->magic = OZZY_MAGIC;
	shm->version = OZZY_SHARED_VERSION;
	shm->vendor_id = e->model->vid;
	shm->product_id = e->model->pid;
	if (!shm->product_name[0])
		ozzy_strlcpy((char *)shm->product_name, e->model->name, sizeof(shm->product_name));
	ozzy_settings_defaults(&e->settings);

	/* Transfers in the shared memory (laid out at start) */
	memset(m, 0, sizeof(*m));
	m->pcm_out_ring = (uint8_t *)shm->audio.output_buffer;
	m->pcm_out_ring_bytes = sizeof(shm->audio.output_buffer);
	m->pcm_in_ring = (uint8_t *)shm->audio.input_buffer;
	m->pcm_in_ring_bytes = sizeof(shm->audio.input_buffer);
	m->pcm_out_slots = m->pcm_in_slots = 1;
}

uint32_t ozzy_engine_pcm_step(const struct ozzy_engine *e, enum ozzy_pcm_dir dir)
{
	const struct ozzy_engine_pipe *p = &e->pipe[dir == OZZY_PCM_OUT ? OZZY_PIPE_PCM_OUT : OZZY_PIPE_PCM_IN];

	if (!e->shm)
		return 0;
	return ozzy_pcm_step(p->transfer_type, p->max_packet_size, dir == OZZY_PCM_OUT ?
			     e->shm->audio.output_bytes_per_frame : e->shm->audio.input_bytes_per_frame);
}

enum ozzy_setting_change ozzy_engine_change_setting(struct ozzy_engine *e, unsigned int id, unsigned int value)
{
	if (id >= OZZY_NUM_SETTINGS || value >= ozzy_setting_descs[id].num_values)
		return OZZY_SETTING_INVALID;
	if (!ozzy_setting_value_fits(id, value, ozzy_engine_pcm_step(e, OZZY_PCM_OUT), ozzy_engine_pcm_step(e, OZZY_PCM_IN))) {
		ozzy_log("%s: %s isn't whole packets of the endpoint", ozzy_setting_descs[id].name,
			 ozzy_setting_descs[id].value_names[value]);
		return OZZY_SETTING_INVALID;
	}
	if (e->settings.value[id] == value)
		return OZZY_SETTING_SAME;
	ozzy_log("%s: %s", ozzy_setting_descs[id].name, ozzy_setting_descs[id].value_names[value]);
	ozzy_engine_set_setting(e, id, (uint8_t)value);
	return OZZY_SETTING_RESTART;
}

enum ozzy_setting_change ozzy_engine_change_rate(struct ozzy_engine *e, uint32_t rate)
{
	uint32_t current = e->shm ? e->shm->audio.sample_rate : 0;

	if (!ozzy_device_has_rate(e->model, rate)) {
		ozzy_log("%u Hz: not a rate the device has", rate);
		return OZZY_SETTING_INVALID;
	}
	if (rate == current)
		return OZZY_SETTING_SAME;
	ozzy_log("sample rate %u -> %u Hz", current, rate);
	ozzy_engine_set_rate(e, rate);
	return OZZY_SETTING_RESTART;
}

uint32_t ozzy_engine_pcm_layout(struct ozzy_engine *e, enum ozzy_pcm_dir dir, uint32_t bytes)
{
	struct ozzy_engine_memory *m = &e->mem;
	uint8_t *ring = dir == OZZY_PCM_OUT ? m->pcm_out_ring : m->pcm_in_ring;
	uint32_t ring_bytes = dir == OZZY_PCM_OUT ? m->pcm_out_ring_bytes : m->pcm_in_ring_bytes;
	uint32_t places;

	if (!bytes)
		return 0;
	if (ring)
		places = ring_bytes / bytes;
	else
		places = bytes <= OZZY_PCM_SLOT_MIN ? OZZY_PCM_OWN_SLOTS : 0;
	if (!places)
		return 0;
	if (dir == OZZY_PCM_OUT) {
		m->pcm_out_slots = places;
		m->pcm_out_stride = bytes;
	} else {
		m->pcm_in_slots = places;
		m->pcm_in_stride = bytes;
	}
	return places;
}

/* struct ozzy_info has one pipe per role */
typedef char ozzy_info_pipes_match[((int)OZZY_INFO_PIPES == (int)OZZY_PIPE_ROLES) ? 1 : -1];

/* --- The shared memory's stats (ozzy_stats.h) --- */

/* A completion later than this past the queued transfers' time is late: jitter, not a stop */
#define OZZY_STATS_JITTER_US 1000

static int stats_dir(const struct ozzy_engine *e, uint8_t pipe)
{
	if (pipe && pipe == e->pipe[OZZY_PIPE_PCM_OUT].address)
		return OZZY_PCM_OUT;
	if (pipe && pipe == e->pipe[OZZY_PIPE_PCM_IN].address)
		return OZZY_PCM_IN;
	return -1;
}

/* At a start: each direction's late gap, the transfers it keeps queued plus the jitter */
static void stats_start(struct ozzy_engine *e)
{
	static const enum ozzy_pipe_role roles[OZZY_PCM_DIRS] = { OZZY_PIPE_PCM_OUT, OZZY_PIPE_PCM_IN };
	uint64_t hz = ozzy_bus_time_hz(e->bus);
	uint32_t rate = e->shm->audio.sample_rate;
	unsigned int dir;

	for (dir = 0; dir < OZZY_PCM_DIRS; dir++) {
		uint64_t frames = (uint64_t)e->report.in_flight[roles[dir]] *
				  ozzy_engine_transfer_frames(e, (enum ozzy_pcm_dir)dir);

		e->stats_last[dir] = 0;
		e->stats_late[dir] = hz && rate && frames ?
			ozzy_div_u64(frames * hz, rate) + ozzy_div_u64(hz * OZZY_STATS_JITTER_US, 1000000) : 0;
	}
}

void ozzy_engine_stats_complete(struct ozzy_engine *e, uint8_t pipe, int status)
{
	int dir = stats_dir(e, pipe);
	volatile struct ozzy_stats_dir *s;
	bool incident = false;
	uint64_t now, hz;

	if (dir < 0 || !e->shm || !e->bus)
		return;
	if (status == OZZY_STATUS_ABORTED) {   /* a stop: what follows is no gap */
		e->stats_last[dir] = 0;
		return;
	}
	s = &e->shm->stats.dir[dir];
	now = ozzy_bus_time(e->bus);
	hz = ozzy_bus_time_hz(e->bus);
	if (e->stats_last[dir] && now > e->stats_last[dir] && hz) {
		uint64_t gap = now - e->stats_last[dir];
		uint64_t us = ozzy_div_u64(gap * 1000000, hz);

		if (us > s->max_gap_us)
			s->max_gap_us = us > 0xffffffffu ? 0xffffffffu : (uint32_t)us;
		if (e->stats_late[dir] && gap > e->stats_late[dir]) {
			s->late++;
			incident = true;
		}
	}
	e->stats_last[dir] = now;
	s->transfers++;
	if (status != 0) {
		s->errors++;
		incident = true;
	}
	if (incident)
		ozzy_bus_stats_changed(e->bus);
}

void ozzy_engine_stats_short(struct ozzy_engine *e, enum ozzy_pcm_dir dir)
{
	if (!e->shm)
		return;
	e->shm->stats.dir[dir].short_transfers++;
	ozzy_bus_stats_changed(e->bus);
}

void ozzy_engine_stats_recovery(struct ozzy_engine *e, bool streaming)
{
	if (!e->shm)
		return;
	e->shm->stats.recoveries++;
	if (!streaming)
		e->shm->stats.recovery_failures++;
	ozzy_bus_stats_changed(e->bus);
}

bool ozzy_engine_start(struct ozzy_engine *e)
{
	memset(&e->report, 0, sizeof(e->report));
	if (!e->ops->start(e))
		return false;
	e->starts++;
	ozzy_engine_publish_info(e);
	stats_start(e);
	return true;
}

void ozzy_engine_publish_info(struct ozzy_engine *e)
{
	volatile struct ozzy_shared_memory *shm = e->shm;
	const struct ozzy_device *model = e->model;
	const struct ozzy_engine_report *r = &e->report;
	struct ozzy_info info;
	unsigned int i;

	if (!shm)
		return;
	memset(&info, 0, sizeof(info));
	info.version = OZZY_INFO_VERSION;
	info.size = sizeof(info);
	info.starts = e->starts;
	info.vendor_id = model->vid;
	info.product_id = model->pid;
	info.family = model->family;
	info.status = model->status;
	info.usb_speed = e->usb_speed;
	info.interface_count = e->profile.interface_count < 4 ? e->profile.interface_count : 4;
	for (i = 0; i < info.interface_count; i++) {
		info.interfaces[i] = e->profile.interfaces[i];
		info.alt_settings[i] = e->profile.alt_settings[i];
	}
	info.output = r->output;
	info.firmware_valid = r->firmware_valid;
	info.firmware_chip = r->firmware_chip;
	memcpy(info.firmware, r->firmware, sizeof(info.firmware));
	info.input_selector_valid = r->input_selector_valid;
	info.input_selector = r->input_selector;

	info.sample_rate = shm->audio.sample_rate;
	info.boot_rate = r->boot_rate;
	info.device_rates = model->rates;
	info.in_channels = model->in_channels;
	info.out_channels = model->out_channels;
	info.transfer_frames[OZZY_PCM_OUT] = ozzy_engine_transfer_frames(e, OZZY_PCM_OUT);
	info.transfer_frames[OZZY_PCM_IN] = ozzy_engine_transfer_frames(e, OZZY_PCM_IN);
	info.out_bytes_per_frame = shm->audio.output_bytes_per_frame;
	info.in_bytes_per_frame = shm->audio.input_bytes_per_frame;
	info.safety_offset_frames = shm->audio.safety_offset_frames;
	info.input_safety_offset_frames = shm->audio.input_safety_offset_frames;
	if (e->driver_version)
		ozzy_strlcpy(info.driver_version, e->driver_version, sizeof(info.driver_version));
	for (i = 0; i < OZZY_PIPE_ROLES; i++) {
		info.pipes[i].address = e->pipe[i].address;
		info.pipes[i].transfer_type = e->pipe[i].transfer_type;
		info.pipes[i].interval = e->pipe[i].interval;
		info.pipes[i].max_packet = e->pipe[i].max_packet_size;
		info.pipes[i].in_flight = e->pipe[i].address ? r->in_flight[i] : 0;
	}
	memcpy((void *)&shm->info, &info, sizeof(info));
}

bool ozzy_engine_restart(struct ozzy_engine *e)
{
	ozzy_engine_suspend(e);
	if (ozzy_engine_start(e))
		return true;
	ozzy_log("restart failed");
	ozzy_engine_stop(e);
	return false;
}

void ozzy_engine_set_setting(struct ozzy_engine *e, unsigned int id, uint8_t value)
{
	if (id >= OZZY_NUM_SETTINGS)
		return;
	e->settings.value[id] = value;
}

void ozzy_engine_publish_settings(struct ozzy_engine *e)
{
	volatile struct ozzy_shared_memory *shm = e->shm;
	unsigned int i;

	if (!shm)
		return;
	for (i = 0; i < OZZY_NUM_SETTINGS; i++)
		shm->audio.settings.value[i] = e->settings.value[i];
}

const struct ozzy_pcm_ring *ozzy_engine_take_pcm_ring(struct ozzy_engine *e, enum ozzy_pcm_dir dir)
{
	const struct ozzy_pcm_ring *ring = (const struct ozzy_pcm_ring *)
		ozzy_load_acquire_ptr((const void *const volatile *)&e->pcm_ring_set[dir]);
	uint32_t channels = dir == OZZY_PCM_OUT ? e->model->out_channels : e->model->in_channels;

	if (ring != e->pcm_ring[dir]) {
		e->pcm_ring[dir] = ring;
		if (ring)
			e->pcm_pos[dir] = ring->start;
	}
	return ring && ring->frames && ring->channels == channels ? ring : NULL;
}

void ozzy_engine_bind_endpoints(struct ozzy_engine *e, const struct ozzy_usb_endpoint *eps, unsigned int count,
				const struct ozzy_usb_endpoint *bound[OZZY_PIPE_ROLES])
{
	static const char *const types[] = { "control", "isoc", "bulk", "interrupt" };
	unsigned int role, i;

	for (i = 0; i < count; i++)
		ozzy_log("  interface %u: EP 0x%02x %-3s %-9s wMaxPacketSize %u x%u bInterval %u", eps[i].intf,
			 eps[i].address, (eps[i].address & 0x80) ? "IN" : "OUT", types[eps[i].attributes & 3],
			 eps[i].max_packet & 0x07FF, ((eps[i].max_packet >> 11) & 3) + 1, eps[i].interval);

	for (role = 0; role < OZZY_PIPE_ROLES; role++) {
		const struct ozzy_ep *spec = &e->profile.pipes[role];
		const struct ozzy_usb_endpoint *ep = NULL;

		bound[role] = NULL;
		if (!spec->address && !spec->types)
			continue;
		for (i = 0; i < count && !ep; i++) {
			if (eps[i].intf != spec->intf)
				continue;
			if (spec->address ? eps[i].address == spec->address :
					    ozzy_ep_matches(spec, eps[i].address, eps[i].attributes))
				ep = &eps[i];
		}
		if (!ep) {
			ozzy_log("  no endpoint for pipe role %u on interface %u", role, spec->intf);
			continue;
		}
		bound[role] = ep;
		ozzy_engine_set_endpoint(e, (enum ozzy_pipe_role)role, ep->address, ep->attributes & 3,
					 ozzy_usb_endpoint_bytes(ep), ep->interval);
	}
}

static uint8_t *alloc_zeroed(const struct ozzy_allocator *a, size_t size, bool *ok)
{
	uint8_t *p = (uint8_t *)a->alloc(a->ctx, size);

	if (p)
		memset(p, 0, size);
	else
		*ok = false;
	return p;
}

bool ozzy_engine_alloc_memory(struct ozzy_engine *e, const struct ozzy_allocator *a)
{
	struct ozzy_engine_memory *m = &e->mem;
	bool ok = true;
	unsigned int i;

	ozzy_engine_free_memory(e);
	memset(m, 0, sizeof(*m));
	e->allocator = a;
	for (i = 0; i < OZZY_PCM_OWN_SLOTS; i++) {
		m->pcm_out[i] = alloc_zeroed(a, OZZY_PCM_SLOT_MIN, &ok);
		m->pcm_in[i] = alloc_zeroed(a, OZZY_PCM_SLOT_MIN, &ok);
	}
	m->pcm_out_slots = m->pcm_in_slots = OZZY_PCM_OWN_SLOTS;
	if (!ok)
		ozzy_engine_free_memory(e);
	return ok;
}

void ozzy_engine_free_memory(struct ozzy_engine *e)
{
	const struct ozzy_allocator *a = e->allocator;
	struct ozzy_engine_memory *m = &e->mem;
	unsigned int i;

	if (!a)
		return;
	for (i = 0; i < OZZY_PCM_OWN_SLOTS; i++) {
		if (m->pcm_out[i])
			a->free(a->ctx, m->pcm_out[i]);
		if (m->pcm_in[i])
			a->free(a->ctx, m->pcm_in[i]);
	}
	memset(m, 0, sizeof(*m));
	e->allocator = NULL;
}
