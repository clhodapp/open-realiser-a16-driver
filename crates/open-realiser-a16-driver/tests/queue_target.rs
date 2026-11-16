// SPDX-License-Identifier: MIT
//
// The `a16.queue-target` request: each queue takes it up when it primes from empty, never
// while it plays, and a request outside the bounds for the graph's quantum is clamped and
// logged.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use a16_engine::{Direction, PcmRing};
use open_realiser_a16_driver::bridge::{AudioPath, BLOCK, Shared};
use open_realiser_a16_driver::bus::Audio;
use open_realiser_a16_driver::live;
use open_realiser_a16_driver::profiles::{self, Missing};

type Lines = Rc<RefCell<Vec<String>>>;

fn path() -> (Arc<Shared>, AudioPath, Lines) {
    let effective = profiles::effective(&Missing::default()).expect("profiles");
    let shared = Arc::new(Shared::new(effective, 0));
    let lines: Lines = Rc::default();
    let sink = Rc::clone(&lines);
    let audio = AudioPath::new(
        Arc::clone(&shared),
        Box::new(move |line| sink.borrow_mut().push(line.to_owned())),
    )
    .expect("resamplers");
    (shared, audio, lines)
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

/// Out transfers until the playback queue runs dry.
fn play_out(audio: &mut AudioPath, now: &mut u64) {
    for _ in 0..200 {
        if !transfer(audio, now) {
            return;
        }
    }
    panic!("the queue never played out");
}

fn targets(lines: &Lines) -> Vec<String> {
    lines
        .borrow()
        .iter()
        .filter(|l| l.contains("queue target"))
        .cloned()
        .collect()
}

#[test]
fn a_request_applies_when_playback_next_primes_from_empty() {
    let (shared, mut audio, lines) = path();
    let mut now = 0;
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 4096);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), live::TARGET);
    // Asked for while playing: the queue keeps its target
    shared.queue_request.store(1024, Ordering::SeqCst);
    for _ in 0..5 {
        assert!(transfer(&mut audio, &mut now));
        assert_eq!(audio.playback_target(), live::TARGET);
    }
    // The client stops feeding the queue: it underruns, and primes again to the request,
    // from at least a quantum and the USB margin
    play_out(&mut audio, &mut now);
    assert_eq!(shared.sink.underruns.load(Ordering::SeqCst), 1);
    queue(&shared, 1024 + 319);
    assert!(!transfer(&mut audio, &mut now));
    queue(&shared, 1);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), 1024);
    assert_eq!(
        targets(&lines),
        [
            "A16 playback queue target: 2048 frames (default)",
            "A16 playback queue target: 1024 frames (requested 1024)",
        ]
    );
}

#[test]
fn a_pause_that_plays_out_primes_to_the_new_target() {
    let (shared, mut audio, lines) = path();
    let mut now = 0;
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, live::TARGET as usize);
    assert!(transfer(&mut audio, &mut now));
    shared.queue_request.store(3000, Ordering::SeqCst);
    shared.sink.active.store(false, Ordering::SeqCst);
    play_out(&mut audio, &mut now);
    assert_eq!(audio.playback_target(), live::TARGET);
    // Streaming again: silence until the queue holds the new target
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 2900);
    assert!(!transfer(&mut audio, &mut now));
    queue(&shared, 100);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), 3000);
    // Back to the default at the next start
    shared.queue_request.store(0, Ordering::SeqCst);
    shared.sink.active.store(false, Ordering::SeqCst);
    play_out(&mut audio, &mut now);
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, live::TARGET as usize);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), live::TARGET);
    assert_eq!(shared.sink.underruns.load(Ordering::SeqCst), 0);
    assert_eq!(
        targets(&lines),
        [
            "A16 playback queue target: 2048 frames (default)",
            "A16 playback queue target: 3000 frames (requested 3000)",
            "A16 playback queue target: 2048 frames (default)",
        ]
    );
}

#[test]
fn requests_outside_the_bounds_are_clamped_for_the_quantum() {
    let (shared, mut audio, lines) = path();
    let mut now = 0;
    shared.graph_quantum.store(1024, Ordering::SeqCst);
    shared.queue_request.store(512, Ordering::SeqCst);
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 1343);
    assert!(!transfer(&mut audio, &mut now));
    queue(&shared, 1);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), 832);
    // A smaller quantum allows the request
    shared.graph_quantum.store(256, Ordering::SeqCst);
    shared.sink.active.store(false, Ordering::SeqCst);
    play_out(&mut audio, &mut now);
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, 512);
    assert!(!transfer(&mut audio, &mut now));
    queue(&shared, 64);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), 512);
    // Above the largest target
    shared.queue_request.store(20000, Ordering::SeqCst);
    shared.sink.active.store(false, Ordering::SeqCst);
    play_out(&mut audio, &mut now);
    shared.sink.active.store(true, Ordering::SeqCst);
    queue(&shared, live::MAX_TARGET as usize);
    assert!(transfer(&mut audio, &mut now));
    assert_eq!(audio.playback_target(), live::MAX_TARGET);
    assert_eq!(
        targets(&lines),
        [
            "A16 playback queue target: 832 frames (requested 512, raised to the minimum for quantum 1024)",
            "A16 playback queue target: 512 frames (requested 512)",
            "A16 playback queue target: 8192 frames (requested 20000, lowered to the maximum)",
        ]
    );
}

/// An in transfer of silence.
fn receive(audio: &mut AudioPath) {
    audio.after_in(&PcmRing::new(Direction::In, BLOCK));
}

/// The capture consumer, as the return node's process callback runs it: whether the capture
/// queue is playing after a cycle that takes `frames` frames.
fn consume(shared: &Shared, primed: &mut bool, frames: u32) {
    let fifo = &shared.source.fifo;
    if !*primed && shared.capture_ready(fifo.available()) {
        *primed = true;
    }
    if *primed {
        let mut out = vec![0.0f32; frames as usize * 2];
        let got = fifo.peek(&mut out);
        fifo.consume(got);
        if got < frames {
            *primed = false;
        }
    }
}

/// In transfers until the capture queue holds `frames` frames.
fn fill(shared: &Shared, audio: &mut AudioPath, frames: u32) {
    for _ in 0..200 {
        if shared.source.fifo.available() >= frames {
            return;
        }
        receive(audio);
    }
    panic!("the capture queue never filled");
}

#[test]
fn the_return_takes_a_request_when_its_queue_primes() {
    let (shared, mut audio, lines) = path();
    let mut primed = false;
    shared.source.active.store(true, Ordering::SeqCst);
    fill(&shared, &mut audio, live::TARGET);
    consume(&shared, &mut primed, 1024);
    assert!(primed);
    assert_eq!(shared.capture_target(), (live::TARGET, 0));
    // Asked for while the queue plays: the target stays
    shared.queue_request.store(1500, Ordering::SeqCst);
    for _ in 0..5 {
        fill(&shared, &mut audio, 1024);
        consume(&shared, &mut primed, 1024);
        assert!(primed);
    }
    assert_eq!(shared.capture_target(), (live::TARGET, 0));
    // The consumer runs the queue dry, and it primes again to the request
    consume(&shared, &mut primed, 8192);
    assert!(!primed);
    consume(&shared, &mut primed, 1024);
    assert!(!primed);
    assert_eq!(shared.capture_target(), (1500, 1500));
    fill(&shared, &mut audio, 1500);
    consume(&shared, &mut primed, 1024);
    assert!(primed);
    // The quantum the graph runs when the queue primes sets the bound
    shared.queue_request.store(100, Ordering::SeqCst);
    shared.graph_quantum.store(256, Ordering::SeqCst);
    consume(&shared, &mut primed, 8192);
    consume(&shared, &mut primed, 256);
    assert_eq!(shared.capture_target(), (448, 100));
    receive(&mut audio);
    shared.graph_quantum.store(1024, Ordering::SeqCst);
    consume(&shared, &mut primed, 1024);
    assert_eq!(shared.capture_target(), (832, 100));
    receive(&mut audio);
    assert_eq!(
        targets(&lines),
        [
            "A16 return queue target: 2048 frames (default)",
            "A16 return queue target: 1500 frames (requested 1500)",
            "A16 return queue target: 448 frames (requested 100, raised to the minimum for quantum 256)",
            "A16 return queue target: 832 frames (requested 100, raised to the minimum for quantum 1024)",
        ]
    );
}

/// The return node starts streaming again: its consumer, discarding the queue, primes it to
/// the request.
#[test]
fn the_return_primes_to_the_request_when_it_starts_again() {
    let (shared, mut audio, lines) = path();
    let mut primed = false;
    let source = &shared.source;
    source.active.store(true, Ordering::SeqCst);
    fill(&shared, &mut audio, live::TARGET);
    consume(&shared, &mut primed, 1024);
    assert!(primed);
    shared.queue_request.store(3000, Ordering::SeqCst);
    source.active.store(false, Ordering::SeqCst);
    receive(&mut audio);
    source.active.store(true, Ordering::SeqCst);
    // The stream's state change resets the queue, and the consumer primes it afresh
    source.fifo.discard();
    primed = false;
    receive(&mut audio);
    consume(&shared, &mut primed, 1024);
    assert!(!primed);
    fill(&shared, &mut audio, 3000);
    consume(&shared, &mut primed, 1024);
    assert!(primed);
    assert_eq!(shared.capture_target(), (3000, 3000));
    assert_eq!(
        targets(&lines),
        [
            "A16 return queue target: 2048 frames (default)",
            "A16 return queue target: 3000 frames (requested 3000)",
        ]
    );
}
