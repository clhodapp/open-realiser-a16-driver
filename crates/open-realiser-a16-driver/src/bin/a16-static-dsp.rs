// SPDX-License-Identifier: MIT
//! `a16-static-dsp`: synchronous PipeWire SPA DSP, stereo f32le on stdin to Custom #2
//! 16-channel f32le on stdout. No server connection, clock adaptation, realtime thread or
//! USB access. The buffer setup follows PipeWire's MIT-licensed test-audioconvert.c (Wim
//! Taymans).

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::{Read, Seek, SeekFrom, Write};
use std::ptr;

use open_realiser_a16_driver::{perror, strerror};
use pipewire::spa::sys as spa;
use pipewire::sys as pw;

const BLOCK: usize = 512;
const CHANNELS: usize = 16;
/// The USB slots of the filter graph's twelve outputs.
const USB: [usize; 12] = [0, 1, 2, 3, 6, 7, 8, 9, 11, 12, 13, 14];

/// `spa_filter_graph_methods`, version 0, from `spa/filter-graph/filter-graph.h`.
#[repr(C)]
struct FilterGraphMethods {
    version: u32,
    add_listener:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *mut c_void) -> c_int>,
    enum_prop_info:
        Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut *mut c_void) -> c_int>,
    get_props: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> c_int>,
    set_props: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> c_int>,
    activate: Option<unsafe extern "C" fn(*mut c_void, *const spa::spa_dict) -> c_int>,
    deactivate: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    reset: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    process: Option<
        unsafe extern "C" fn(*mut c_void, *const *const c_void, *const *mut c_void, u32) -> c_int,
    >,
}

const FILTER_GRAPH: &CStr = c"Spa:Pointer:Interface:FilterGraph";

/// A filter graph's methods and their object.
///
/// # Safety
///
/// `graph` must be a live `spa_filter_graph`.
unsafe fn graph_methods(
    graph: *mut spa::spa_interface,
) -> (&'static FilterGraphMethods, *mut c_void) {
    // SAFETY: the caller's interface, whose funcs are filter graph methods
    unsafe {
        (
            &*(*graph).cb.funcs.cast::<FilterGraphMethods>(),
            (*graph).cb.data,
        )
    }
}

/// Exit 1 with the call's text and error unless `res` is a success.
fn check(res: c_int, call: &str) {
    if res < 0 {
        eprintln!("{call}: {}", strerror(-res));
        std::process::exit(1);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn load_props(path: &str) -> *mut pw::pw_properties {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(error) => fail(&perror(path, &error)),
    };
    let size = match f.seek(SeekFrom::End(0)) {
        Ok(size) if size <= 1024 * 1024 => size,
        _ => fail("Invalid config size"),
    };
    let mut text = Vec::with_capacity(size as usize + 1);
    if f.rewind().is_err() || f.read_to_end(&mut text).map_or(true, |n| n as u64 != size) {
        fail("Config read failed");
    }
    // The text ends at its first NUL, as a C string does
    if let Some(end) = text.iter().position(|&c| c == 0) {
        text.truncate(end);
    }
    let text = CString::new(text).unwrap_or_default();
    // SAFETY: a NUL-terminated string
    let props = unsafe { pw::pw_properties_new_string(text.as_ptr()) };
    if props.is_null() {
        fail(&format!("Invalid properties: {path}"));
    }
    props
}

fn set(props: *mut pw::pw_properties, key: &CStr, value: &CStr) {
    // SAFETY: live properties and NUL-terminated strings
    unsafe {
        pw::pw_properties_set(props, key.as_ptr(), value.as_ptr());
    }
}

/// `N` planes of one block each, aligned as a C compiler aligns static arrays: audioconvert
/// takes its SIMD paths by the alignment of the planes, so a plane's alignment can change the
/// samples.
#[repr(C, align(64))]
struct Planes<const N: usize>([[f32; BLOCK]; N]);

/// A port's buffer of planar float channels, and its io area.
#[repr(C)]
struct Buffers {
    buffer: spa::spa_buffer,
    data: [spa::spa_data; 8],
    chunk: [spa::spa_chunk; 8],
    io: spa::spa_io_buffers,
}

/// Set port 0 of `dir` to planar float at 48 kHz with `positions`, in convert mode.
///
/// # Safety
///
/// `node` must be a live audioconvert node.
unsafe fn format(node: *mut spa::spa_node, dir: spa::spa_direction, positions: &[u32]) {
    let mut mem = [0u8; 4096];
    // SAFETY: a builder over the local buffer, and the caller's node
    unsafe {
        let mut b: spa::spa_pod_builder = std::mem::zeroed();
        spa::spa_pod_builder_init(&mut b, mem.as_mut_ptr().cast(), mem.len() as u32);
        let mut info: spa::spa_audio_info_raw = std::mem::zeroed();
        info.format = spa::SPA_AUDIO_FORMAT_F32P;
        info.rate = 48000;
        info.channels = positions.len() as u32;
        info.position[..positions.len()].copy_from_slice(positions);
        let f = spa::spa_format_audio_raw_build(&mut b, spa::SPA_PARAM_Format, &info);
        let mut frame: spa::spa_pod_frame = std::mem::zeroed();
        spa::spa_pod_builder_push_object(
            &mut b,
            &mut frame,
            spa::SPA_TYPE_OBJECT_ParamPortConfig,
            spa::SPA_PARAM_PortConfig,
        );
        spa::spa_pod_builder_prop(&mut b, spa::SPA_PARAM_PORT_CONFIG_direction, 0);
        spa::spa_pod_builder_id(&mut b, dir);
        spa::spa_pod_builder_prop(&mut b, spa::SPA_PARAM_PORT_CONFIG_mode, 0);
        spa::spa_pod_builder_id(&mut b, spa::SPA_PARAM_PORT_CONFIG_MODE_convert);
        let config = spa::spa_pod_builder_pop(&mut b, &mut frame).cast::<spa::spa_pod>();
        check(
            spa::spa_node_set_param(node, spa::SPA_PARAM_PortConfig, 0, config),
            "spa_node_set_param(node, SPA_PARAM_PortConfig, 0, config)",
        );
        check(
            spa::spa_node_port_set_param(node, dir, 0, spa::SPA_PARAM_Format, 0, f),
            "spa_node_port_set_param(node, dir, 0, SPA_PARAM_Format, 0, f)",
        );
    }
}

/// Give port 0 of `dir` one buffer over `planes`, and its io area.
///
/// # Safety
///
/// `node` must be a live node; `b` and the planes must outlive its use of them.
unsafe fn buffers(
    node: *mut spa::spa_node,
    b: &mut Buffers,
    dir: spa::spa_direction,
    planes: &mut [[f32; BLOCK]],
) {
    // SAFETY: the caller's node; `b` and the planes stay put while the node uses them
    unsafe {
        *b = std::mem::zeroed();
        b.buffer.n_datas = planes.len() as u32;
        b.buffer.datas = b.data.as_mut_ptr();
        for (i, plane) in planes.iter_mut().enumerate() {
            b.data[i].type_ = spa::SPA_DATA_MemPtr;
            b.data[i].flags = spa::SPA_DATA_FLAG_READWRITE;
            b.data[i].fd = -1;
            b.data[i].maxsize = (BLOCK * 4) as u32;
            b.data[i].data = plane.as_mut_ptr().cast();
            b.data[i].chunk = &mut b.chunk[i];
            b.chunk[i].stride = 4;
        }
        let mut buf: *mut spa::spa_buffer = &mut b.buffer;
        check(
            spa::spa_node_port_use_buffers(node, dir, 0, 0, &mut buf, 1),
            "spa_node_port_use_buffers(node, dir, 0, 0, &buf, 1)",
        );
        b.io = spa::spa_io_buffers {
            status: spa::SPA_STATUS_NEED_DATA as i32,
            buffer_id: spa::SPA_ID_INVALID,
        };
        check(
            spa::spa_node_port_set_io(
                node,
                dir,
                0,
                spa::SPA_IO_Buffers,
                ptr::from_mut(&mut b.io).cast(),
                size_of::<spa::spa_io_buffers>(),
            ),
            "spa_node_port_set_io(node, dir, 0, SPA_IO_Buffers, &b->io, sizeof(b->io))",
        );
    }
}

/// `%.9g`.
fn g9(value: f64) -> String {
    let mut text = [0 as c_char; 64];
    // SAFETY: snprintf writes at most the buffer's size, NUL included
    unsafe {
        libc::snprintf(text.as_mut_ptr(), text.len(), c"%.9g".as_ptr(), value);
        CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
    }
}

/// Fill `buf` from `input` as `fread` does: until it is full, at end of input or on an
/// error. The bytes read, and the error.
fn read_full(input: &mut impl Read, buf: &mut [u8]) -> (usize, Option<std::io::Error>) {
    let mut n = 0;
    while n < buf.len() {
        match input.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return (n, Some(error)),
        }
    }
    (n, None)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!(
            "Usage: a16-static-dsp PSD-PROPS.json HEIGHT-PROPS.json < stereo.f32 > custom2.f32"
        );
        std::process::exit(2);
    }
    pipewire::init();
    // SAFETY: the C API, used in order on this thread; every buffer the nodes use lives
    // until they are unloaded
    unsafe {
        let main_loop = pw::pw_main_loop_new(ptr::null());
        let ctx = pw::pw_context_new(pw::pw_main_loop_get_loop(main_loop), ptr::null_mut(), 0);
        if ctx.is_null() {
            fail(&perror("pw_context_new", &std::io::Error::last_os_error()));
        }
        let mix_props = load_props(&args[1]);
        let graph_props = load_props(&args[2]);
        set(mix_props, c"clock.quantum-limit", c"8192");
        let mix_handle =
            pw::pw_context_load_spa_handle(ctx, c"audio.convert".as_ptr(), &(*mix_props).dict);
        if mix_handle.is_null() {
            fail(&perror(
                "load audioconvert",
                &std::io::Error::last_os_error(),
            ));
        }
        let mut mix: *mut c_void = ptr::null_mut();
        check(
            spa::spa_handle_get_interface(
                mix_handle,
                spa::SPA_TYPE_INTERFACE_Node.as_ptr().cast(),
                &mut mix,
            ),
            "spa_handle_get_interface(mix_handle, SPA_TYPE_INTERFACE_Node, (void **)&mix)",
        );
        let mix = mix.cast::<spa::spa_node>();
        let positions = [
            spa::SPA_AUDIO_CHANNEL_FL,
            spa::SPA_AUDIO_CHANNEL_FR,
            spa::SPA_AUDIO_CHANNEL_FC,
            spa::SPA_AUDIO_CHANNEL_LFE,
            spa::SPA_AUDIO_CHANNEL_SL,
            spa::SPA_AUDIO_CHANNEL_SR,
            spa::SPA_AUDIO_CHANNEL_RL,
            spa::SPA_AUDIO_CHANNEL_RR,
        ];
        format(mix, spa::SPA_DIRECTION_INPUT, &positions[..2]);
        format(mix, spa::SPA_DIRECTION_OUTPUT, &positions);
        let mut input = Box::new(Planes([[0.0f32; BLOCK]; 2]));
        let mut bed = Box::new(Planes([[0.0f32; BLOCK]; 8]));
        let mut height = Box::new(Planes([[0.0f32; BLOCK]; 12]));
        let mut inbuf: Box<Buffers> = Box::new(std::mem::zeroed());
        let mut outbuf: Box<Buffers> = Box::new(std::mem::zeroed());
        buffers(mix, &mut inbuf, spa::SPA_DIRECTION_INPUT, &mut input.0[..]);
        buffers(mix, &mut outbuf, spa::SPA_DIRECTION_OUTPUT, &mut bed.0[..]);
        let start = spa::spa_command {
            pod: spa::spa_pod {
                size: size_of::<spa::spa_command_body>() as u32,
                type_: spa::SPA_TYPE_Object,
            },
            body: spa::spa_command_body {
                body: spa::spa_pod_object_body {
                    type_: spa::SPA_TYPE_COMMAND_Node,
                    id: spa::SPA_NODE_COMMAND_Start,
                },
            },
        };
        check(
            spa::spa_node_send_command(mix, &start),
            "spa_node_send_command(mix, &start)",
        );
        set(
            graph_props,
            c"library.name",
            c"filter-graph/libspa-filter-graph",
        );
        set(graph_props, c"clock.quantum-limit", c"8192");
        let graph_handle =
            pw::pw_context_load_spa_handle(ctx, c"filter.graph".as_ptr(), &(*graph_props).dict);
        if graph_handle.is_null() {
            fail(&perror(
                "load filter graph",
                &std::io::Error::last_os_error(),
            ));
        }
        let mut graph: *mut c_void = ptr::null_mut();
        check(
            spa::spa_handle_get_interface(graph_handle, FILTER_GRAPH.as_ptr(), &mut graph),
            "spa_handle_get_interface(graph_handle, SPA_TYPE_INTERFACE_FilterGraph, (void **)&graph)",
        );
        let (methods, object) = graph_methods(graph.cast());
        let rate = [spa::spa_dict_item {
            key: c"audio.rate".as_ptr(),
            value: c"48000".as_ptr(),
        }];
        let rate = spa::spa_dict {
            flags: 0,
            n_items: 1,
            items: rate.as_ptr(),
        };
        check(
            methods
                .activate
                .map_or(-libc::ENOTSUP, |f| f(object, &rate)),
            "spa_filter_graph_activate(graph, &SPA_DICT_ITEMS(SPA_DICT_ITEM(\"audio.rate\", \"48000\")))",
        );
        let bin: [*const c_void; 8] = std::array::from_fn(|i| bed.0[i].as_ptr().cast());
        let hout: [*mut c_void; 12] = std::array::from_fn(|i| height.0[i].as_mut_ptr().cast());
        let mut frames: u64 = 0;
        let mut peaks = [0.0f64; 16];
        let mut interleaved = [0u8; BLOCK * 2 * 4];
        let mut output = vec![0.0f32; BLOCK * CHANNELS];
        let mut stdin = std::io::stdin().lock();
        let mut stdout = std::io::BufWriter::with_capacity(65536, std::io::stdout().lock());
        loop {
            let (bytes, error) = read_full(&mut stdin, &mut interleaved);
            if bytes % 8 != 0 {
                fail("Truncated stereo frame");
            }
            let count = bytes / 8;
            if count == 0 {
                if let Some(error) = error {
                    fail(&perror("stdin", &error));
                }
                break;
            }
            input.0 = [[0.0; BLOCK]; 2];
            for i in 0..count {
                for c in 0..2 {
                    let at = (i * 2 + c) * 4;
                    input.0[c][i] = f32::from_le_bytes([
                        interleaved[at],
                        interleaved[at + 1],
                        interleaved[at + 2],
                        interleaved[at + 3],
                    ]);
                }
            }
            for c in 0..2 {
                inbuf.chunk[c].size = (BLOCK * 4) as u32;
                inbuf.chunk[c].offset = 0;
            }
            inbuf.io.status = spa::SPA_STATUS_HAVE_DATA as i32;
            inbuf.io.buffer_id = 0;
            outbuf.io.status = spa::SPA_STATUS_NEED_DATA as i32;
            check(spa::spa_node_process(mix), "spa_node_process(mix)");
            if outbuf.io.status != spa::SPA_STATUS_HAVE_DATA as i32
                || outbuf.chunk[0].size != (BLOCK * 4) as u32
            {
                fail(&format!(
                    "Short DSP output at frame {frames}: status {}, bytes {}, expected {}",
                    outbuf.io.status,
                    outbuf.chunk[0].size,
                    BLOCK * 4
                ));
            }
            check(
                methods.process.map_or(-libc::ENOTSUP, |f| {
                    f(object, bin.as_ptr(), hout.as_ptr(), BLOCK as u32)
                }),
                "spa_filter_graph_process(graph, bin, hout, BLOCK)",
            );
            output[..count * 16].fill(0.0);
            for i in 0..count {
                for (c, &slot) in USB.iter().enumerate() {
                    let v = height.0[c][i];
                    if !v.is_finite() {
                        fail("Nonfinite output");
                    }
                    output[i * 16 + slot] = v;
                    let magnitude = f64::from(v).abs();
                    if magnitude > peaks[slot] {
                        peaks[slot] = magnitude;
                    }
                }
            }
            let mut out = Vec::with_capacity(count * 16 * 4);
            for v in &output[..count * 16] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            if let Err(error) = stdout.write_all(&out) {
                fail(&perror("stdout", &error));
            }
            frames += count as u64;
        }
        if let Err(error) = stdout.flush() {
            fail(&perror("fflush", &error));
        }
        let peaks: Vec<String> = peaks.iter().map(|&p| g9(p)).collect();
        eprintln!("frames={frames} peaks={}", peaks.join(","));
        if let Some(f) = methods.deactivate {
            f(object);
        }
        pw::pw_unload_spa_handle(graph_handle);
        pw::pw_unload_spa_handle(mix_handle);
        pw::pw_properties_free(mix_props);
        pw::pw_properties_free(graph_props);
        pw::pw_context_destroy(ctx);
        pw::pw_main_loop_destroy(main_loop);
        drop(stdout);
        pipewire::deinit();
    }
}
