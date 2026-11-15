// SPDX-License-Identifier: MIT
//
// The playback queue across a pause of the playback node: queued audio keeps playing out,
// playback resumes without a gap, and a queue that plays out while paused ends in silence
// without counting an underrun.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use a16_engine::{Direction, PcmRing};
use open_realiser_a16_driver::bridge::{AudioPath, BLOCK, Shared};
use open_realiser_a16_driver::bus::Audio;
use open_realiser_a16_driver::live;
use open_realiser_a16_driver::profiles::{self, Missing};

fn path() -> (Arc<Shared>, AudioPath) {
    let effective = profiles::effective(&Missing::default()).expect("profiles");
    let shared = Arc::new(Shared::new(effective, 0));
    let audio = AudioPath::new(Arc::clone(&shared), Box::new(|_| {})).expect("resamplers");
    (shared, audio)
}

/// `frames` frames of a constant level on every channel.
fn queue(shared: &Shared, frames: usize) {
    let samples = vec![0.25f32; frames * live::CHANNELS];
    assert_eq!(shared.sink.fifo.push(&samples) as usize, frames);
}

/// The next out transfer; whether any of its samples is not silent.
fn transfer(audio: &mut AudioPath, now: &mut u64) -> bool {
    let mut ring = PcmRing::new(Direction::Out, BLOCK);
    *now += 1_666_667;
    audio.before_out(&mut ring, *now);
    ring.as_bytes()
        .chunks(3)
        .any(|sample| live::unpack(sample) != 0.0)
}

#[test]
fn a_brief_pause_keeps_playing_the_queue() {
    let (shared, mut audio) = path();
    let mut now = 0;
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 4096);
    for _ in 0..5 {
        assert!(transfer(&mut audio, &mut now));
    }
    // PipeWire pauses the node for a moment: nothing is added, the queue plays on
    shared.sink.active.store(false, Ordering::SeqCst);
    let before = shared.sink.fifo.available();
    for _ in 0..3 {
        assert!(
            transfer(&mut audio, &mut now),
            "audio stopped during the pause"
        );
    }
    assert!(shared.sink.fifo.available() < before);
    // Streaming again: the next audio follows straight on
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 1024);
    for _ in 0..5 {
        assert!(transfer(&mut audio, &mut now), "a gap after the pause");
    }
    assert_eq!(shared.sink.underruns.load(Ordering::SeqCst), 0);
}

#[test]
fn a_queue_that_plays_out_while_paused_ends_in_silence() {
    let (shared, mut audio) = path();
    let mut now = 0;
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, live::TARGET as usize);
    assert!(transfer(&mut audio, &mut now));
    shared.sink.active.store(false, Ordering::SeqCst);
    let mut silent = 0;
    for _ in 0..60 {
        if !transfer(&mut audio, &mut now) {
            silent += 1;
        }
    }
    assert!(silent > 0, "the queue never played out");
    assert!(!transfer(&mut audio, &mut now));
    assert_eq!(shared.sink.underruns.load(Ordering::SeqCst), 0);
    // Streaming again with less than the target queued: silence until it fills
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 512);
    assert!(!transfer(&mut audio, &mut now));
    queue(&shared, live::TARGET as usize);
    assert!(transfer(&mut audio, &mut now));
}
