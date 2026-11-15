// SPDX-License-Identifier: MIT
//! Golden trace: what the engine does on the bus for the A16, in four runs of 600
//! completions:
//!   interrupt  the 964-byte interrupt out layout, PCM rings attached at completion 20,
//!              detached at 300
//!   bulk       the 512-byte bulk out layout, the same rings
//!   96 kHz     the interrupt layout at 96 kHz
//!   selector   the interrupt layout with the 'I' selector already at Automatic (no write)
//! Every submission, control request and progress report is printed, then hashes of the
//! rings. Compared with `golden/trace.txt`: the golden trace of the C engine this crate
//! ports, without what only the C shared memory had, each submission's offset in it and its
//! hash.

mod common;

use std::fmt::Write as _;

use a16_engine::{Direction, PcmRing, SampleRate, TransferType};
use common::{FakeBus, fnv, pipe, quiet_engine};

/// The C test's rings: 960 frames, in a buffer of 960 16-channel frames each way.
const FRAMES: usize = 960;
const RING_BUFFER_BYTES: usize = FRAMES * 16 * 3;

fn run(
    out: &mut String,
    name: &str,
    transfer_type: TransferType,
    out_max: u16,
    in_max: u16,
    rate: u32,
    selector: u8,
) {
    let mut bus = FakeBus::traced();
    bus.i_reg[0] = selector;
    let mut counter = [0u32; 256];
    let mut fill = |t: &mut a16_engine::Transfer| {
        let c = &mut counter[usize::from(t.endpoint())];
        for b in t.buffer_mut() {
            *b = c.wrapping_mul(37).wrapping_add(11) as u8;
            *c += 1;
        }
    };

    let _ = writeln!(out, "== {name}");
    let mut engine = quiet_engine();
    engine.set_pipe(Direction::Out, pipe(transfer_type, out_max, 1));
    engine.set_pipe(Direction::In, pipe(transfer_type, in_max, 1));
    engine.set_rate(SampleRate::from_hz(rate));
    let ok = engine.start(&mut bus).is_ok();
    let trace = bus.trace.as_mut().expect("traced");
    let _ = writeln!(trace, "start {}", u8::from(ok));

    let out_ring: Vec<u8> = (0..RING_BUFFER_BYTES).map(|k| (k * 13 + 5) as u8).collect();
    let mut out_ring = Some(PcmRing::from_bytes(Direction::Out, out_ring));
    let mut in_ring = Some(PcmRing::new(Direction::In, FRAMES));
    let mut done = 0;
    while done < 600 {
        let Some(transfer) = bus.pop() else { break };
        done += 1;
        if done == 20 {
            engine.attach_pcm_ring(out_ring.take().expect("out ring"), 0);
            engine.attach_pcm_ring(in_ring.take().expect("in ring"), 0);
        }
        if done == 300 {
            out_ring = engine.detach_pcm_ring(Direction::Out);
            in_ring = engine.detach_pcm_ring(Direction::In);
            bus.trace.as_mut().expect("traced").push_str("detached\n");
        }
        bus.complete(&mut engine, transfer, &mut fill);
    }
    engine.stop(&mut bus);

    // The in ring sat at the start of a buffer as large as the out ring
    let mut in_bytes = in_ring.expect("in ring").into_bytes();
    in_bytes.resize(RING_BUFFER_BYTES, 0);
    let out_bytes = out_ring.expect("out ring").into_bytes();
    let trace = bus.trace.take().expect("traced");
    out.push_str(&trace);
    let _ = writeln!(
        out,
        "out_ring {:016x} in_ring {:016x}",
        fnv(&out_bytes),
        fnv(&in_bytes)
    );
}

#[test]
fn matches_golden() {
    let mut out = String::new();
    run(
        &mut out,
        "interrupt",
        TransferType::Interrupt,
        964,
        1024,
        0,
        0,
    );
    run(&mut out, "bulk", TransferType::Bulk, 512, 512, 0, 0);
    run(
        &mut out,
        "96 kHz",
        TransferType::Interrupt,
        964,
        1024,
        96000,
        0,
    );
    run(
        &mut out,
        "selector",
        TransferType::Interrupt,
        964,
        1024,
        0,
        0x32,
    );
    let golden = include_str!("golden/trace.txt");
    if out != golden {
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden-trace.txt");
        std::fs::write(&path, &out).expect("write the trace");
        panic!(
            "trace differs from golden/trace.txt; written to {}",
            path.display()
        );
    }
}
