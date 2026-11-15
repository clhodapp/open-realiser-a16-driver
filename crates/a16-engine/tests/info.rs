// SPDX-License-Identifier: MIT
//! What the engine reports about the stream on the A16: a start sets the pipes and the format;
//! a restart at another rate updates them.

mod common;

use a16_engine::{Direction, OutLayout, SampleRate, TransferType};
use common::{FakeBus, a16_engine};

#[test]
fn after_start_and_restart() {
    let mut bus = FakeBus::new();
    bus.i_reg[0] = 0x16;
    let mut e = a16_engine();
    assert_eq!(e.sample_rate(), None);
    e.start(&mut bus).expect("start");

    assert_eq!(
        e.out_layout(),
        OutLayout::Interrupt,
        "964-byte interrupt out endpoint"
    );
    assert_eq!(e.sample_rate(), Some(SampleRate::Hz48000));
    assert_eq!(
        (
            e.transfer_frames(Direction::Out),
            e.transfer_frames(Direction::In)
        ),
        (80, 80)
    );
    assert_eq!(
        (
            e.transfer_bytes(Direction::Out),
            e.transfer_bytes(Direction::In)
        ),
        (3856, 5120)
    );
    assert_eq!(e.safety_offset(Direction::Out), 4 * 80);
    assert_eq!(e.safety_offset(Direction::In), 4 * 80);
    let out = e.pipe(Direction::Out).expect("out pipe");
    assert_eq!(
        (out.transfer_type, out.max_packet_size),
        (TransferType::Interrupt, 964)
    );
    assert_eq!(
        (e.in_flight(Direction::Out), e.in_flight(Direction::In)),
        (4, 4)
    );

    // Another rate: the restart runs at it
    e.change_rate(SampleRate::Hz44100);
    e.restart(&mut bus).expect("rate restart");
    assert_eq!(e.sample_rate(), Some(SampleRate::Hz44100));
    e.stop(&mut bus);
}
