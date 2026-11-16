// SPDX-License-Identifier: MIT
//! The PipeWire side of the bridge: the `a16` card with its profiles, the `a16.playback`
//! sink and the `a16.capture` source. The card and the streams live on PipeWire's thread
//! loop; the streams' process callbacks run on its realtime data thread, where they make
//! only bounded copies through the queues and touch only atomics.
//!
//! The sink is a driver: when it drives its graph, a timer on the data thread starts each
//! cycle, filling the graph's clock and triggering the graph, at the A16's rate as
//! [`Cycles`] follows it. The source is in the sink's node group, so it follows that graph.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};

use pipewire::context::ContextRc;
use pipewire::core::CoreRc;
use pipewire::metadata::{Metadata, MetadataListener};
use pipewire::registry::{self, RegistryRc};
use pipewire::spa::sys as spa;
use pipewire::sys as pw;
use pipewire::thread_loop::ThreadLoopRc;
use pipewire::types::ObjectType;

use crate::bridge::{DEFAULT_QUANTUM, RATE, Shared};
use crate::graph_clock::Cycles;
use crate::live;

/// One stream and what its callbacks reach.
struct Endpoint {
    stream: *mut pw::pw_stream,
    listener: spa::spa_hook,
    shared: Arc<Shared>,
    source: bool,
    channels: u32,
    /// The capture queue has filled to its target since it last ran dry (source only, on
    /// the data thread).
    capture_primed: bool,
    /// The graph's position, as PipeWire last set it.
    position: AtomicPtr<spa::spa_io_position>,
    /// The data loop the stream runs on.
    data_loop: *mut pw::pw_loop,
    /// The sink's cycle timer, on the data loop; null for the source.
    timer: *mut spa::spa_source,
    /// When the sink's cycles start (on the data thread).
    cycles: Cycles,
}

/// `priority.driver` of the sink. Of the drivers in one graph, the one with the highest
/// priority drives it.
const DRIVER_PRIORITY: &str = "1500";
/// The setting, on subject 0 of the `settings` metadata, that asks for a queue target.
const QUEUE_TARGET_KEY: &str = "a16.queue-target";

/// `CLOCK_MONOTONIC`, in nanoseconds.
fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid timespec to fill
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Run `f` with `endpoint` on the endpoint's data loop and wait for it.
///
/// # Safety
///
/// The endpoint and its data loop must be live; the caller must not be holding anything
/// the data loop waits for.
unsafe fn on_data_loop(
    endpoint: *mut Endpoint,
    f: unsafe extern "C" fn(
        *mut spa::spa_loop,
        bool,
        u32,
        *const c_void,
        usize,
        *mut c_void,
    ) -> c_int,
) {
    // SAFETY: the caller's live endpoint and loop
    unsafe {
        let data_loop = (*endpoint).data_loop;
        spa::spa_loop_invoke(
            (*data_loop).loop_.cast(),
            Some(f),
            0,
            ptr::null(),
            0,
            true,
            endpoint.cast(),
        );
    }
}

/// Arm the sink's timer to fire at `nsec` on `CLOCK_MONOTONIC`, or disarm it with 0.
///
/// # Safety
///
/// On the data loop, with the endpoint's timer live.
unsafe fn set_timer(e: &Endpoint, nsec: u64) {
    let mut value = libc::timespec {
        tv_sec: (nsec / 1_000_000_000) as libc::time_t,
        tv_nsec: (nsec % 1_000_000_000) as libc::c_long,
    };
    let mut interval = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: the caller's live timer on its loop
    unsafe {
        spa::spa_loop_utils_update_timer(
            (*e.data_loop).utils.cast(),
            e.timer,
            ptr::addr_of_mut!(value).cast(),
            ptr::addr_of_mut!(interval).cast(),
            true,
        );
    }
}

unsafe extern "C" fn add_timer(
    _loop: *mut spa::spa_loop,
    _async: bool,
    _seq: u32,
    _data: *const c_void,
    _size: usize,
    user_data: *mut c_void,
) -> c_int {
    // SAFETY: on the data loop, with the endpoint on_data_loop passes
    unsafe {
        let e = &mut *user_data.cast::<Endpoint>();
        e.timer =
            spa::spa_loop_utils_add_timer((*e.data_loop).utils.cast(), Some(cycle), user_data);
    }
    0
}

unsafe extern "C" fn remove_timer(
    _loop: *mut spa::spa_loop,
    _async: bool,
    _seq: u32,
    _data: *const c_void,
    _size: usize,
    user_data: *mut c_void,
) -> c_int {
    // SAFETY: on the data loop, with the endpoint on_data_loop passes
    unsafe {
        let e = &mut *user_data.cast::<Endpoint>();
        if !e.timer.is_null() {
            spa::spa_loop_utils_destroy_source((*e.data_loop).utils.cast(), e.timer);
            e.timer = ptr::null_mut();
        }
    }
    0
}

unsafe extern "C" fn start_timer(
    _loop: *mut spa::spa_loop,
    _async: bool,
    _seq: u32,
    _data: *const c_void,
    _size: usize,
    user_data: *mut c_void,
) -> c_int {
    // SAFETY: on the data loop, with the endpoint on_data_loop passes
    unsafe {
        let e = &mut *user_data.cast::<Endpoint>();
        if !e.timer.is_null() {
            let now = monotonic_ns();
            e.cycles.start(now);
            set_timer(e, now);
        }
    }
    0
}

unsafe extern "C" fn stop_timer(
    _loop: *mut spa::spa_loop,
    _async: bool,
    _seq: u32,
    _data: *const c_void,
    _size: usize,
    user_data: *mut c_void,
) -> c_int {
    // SAFETY: on the data loop, with the endpoint on_data_loop passes
    unsafe {
        let e = &mut *user_data.cast::<Endpoint>();
        if !e.timer.is_null() {
            set_timer(e, 0);
        }
    }
    0
}

/// The sink's timer, on the data thread: start a cycle. While the sink drives its graph,
/// fill the graph's clock and trigger the graph; while it follows another driver, only
/// keep the schedule.
unsafe extern "C" fn cycle(data: *mut c_void, _expirations: u64) {
    // SAFETY: data is the sink's Endpoint, which outlives its timer. PipeWire's position is
    // live while set, and the driver alone writes its clock, on this thread
    unsafe {
        let e = &mut *data.cast::<Endpoint>();
        let now = monotonic_ns();
        let driving = pw::pw_stream_is_driving(e.stream);
        let position = e.position.load(Ordering::Acquire);
        let (duration, rate) = if position.is_null() {
            (u64::from(DEFAULT_QUANTUM), RATE)
        } else {
            let clock = &(*position).clock;
            (
                if clock.target_duration == 0 {
                    u64::from(DEFAULT_QUANTUM)
                } else {
                    clock.target_duration
                },
                if clock.target_rate.denom == 0 {
                    RATE
                } else {
                    clock.target_rate.denom
                },
            )
        };
        let c = e
            .cycles
            .wake(now, duration, rate, e.shared.a16_clock.read());
        let shared = &e.shared;
        shared
            .graph_rate_diff
            .store(c.rate_diff.to_bits(), Ordering::Relaxed);
        shared.graph_locked.store(c.locked, Ordering::Relaxed);
        shared.graph_quantum.store(
            u32::try_from(duration).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        set_timer(e, c.next_nsec);
        if !driving {
            return;
        }
        if !position.is_null() {
            let clock = &mut (*position).clock;
            clock.nsec = c.nsec;
            clock.rate = spa::spa_fraction {
                num: 1,
                denom: rate,
            };
            clock.position += clock.duration;
            clock.duration = duration;
            let queued = u64::from(shared.sink.fifo.available());
            clock.delay = (queued * u64::from(rate) / u64::from(RATE)) as i64;
            clock.rate_diff = c.rate_diff;
            clock.next_nsec = c.next_nsec;
        }
        pw::pw_stream_trigger_process(e.stream);
    }
}

unsafe extern "C" fn io_changed(data: *mut c_void, id: u32, area: *mut c_void, _size: u32) {
    // SAFETY: data is the Endpoint the listener was added with
    let e = unsafe { &*data.cast::<Endpoint>() };
    if id == spa::SPA_IO_Position {
        e.position.store(area.cast(), Ordering::Release);
    }
}

/// The card, on PipeWire's loop thread.
struct Nodes {
    shared: Arc<Shared>,
    missing_text: CString,
    core: *mut pw::pw_core,
    device: spa::spa_device,
    device_methods: spa::spa_device_methods,
    device_hooks: spa::spa_hook_list,
    device_params: [spa::spa_param_info; 2],
    device_items: [spa::spa_dict_item; 5],
    device_props: spa::spa_dict,
    device_proxy: *mut pw::pw_proxy,
    proxy_events: pw::pw_proxy_events,
    proxy_listener: spa::spa_hook,
    stream_events: pw::pw_stream_events,
    device_id: u32,
    profile: usize,
    profile_save: bool,
    sink: Option<Box<Endpoint>>,
    source: Option<Box<Endpoint>>,
    /// The data loop the streams run on, and its name.
    data_loop: *mut pw::pw_loop,
    data_loop_name: String,
}

const DEVICE_PROPS: [(&CStr, &CStr); 5] = [
    (c"device.name", c"a16"),
    (c"device.description", c"Smyth Realiser A16 USB"),
    (c"device.api", c"a16"),
    (c"media.class", c"Audio/Device"),
    (c"device.icon-name", c"audio-card"),
];

fn cstr(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

/// The device's hooks, each with its events and data, for an emit.
///
/// # Safety
///
/// `hooks` must be a live hook list.
unsafe fn device_listeners(
    hooks: *mut spa::spa_hook_list,
) -> Vec<(*const spa::spa_device_events, *mut c_void)> {
    let mut out = Vec::new();
    // SAFETY: the caller's live list; each link is the first field of its spa_hook
    unsafe {
        let head = ptr::addr_of_mut!((*hooks).list);
        let mut link = (*head).next;
        while link != head {
            let hook = link.cast::<spa::spa_hook>();
            out.push((
                (*hook).cb.funcs.cast::<spa::spa_device_events>(),
                (*hook).cb.data,
            ));
            link = (*link).next;
        }
    }
    out
}

impl Nodes {
    fn info(&mut self) -> spa::spa_device_info {
        spa::spa_device_info {
            version: spa::SPA_VERSION_DEVICE_INFO,
            change_mask: u64::from(spa::SPA_DEVICE_CHANGE_MASK_PROPS)
                | u64::from(spa::SPA_DEVICE_CHANGE_MASK_PARAMS),
            flags: 0,
            props: &self.device_props,
            params: self.device_params.as_mut_ptr(),
            n_params: 2,
        }
    }

    fn emit_info(&mut self, info: &spa::spa_device_info) {
        // SAFETY: the hooks are those the session manager added, valid while listed
        for (events, data) in unsafe { device_listeners(&mut self.device_hooks) } {
            unsafe {
                if let Some(f) = (*events).info {
                    f(data, info);
                }
            }
        }
    }

    fn emit_result(&mut self, seq: c_int, res: c_int, type_: u32, result: *const c_void) {
        // SAFETY: as emit_info
        for (events, data) in unsafe { device_listeners(&mut self.device_hooks) } {
            unsafe {
                if let Some(f) = (*events).result {
                    f(data, seq, res, type_, result);
                }
            }
        }
    }

    /// Make and connect the sink (`source` false) or the source.
    fn make_stream(&mut self, source: bool) -> bool {
        let shared = Arc::clone(&self.shared);
        let profile = &shared.effective[self.profile];
        let channels = if source { 2 } else { profile.channels() as u32 };
        let description = if source {
            "Realiser A16 USB Return".to_owned()
        } else {
            format!("Realiser A16 USB {}", profile.name)
        };
        let positions = if source {
            "[ FL FR ]"
        } else {
            profile.positions.as_str()
        };
        let category = if profile.custom {
            "custom"
        } else if profile.advanced {
            "advanced"
        } else {
            "compatibility"
        };
        let mut items: Vec<(&CStr, String)> = vec![
            (
                c"node.name",
                if source {
                    "a16.capture"
                } else {
                    "a16.playback"
                }
                .to_owned(),
            ),
            (c"node.description", description),
            (c"device.id", self.device_id.to_string()),
            (
                c"card.profile.device",
                if source { "1" } else { "0" }.to_owned(),
            ),
            (
                c"media.class",
                if source { "Audio/Source" } else { "Audio/Sink" }.to_owned(),
            ),
            (c"media.type", "Audio".to_owned()),
            (
                c"media.category",
                if source { "Capture" } else { "Playback" }.to_owned(),
            ),
            (c"node.virtual", "true".to_owned()),
            (c"node.pause-on-idle", "true".to_owned()),
            (c"node.rate", "1/48000".to_owned()),
            (c"node.latency", "1024/48000".to_owned()),
            // The queues swing half a quantum around their 2048-frame target: a 2048-frame
            // quantum leaves about 1024 frames in hand, and a 4096-frame one would run them dry
            (c"node.max-latency", "2048/48000".to_owned()),
            (c"priority.session", "0".to_owned()),
            (c"node.autoconnect", "false".to_owned()),
            (c"audio.position", positions.to_owned()),
            (c"audio.rate", "48000".to_owned()),
            (c"audio.channels", channels.to_string()),
            (c"stream.dont-remix", "true".to_owned()),
            (c"node.group", "a16".to_owned()),
            (c"resample.quality", "4".to_owned()),
            (c"node.loop.name", self.data_loop_name.clone()),
        ];
        if !source {
            items.extend([
                (c"priority.driver", DRIVER_PRIORITY.to_owned()),
                (c"a16.profile", profile.name.to_owned()),
                (c"a16.room.required", profile.room.to_owned()),
                (c"a16.speakers", profile.speakers.clone()),
                (
                    c"a16.missing-speakers",
                    self.missing_text.to_string_lossy().into_owned(),
                ),
                (c"a16.profile.category", category.to_owned()),
                (
                    c"a16.profile.pulse-compatible",
                    profile.pulse_compatible.to_string(),
                ),
                (c"a16.profile.advanced", profile.advanced.to_string()),
            ]);
        }
        let mut endpoint = Box::new(Endpoint {
            stream: ptr::null_mut(),
            // SAFETY: a zeroed hook is an unlinked one
            listener: unsafe { std::mem::zeroed() },
            shared: Arc::clone(&shared),
            source,
            channels,
            capture_primed: false,
            position: AtomicPtr::new(ptr::null_mut()),
            data_loop: self.data_loop,
            timer: ptr::null_mut(),
            cycles: Cycles::default(),
        });
        let name = if source {
            c"A16 return"
        } else {
            c"A16 playback"
        };
        // SAFETY: the core is connected; the properties go to the stream; the endpoint is
        // boxed, so the listener and the data pointer stay put until the stream is destroyed
        unsafe {
            let props = pw::pw_properties_new(ptr::null());
            for (key, value) in &items {
                pw::pw_properties_set(props, key.as_ptr(), cstr(value).as_ptr());
            }
            endpoint.stream = pw::pw_stream_new(self.core, name.as_ptr(), props);
            if endpoint.stream.is_null() {
                return false;
            }
            let data = ptr::addr_of_mut!(*endpoint).cast::<c_void>();
            if !source {
                on_data_loop(ptr::addr_of_mut!(*endpoint), add_timer);
                if endpoint.timer.is_null() {
                    pw::pw_stream_destroy(endpoint.stream);
                    return false;
                }
            }
            pw::pw_stream_add_listener(
                endpoint.stream,
                &mut endpoint.listener,
                &self.stream_events,
                data,
            );
            let mut info: spa::spa_audio_info_raw = std::mem::zeroed();
            info.format = spa::SPA_AUDIO_FORMAT_F32_LE;
            info.rate = RATE;
            info.channels = channels;
            for c in 0..channels as usize {
                info.position[c] = if source {
                    if c == 0 {
                        spa::SPA_AUDIO_CHANNEL_FL
                    } else {
                        spa::SPA_AUDIO_CHANNEL_FR
                    }
                } else {
                    profile.position[c]
                };
            }
            let mut buffer = [0u8; 1024];
            let mut builder: spa::spa_pod_builder = std::mem::zeroed();
            spa::spa_pod_builder_init(
                &mut builder,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
            );
            let mut param =
                spa::spa_format_audio_raw_build(&mut builder, spa::SPA_PARAM_EnumFormat, &info)
                    .cast_const();
            let direction = if source {
                spa::SPA_DIRECTION_OUTPUT
            } else {
                spa::SPA_DIRECTION_INPUT
            };
            let mut flags = pw::pw_stream_flags_PW_STREAM_FLAG_MAP_BUFFERS
                | pw::pw_stream_flags_PW_STREAM_FLAG_RT_PROCESS;
            if !source {
                flags |= pw::pw_stream_flags_PW_STREAM_FLAG_DRIVER;
            }
            let connected = pw::pw_stream_connect(
                endpoint.stream,
                direction,
                pw::PW_ID_ANY,
                flags,
                &mut param,
                1,
            ) >= 0;
            if source {
                self.source = Some(endpoint);
            } else {
                self.sink = Some(endpoint);
            }
            connected
        }
    }

    fn destroy_sink(&mut self) {
        if let Some(mut endpoint) = self.sink.take() {
            // SAFETY: the timer and the stream are live; removing the timer on the data loop
            // ends its callbacks, and destroying the stream stops its data callback first
            unsafe {
                on_data_loop(ptr::addr_of_mut!(*endpoint), remove_timer);
                pw::pw_stream_destroy(endpoint.stream);
            }
        }
    }

    /// The profile pod of `selected`, for EnumProfile (`id`) or Profile.
    ///
    /// # Safety
    ///
    /// `b` must be a builder over a buffer the pod fits in.
    unsafe fn profile_pod(
        &self,
        b: &mut spa::spa_pod_builder,
        id: u32,
        selected: usize,
    ) -> *mut spa::spa_pod {
        let p = &self.shared.effective[selected];
        let name = cstr(p.name);
        let description = cstr(&p.description);
        // SAFETY: the caller's builder
        unsafe {
            let mut object: spa::spa_pod_frame = std::mem::zeroed();
            let mut classes: spa::spa_pod_frame = std::mem::zeroed();
            let mut class: spa::spa_pod_frame = std::mem::zeroed();
            spa::spa_pod_builder_push_object(b, &mut object, spa::SPA_TYPE_OBJECT_ParamProfile, id);
            spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_index, 0);
            spa::spa_pod_builder_int(b, selected as i32);
            spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_name, 0);
            spa::spa_pod_builder_string(b, name.as_ptr());
            spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_description, 0);
            spa::spa_pod_builder_string(b, description.as_ptr());
            spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_priority, 0);
            spa::spa_pod_builder_int(b, if selected == self.profile { 100 } else { 50 });
            spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_available, 0);
            spa::spa_pod_builder_id(
                b,
                if p.available {
                    spa::SPA_PARAM_AVAILABILITY_yes
                } else {
                    spa::SPA_PARAM_AVAILABILITY_no
                },
            );
            spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_classes, 0);
            spa::spa_pod_builder_push_struct(b, &mut classes);
            spa::spa_pod_builder_int(b, if p.available { 2 } else { 0 });
            if p.available {
                for (media_class, device) in [(c"Audio/Sink", 0u32), (c"Audio/Source", 1u32)] {
                    spa::spa_pod_builder_push_struct(b, &mut class);
                    spa::spa_pod_builder_string(b, media_class.as_ptr());
                    spa::spa_pod_builder_int(b, 1);
                    spa::spa_pod_builder_string(b, c"card.profile.devices".as_ptr());
                    spa::spa_pod_builder_array(
                        b,
                        size_of::<u32>() as u32,
                        spa::SPA_TYPE_Int,
                        1,
                        ptr::from_ref(&device).cast(),
                    );
                    spa::spa_pod_builder_pop(b, &mut class);
                }
            }
            spa::spa_pod_builder_pop(b, &mut classes);
            if id == spa::SPA_PARAM_Profile {
                spa::spa_pod_builder_prop(b, spa::SPA_PARAM_PROFILE_save, 0);
                spa::spa_pod_builder_bool(b, self.profile_save);
            }
            spa::spa_pod_builder_pop(b, &mut object).cast()
        }
    }
}

/// The value of property `key` of `object`, past a choice of none; null without one.
///
/// # Safety
///
/// `object` must be a valid object pod.
unsafe fn prop_value(object: *const spa::spa_pod_object, key: u32) -> *const spa::spa_pod {
    // SAFETY: the caller's object
    unsafe {
        let prop = spa::spa_pod_object_find_prop(object, ptr::null(), key);
        if prop.is_null() {
            return ptr::null();
        }
        let mut value: *const spa::spa_pod = &(*prop).value;
        if (*value).type_ == spa::SPA_TYPE_Choice {
            let mut n = 0;
            let mut choice = 0;
            value = spa::spa_pod_get_values(value, &mut n, &mut choice);
            if choice != spa::SPA_CHOICE_None {
                return ptr::null();
            }
        }
        value
    }
}

unsafe extern "C" fn device_add_listener(
    data: *mut c_void,
    listener: *mut spa::spa_hook,
    events: *const spa::spa_device_events,
    userdata: *mut c_void,
) -> c_int {
    // SAFETY: data is the Nodes the device was made with; PipeWire passes a live hook
    unsafe {
        let nodes = &mut *data.cast::<Nodes>();
        spa::spa_hook_list_append(&mut nodes.device_hooks, listener, events.cast(), userdata);
        let info = nodes.info();
        if let Some(f) = (*events).info {
            f(userdata, &info);
        }
    }
    0
}

unsafe extern "C" fn device_sync(data: *mut c_void, seq: c_int) -> c_int {
    // SAFETY: data is the Nodes the device was made with
    let nodes = unsafe { &mut *data.cast::<Nodes>() };
    nodes.emit_result(seq, 0, 0, ptr::null());
    0
}

unsafe extern "C" fn device_enum_params(
    data: *mut c_void,
    seq: c_int,
    id: u32,
    start: u32,
    max: u32,
    filter: *const spa::spa_pod,
) -> c_int {
    // SAFETY: data is the Nodes the device was made with
    let nodes = unsafe { &mut *data.cast::<Nodes>() };
    if id != spa::SPA_PARAM_EnumProfile && id != spa::SPA_PARAM_Profile {
        return -libc::ENOENT;
    }
    let count = if id == spa::SPA_PARAM_EnumProfile {
        nodes.shared.effective.len() as u32
    } else {
        1
    };
    let mut index = start;
    let mut emitted = 0;
    while index < count && emitted < max {
        let selected = if id == spa::SPA_PARAM_Profile {
            nodes.profile
        } else {
            index as usize
        };
        let mut storage = [0u8; 2048];
        let mut filtered = [0u8; 2048];
        // SAFETY: builders over the local buffers, which the pods fit in
        unsafe {
            let mut b: spa::spa_pod_builder = std::mem::zeroed();
            spa::spa_pod_builder_init(&mut b, storage.as_mut_ptr().cast(), storage.len() as u32);
            let param = nodes.profile_pod(&mut b, id, selected);
            let mut fb: spa::spa_pod_builder = std::mem::zeroed();
            spa::spa_pod_builder_init(&mut fb, filtered.as_mut_ptr().cast(), filtered.len() as u32);
            let mut result: *mut spa::spa_pod = ptr::null_mut();
            if spa::spa_pod_filter(&mut fb, &mut result, param, filter) >= 0 {
                let reply = spa::spa_result_device_params {
                    id,
                    index,
                    next: index + 1,
                    param: result,
                };
                nodes.emit_result(
                    seq,
                    0,
                    spa::SPA_RESULT_TYPE_DEVICE_PARAMS,
                    ptr::from_ref(&reply).cast(),
                );
                emitted += 1;
            }
        }
        index += 1;
    }
    0
}

unsafe extern "C" fn device_set_param(
    data: *mut c_void,
    id: u32,
    _flags: u32,
    param: *const spa::spa_pod,
) -> c_int {
    // SAFETY: data is the Nodes the device was made with
    let nodes = unsafe { &mut *data.cast::<Nodes>() };
    if id != spa::SPA_PARAM_Profile {
        return -libc::ENOENT;
    }
    let mut selected: i32 = -1;
    let mut save = false;
    // SAFETY: PipeWire passes a valid pod, or null
    let parsed = !param.is_null()
        && unsafe {
            spa::spa_pod_is_object_type(param, spa::SPA_TYPE_OBJECT_ParamProfile) && {
                let object = param.cast::<spa::spa_pod_object>();
                let index = prop_value(object, spa::SPA_PARAM_PROFILE_index);
                let ok = !index.is_null() && spa::spa_pod_get_int(index, &mut selected) >= 0;
                let value = prop_value(object, spa::SPA_PARAM_PROFILE_save);
                if ok && !value.is_null() {
                    let mut v = false;
                    if spa::spa_pod_get_bool(value, &mut v) >= 0 {
                        save = v;
                    }
                }
                ok
            }
        };
    if !parsed || selected < 0 || selected as usize >= nodes.shared.effective.len() {
        return -libc::EINVAL;
    }
    let selected = selected as usize;
    if !nodes.shared.effective[selected].available {
        return -libc::ENOTSUP;
    }
    if selected != nodes.profile {
        // Destroying the stream synchronizes its data callback before the endpoint goes.
        // Only USB consumes or resets the playback queue, also on a profile change.
        nodes.destroy_sink();
        nodes.shared.sink.active.store(false, Ordering::SeqCst);
        nodes.shared.sink.reset.store(true, Ordering::SeqCst);
        nodes.profile = selected;
        nodes.shared.profile.store(selected, Ordering::SeqCst);
        if nodes.device_id != spa::SPA_ID_INVALID && !nodes.make_stream(false) {
            nodes.shared.failed.store(true, Ordering::SeqCst);
            return -libc::EIO;
        }
        let p = &nodes.shared.effective[selected];
        eprintln!(
            "A16 profile: {} ({} graph channels, 16 USB channels)",
            p.name,
            p.channels()
        );
    }
    nodes.profile_save = save;
    nodes.device_params[1].flags ^= spa::SPA_PARAM_INFO_SERIAL;
    let mut info = nodes.info();
    info.change_mask = u64::from(spa::SPA_DEVICE_CHANGE_MASK_PARAMS);
    nodes.emit_info(&info);
    0
}

unsafe extern "C" fn device_bound(data: *mut c_void, id: u32) {
    // SAFETY: data is the Nodes the proxy listener was added with
    let nodes = unsafe { &mut *data.cast::<Nodes>() };
    nodes.device_id = id;
    if !nodes.make_stream(false) || !nodes.make_stream(true) {
        nodes.shared.failed.store(true, Ordering::SeqCst);
    }
    eprintln!(
        "A16 card published: id={id} profile={}",
        nodes.shared.effective[nodes.profile].name
    );
}

unsafe extern "C" fn state_changed(
    data: *mut c_void,
    _old: pw::pw_stream_state,
    state: pw::pw_stream_state,
    error: *const c_char,
) {
    // SAFETY: data is the Endpoint the listener was added with; PipeWire's strings. The
    // data loop never waits for the thread loop this runs on
    unsafe {
        let streaming = state == pw::pw_stream_state_PW_STREAM_STATE_STREAMING;
        if !(*data.cast::<Endpoint>()).source {
            on_data_loop(
                data.cast(),
                if streaming { start_timer } else { stop_timer },
            );
        }
        let e = &*data.cast::<Endpoint>();
        if streaming && e.source {
            e.shared.source.reset.store(true, Ordering::SeqCst);
        }
        let side = if e.source {
            &e.shared.source
        } else {
            &e.shared.sink
        };
        side.active.store(streaming, Ordering::SeqCst);
        let name = CStr::from_ptr(pw::pw_stream_state_as_string(state)).to_string_lossy();
        let error = if error.is_null() {
            String::new()
        } else {
            format!(": {}", CStr::from_ptr(error).to_string_lossy())
        };
        eprintln!(
            "{}: {name}{error}",
            if e.source {
                "A16 return"
            } else {
                "A16 playback"
            }
        );
        if state == pw::pw_stream_state_PW_STREAM_STATE_ERROR {
            e.shared.failed.store(true, Ordering::SeqCst);
        }
    }
}

/// On PipeWire's realtime data thread: bounded copies and atomics only.
unsafe extern "C" fn process(data: *mut c_void) {
    // SAFETY: data is the Endpoint the listener was added with. PipeWire maps the buffer
    // and its data for the length its chunk and maxsize give
    unsafe {
        let e = &mut *data.cast::<Endpoint>();
        let b = pw::pw_stream_dequeue_buffer(e.stream);
        if b.is_null() {
            return;
        }
        let buf = (*b).buffer;
        if (*buf).n_datas != 1 || (*(*buf).datas).data.is_null() || (*(*buf).datas).chunk.is_null()
        {
            e.shared.failed.store(true, Ordering::SeqCst);
            pw::pw_stream_queue_buffer(e.stream, b);
            return;
        }
        let d = &mut *(*buf).datas;
        let chunk = &mut *d.chunk;
        let stride = e.channels * 4;
        let shared = Arc::clone(&e.shared);
        let n;
        if e.source {
            let side = &shared.source;
            if side.reset.swap(false, Ordering::SeqCst) {
                side.fifo.discard();
                e.capture_primed = false;
            }
            let mut frames = d.maxsize / stride;
            if (*b).requested != 0 && (*b).requested < u64::from(frames) {
                frames = (*b).requested as u32;
            }
            n = frames;
            let out = std::slice::from_raw_parts_mut(d.data.cast::<f32>(), n as usize * 2);
            let available = side.fifo.available();
            let mut got = 0;
            if !e.capture_primed && shared.capture_ready(available) {
                e.capture_primed = true;
            }
            if e.capture_primed {
                got = side.fifo.peek(out);
                side.fifo.consume(got);
                if got < n {
                    side.underruns.fetch_add(1, Ordering::SeqCst);
                    e.capture_primed = false;
                }
            }
            out[got as usize * 2..].fill(0.0);
            chunk.offset = 0;
            chunk.stride = stride as i32;
            chunk.size = n * stride;
            (*b).size = u64::from(n);
        } else {
            let offset = chunk.offset;
            if offset > d.maxsize
                || chunk.size > d.maxsize - offset
                || (chunk.stride != 0 && chunk.stride != stride as i32)
                || !chunk.size.is_multiple_of(stride)
            {
                shared.failed.store(true, Ordering::SeqCst);
                pw::pw_stream_queue_buffer(e.stream, b);
                return;
            }
            n = chunk.size / stride;
            let profile = shared.profile();
            let channels = e.channels as usize;
            let input = std::slice::from_raw_parts(
                d.data.cast::<u8>().add(offset as usize).cast::<f32>(),
                n as usize * channels,
            );
            let mut got = 0;
            let mut expanded = [0.0f32; 128 * live::CHANNELS];
            while got < n {
                let block = (n - got).min(128);
                for i in 0..block {
                    let frame = (got + i) as usize * channels;
                    profile.expand(
                        &mut expanded[i as usize * 16..],
                        &input[frame..frame + channels],
                    );
                }
                let written = shared.sink.fifo.push(&expanded[..block as usize * 16]);
                got += written;
                if written < block {
                    break;
                }
            }
            if got < n {
                shared.sink.overruns.fetch_add(1, Ordering::SeqCst);
            }
        }
        let side = if e.source {
            &shared.source
        } else {
            &shared.sink
        };
        side.frames.fetch_add(u64::from(n), Ordering::SeqCst);
        pw::pw_stream_queue_buffer(e.stream, b);
    }
}

/// The `settings` metadata, bound, and the listener for its properties.
struct Settings {
    id: u32,
    _metadata: Metadata,
    _listener: MetadataListener,
}

/// Take a value of the `a16.queue-target` setting as the queue target request.
fn request_queue_target(shared: &Shared, value: Option<&str>) {
    match live::parse_queue_target(value) {
        Some(frames) => {
            shared.queue_request.store(frames, Ordering::Relaxed);
            if frames == 0 {
                eprintln!("A16 queue target requested: default");
            } else {
                eprintln!("A16 queue target requested: {frames} frames");
            }
        }
        None => eprintln!(
            "A16 ignores {QUEUE_TARGET_KEY} {:?}: not a number of frames",
            value.unwrap_or("")
        ),
    }
}

/// Bind the `settings` metadata of `global`, if it is that, and follow its
/// `a16.queue-target` on subject 0; binding it reports the value it holds.
fn watch_settings(
    registry: &RegistryRc,
    global: &registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    shared: &Arc<Shared>,
) -> Option<Settings> {
    if global.type_ != ObjectType::Metadata
        || global.props.and_then(|p| p.get("metadata.name")) != Some("settings")
    {
        return None;
    }
    let metadata: Metadata = registry.bind(global).ok()?;
    let shared = Arc::clone(shared);
    let listener = metadata
        .add_listener_local()
        .property(move |subject, key, _type, value| {
            if subject == 0 {
                match key {
                    Some(QUEUE_TARGET_KEY) => request_queue_target(&shared, value),
                    // Every property removed
                    None => request_queue_target(&shared, None),
                    Some(_) => {}
                }
            }
            0
        })
        .register();
    Some(Settings {
        id: global.id,
        _metadata: metadata,
        _listener: listener,
    })
}

/// The connection to PipeWire, its thread loop and the published card.
pub struct PipeWire {
    nodes: Box<Nodes>,
    core_listener: Option<pipewire::core::Listener>,
    /// The `settings` metadata, once the registry has announced it.
    settings: Rc<RefCell<Option<Settings>>>,
    registry_listener: Option<registry::Listener>,
    registry: Option<RegistryRc>,
    core: Option<CoreRc>,
    context: Option<ContextRc>,
    thread_loop: Option<ThreadLoopRc>,
}

/// Why PipeWire did not open: the OS error at the failure.
#[derive(Debug)]
pub struct OpenError(pub std::io::Error);

impl PipeWire {
    /// Connect, publish the card (its streams follow once PipeWire binds it) and start the
    /// thread loop.
    pub fn open(shared: Arc<Shared>, profile: usize, missing: &str) -> Result<PipeWire, OpenError> {
        pipewire::init();
        let failed = || OpenError(std::io::Error::last_os_error());
        // SAFETY: a thread loop created once, on this thread
        let thread_loop =
            unsafe { ThreadLoopRc::new(Some("a16-pipewire"), None) }.map_err(|_| failed())?;
        let mut this = PipeWire {
            nodes: Box::new(Nodes {
                shared: Arc::clone(&shared),
                missing_text: cstr(missing),
                core: ptr::null_mut(),
                // SAFETY: zeroed C structs, filled in below
                device: unsafe { std::mem::zeroed() },
                device_methods: spa::spa_device_methods {
                    version: spa::SPA_VERSION_DEVICE_METHODS,
                    add_listener: Some(device_add_listener),
                    sync: Some(device_sync),
                    enum_params: Some(device_enum_params),
                    set_param: Some(device_set_param),
                },
                device_hooks: unsafe { std::mem::zeroed() },
                device_params: unsafe { std::mem::zeroed() },
                device_items: unsafe { std::mem::zeroed() },
                device_props: unsafe { std::mem::zeroed() },
                device_proxy: ptr::null_mut(),
                proxy_events: unsafe { std::mem::zeroed() },
                proxy_listener: unsafe { std::mem::zeroed() },
                stream_events: unsafe { std::mem::zeroed() },
                device_id: spa::SPA_ID_INVALID,
                profile,
                profile_save: false,
                sink: None,
                source: None,
                data_loop: ptr::null_mut(),
                data_loop_name: String::new(),
            }),
            core_listener: None,
            settings: Rc::new(RefCell::new(None)),
            registry_listener: None,
            registry: None,
            core: None,
            context: None,
            thread_loop: Some(thread_loop),
        };
        let thread_loop = this.thread_loop.as_ref().expect("thread loop");
        let context = ContextRc::new(thread_loop, None).map_err(|_| failed())?;
        let core = context.connect_rc(None).map_err(|_| failed())?;
        let errors = Arc::clone(&shared);
        this.core_listener = Some(
            core.add_listener_local()
                .error(move |id, _seq, res, message| {
                    eprintln!("PipeWire error id={id}: {message} ({res})");
                    if id == pw::PW_ID_CORE || res == -libc::EPIPE {
                        errors.failed.store(true, Ordering::SeqCst);
                    }
                })
                .register(),
        );
        // The registry announces the `settings` metadata, as it does to pw-metadata; the
        // callbacks run on the thread loop, which has not started
        let registry = core.get_registry_rc().map_err(|_| failed())?;
        let weak = registry.downgrade();
        let found = Rc::clone(&this.settings);
        let removed = Rc::clone(&this.settings);
        let watched = Arc::clone(&shared);
        this.registry_listener = Some(
            registry
                .add_listener_local()
                .global(move |global| {
                    let Some(registry) = weak.upgrade() else {
                        return;
                    };
                    if found.borrow().is_some() {
                        return;
                    }
                    if let Some(settings) = watch_settings(&registry, global, &watched) {
                        *found.borrow_mut() = Some(settings);
                    }
                })
                .global_remove(move |id| {
                    let mut settings = removed.borrow_mut();
                    if settings.as_ref().is_some_and(|s| s.id == id) {
                        *settings = None;
                    }
                })
                .register(),
        );
        this.registry = Some(registry);
        let n = &mut *this.nodes;
        n.core = core.as_raw_ptr();
        // SAFETY: the context is live; its data loop lives as long as it does
        unsafe {
            let data_loop = pw::pw_context_get_data_loop(context.as_raw_ptr());
            if data_loop.is_null() {
                return Err(failed());
            }
            n.data_loop = pw::pw_data_loop_get_loop(data_loop);
            if n.data_loop.is_null() || (*n.data_loop).name.is_null() {
                return Err(failed());
            }
            n.data_loop_name = CStr::from_ptr((*n.data_loop).name)
                .to_string_lossy()
                .into_owned();
        }
        n.stream_events.version = pw::PW_VERSION_STREAM_EVENTS;
        n.stream_events.state_changed = Some(state_changed);
        n.stream_events.io_changed = Some(io_changed);
        n.stream_events.process = Some(process);
        n.proxy_events.version = pw::PW_VERSION_PROXY_EVENTS;
        n.proxy_events.bound = Some(device_bound);
        for (item, (key, value)) in n.device_items.iter_mut().zip(DEVICE_PROPS) {
            item.key = key.as_ptr();
            item.value = value.as_ptr();
        }
        n.device_props.n_items = n.device_items.len() as u32;
        n.device_props.items = n.device_items.as_ptr();
        n.device_params[0].id = spa::SPA_PARAM_EnumProfile;
        n.device_params[0].flags = spa::SPA_PARAM_INFO_READ;
        n.device_params[1].id = spa::SPA_PARAM_Profile;
        n.device_params[1].flags = spa::SPA_PARAM_INFO_READWRITE;
        let data = ptr::addr_of_mut!(*n).cast::<c_void>();
        // SAFETY: the Nodes are boxed, so the interface, hooks and listener stay put while
        // the proxy lives; the loop thread has not started
        unsafe {
            spa::spa_hook_list_init(&mut n.device_hooks);
            n.device.iface = spa::spa_interface {
                type_: spa::SPA_TYPE_INTERFACE_Device.as_ptr().cast(),
                version: spa::SPA_VERSION_DEVICE,
                cb: spa::spa_callbacks {
                    funcs: ptr::from_ref(&n.device_methods).cast(),
                    data,
                },
            };
            n.device_proxy = pw::pw_core_export(
                n.core,
                spa::SPA_TYPE_INTERFACE_Device.as_ptr().cast(),
                ptr::null(),
                ptr::addr_of_mut!(n.device).cast(),
                0,
            );
            if n.device_proxy.is_null() {
                return Err(failed());
            }
            pw::pw_proxy_add_listener(n.device_proxy, &mut n.proxy_listener, &n.proxy_events, data);
        }
        this.core = Some(core);
        this.context = Some(context);
        this.thread_loop.as_ref().expect("thread loop").start();
        Ok(this)
    }

    /// Stop the thread loop, then destroy the streams, the card and the connection.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(thread_loop) = &self.thread_loop {
            thread_loop.stop();
        }
        let n = &mut *self.nodes;
        n.destroy_sink();
        if let Some(endpoint) = n.source.take() {
            // SAFETY: the stream is live, and the loop stopped
            unsafe { pw::pw_stream_destroy(endpoint.stream) };
        }
        if !n.device_proxy.is_null() {
            // SAFETY: the proxy is live, and destroyed once
            unsafe { pw::pw_proxy_destroy(n.device_proxy) };
            n.device_proxy = ptr::null_mut();
        }
        self.settings.borrow_mut().take();
        self.registry_listener = None;
        self.registry = None;
        self.core_listener = None;
        self.core = None;
        self.context = None;
        if let Some(thread_loop) = self.thread_loop.take() {
            drop(thread_loop);
            // SAFETY: every PipeWire object is gone, and no PipeWire thread runs
            unsafe { pipewire::deinit() };
        }
    }
}

impl Drop for PipeWire {
    fn drop(&mut self) {
        self.shutdown();
    }
}
