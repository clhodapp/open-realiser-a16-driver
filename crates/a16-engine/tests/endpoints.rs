// SPDX-License-Identifier: MIT
//! Endpoint binding on the A16's endpoint list (fixed addresses, with endpoints it doesn't
//! use around them), one with PCM in on the wrong interface, and a High Speed high-bandwidth
//! wMaxPacketSize.

mod common;

use a16_engine::{Direction, TransferType, UsbEndpoint};
use common::quiet_engine;

const fn ep(
    interface: u8,
    address: u8,
    attributes: u8,
    interval: u8,
    max_packet: u16,
) -> UsbEndpoint {
    UsbEndpoint {
        interface,
        address,
        attributes,
        interval,
        max_packet,
    }
}

#[test]
fn a16_pipes() {
    let a16 = [
        ep(0, 0x02, 0x01, 1, 4),
        ep(0, 0x83, 0x03, 1, 64),
        ep(0, 0x04, 0x02, 0, 512),
        ep(0, 0x05, 0x03, 1, 964),
        ep(1, 0x81, 0x01, 1, 4),
        ep(1, 0x86, 0x03, 1, 1024),
    ];
    let mut engine = quiet_engine();
    let bound = engine.bind_endpoints(&a16);
    assert_eq!(bound[Direction::Out].map(|e| e.address), Some(0x05));
    assert_eq!(bound[Direction::In].map(|e| e.address), Some(0x86));
    let out = engine.pipe(Direction::Out).expect("out pipe");
    let input = engine.pipe(Direction::In).expect("in pipe");
    assert_eq!(
        (out.max_packet_size, out.transfer_type),
        (964, TransferType::Interrupt)
    );
    assert_eq!(
        (input.max_packet_size, input.transfer_type),
        (1024, TransferType::Interrupt)
    );
}

#[test]
fn pcm_in_on_the_wrong_interface() {
    let partial = [
        ep(0, 0x05, 0x03, 1, 964),
        ep(0, 0x83, 0x03, 1, 64),
        ep(0, 0x86, 0x03, 1, 1024),
    ];
    let mut engine = quiet_engine();
    let bound = engine.bind_endpoints(&partial);
    assert_eq!(bound[Direction::Out].map(|e| e.address), Some(0x05));
    assert_eq!(bound[Direction::In], None);
    assert_eq!(
        engine.pipe(Direction::In),
        None,
        "unbound direction told to the engine"
    );
}

#[test]
fn high_bandwidth_bytes_per_interval() {
    assert_eq!(ep(0, 0x02, 0x01, 1, 0x0800 | 312).bytes_per_interval(), 624);
}
