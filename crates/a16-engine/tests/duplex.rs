// SPDX-License-Identifier: MIT
//! The A16 offline at every rate it lists and with no rate requested, on its interrupt
//! endpoints and a bulk pair: both directions make progress, and a stereo test signal on the
//! wire decodes to its samples.

mod common;

use a16_engine::{Direction, PcmRing, SampleRate, TransferType};
use common::{FakeBus, pipe, quiet_engine};

/// Every in frame: channel 1 = +0x123456, channel 2 = -0x123456.
fn stereo(transfer: &mut a16_engine::Transfer) {
    let buffer = transfer.buffer_mut();
    buffer.fill(0);
    for frame in buffer.as_chunks_mut::<64>().0 {
        for bit in 0..24 {
            frame[bit] = ((0x123456u32 >> (23 - bit)) & 1) as u8;
            frame[32 + bit] = ((0xedcbaau32 >> (23 - bit)) & 1) as u8;
        }
    }
}

fn duplex(interrupt: bool, rate: Option<SampleRate>) {
    let mut bus = FakeBus::new();
    let mut e = quiet_engine();
    let (kind, out_max, in_max) = if interrupt {
        (TransferType::Interrupt, 964, 1024)
    } else {
        (TransferType::Bulk, 512, 512)
    };
    e.set_pipe(Direction::Out, pipe(kind, out_max, 1));
    e.set_pipe(Direction::In, pipe(kind, in_max, 1));
    e.attach_pcm_ring(PcmRing::new(Direction::In, 1024), 0);
    e.attach_pcm_ring(PcmRing::new(Direction::Out, 1024), 0);
    e.set_rate(rate);
    e.start(&mut bus).expect("start");
    assert_eq!(e.sample_rate(), Some(rate.unwrap_or(SampleRate::Hz48000)));
    for _ in 0..12 {
        let transfer = bus.pop().expect("a transfer in flight");
        bus.complete(&mut e, transfer, stereo);
    }
    assert!(bus.progress[Direction::In] > 0 && bus.progress[Direction::Out] > 0);
    let input = e.pcm_ring(Direction::In).expect("in ring");
    assert_eq!(input.frame(0), [0x56, 0x34, 0x12, 0xaa, 0xcb, 0xed]);
    e.stop(&mut bus);
    assert!(bus.pop().is_none());
}

#[test]
fn every_rate_both_transports() {
    // No request: the A16 runs at 48 kHz
    duplex(false, None);
    duplex(true, None);
    for rate in SampleRate::ALL {
        duplex(false, Some(rate));
        duplex(true, Some(rate));
    }
}
