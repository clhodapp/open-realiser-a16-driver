// SPDX-License-Identifier: MIT
//! Every out transfer the engine submits is a buffer of exactly one transfer's bytes, and
//! the frames on the wire are the ring's frames in order: out transfer n >= 4 carries ring
//! frames (n - 4) * 80 onwards, wrapping at the ring's end.

mod common;

use a16_engine::codec::encode_frame;
use a16_engine::{Direction, OutLayout, PcmRing};
use common::{FakeBus, a16_engine, fake_rand};

#[test]
fn out_frames_follow_the_ring() {
    let mut bus = FakeBus::new();
    let mut e = a16_engine();
    let ring: Vec<u8> = (0..800 * 48).map(|i| (i * 11 + 1) as u8).collect();
    e.attach_pcm_ring(PcmRing::from_bytes(Direction::Out, ring.clone()), 0);
    e.start(&mut bus).expect("start");

    let mut seed = 3u32;
    let (mut outs, mut wrong) = (0, 0);
    for _ in 0..400 {
        let Some(transfer) = bus.pop() else { break };
        assert_eq!(transfer.len(), e.transfer_bytes(transfer.direction()));
        if transfer.direction() == Direction::Out {
            let n = transfer.id().0 as usize;
            if n >= 4 {
                outs += 1;
                for f in 0..80 {
                    let at = ((n - 4) * 80 + f) % 800 * 48;
                    let mut expected = [0; 48];
                    encode_frame(
                        &mut expected,
                        ring[at..at + 48].try_into().expect("a frame"),
                    );
                    let offset = OutLayout::Interrupt.frame_offset(f as u32);
                    wrong += usize::from(transfer.buffer()[offset..offset + 48] != expected);
                }
            }
        }
        bus.complete(&mut e, transfer, |t| {
            for b in t.buffer_mut() {
                *b = fake_rand(&mut seed) as u8;
            }
        });
    }
    assert!(outs > 50, "{outs} ring-encoded out transfers");
    assert_eq!(wrong, 0, "wrong frames");
    e.stop(&mut bus);
}
