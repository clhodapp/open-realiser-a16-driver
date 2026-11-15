// SPDX-License-Identifier: MIT
//! The stream's stats on the A16 at its defaults (48 kHz, 80 frames x 4 queued each way,
//! 1667 us a transfer): completions on time are counted and give the longest gap, nothing
//! late; a gap past the queue (4 x 1667 us plus 1 ms of jitter) is late on the direction it
//! held up, one just short of it isn't; an error and a short transfer count, each telling
//! the platform (stats_changed); a stop's aborts are no gap after the next start; a new
//! engine starts from zero.

mod common;

use std::time::Duration;

use a16_engine::{Direction, Engine, SampleRate, Status};
use common::{FakeBus, a16_engine};

/// Per 80-frame transfer at 48 kHz.
const PERIOD: u64 = 1667;

fn start() -> (Engine, FakeBus) {
    let mut bus = FakeBus::new();
    bus.fixed_clock = true;
    bus.now = Duration::from_micros(1000);
    let mut e = a16_engine();
    e.start(&mut bus).expect("start");
    (e, bus)
}

/// The next out completion `gap` us after the last, and every in one queued before it.
fn step(e: &mut Engine, bus: &mut FakeBus, gap: u64) {
    while let Some(transfer) = bus.pop() {
        let out = transfer.direction() == Direction::Out;
        if out {
            bus.now += Duration::from_micros(gap);
        }
        bus.complete(e, transfer, |_| {});
        if out {
            return;
        }
    }
}

fn run_for(e: &mut Engine, bus: &mut FakeBus, n: usize, gap: u64) {
    for _ in 0..n {
        step(e, bus, gap);
    }
}

/// Completes queued out transfers until an in one comes, and returns that.
fn next_in(e: &mut Engine, bus: &mut FakeBus) -> a16_engine::Transfer {
    loop {
        let transfer = bus.pop().expect("a transfer in flight");
        if transfer.direction() == Direction::In {
            return transfer;
        }
        bus.complete(e, transfer, |_| {});
    }
}

#[test]
fn counters() {
    let (mut e, mut bus) = start();
    assert_eq!(e.sample_rate(), Some(SampleRate::Hz48000));
    assert_eq!(
        (
            e.transfer_frames(Direction::Out),
            e.transfer_frames(Direction::In)
        ),
        (80, 80)
    );
    let out = |e: &Engine| e.stats().directions[Direction::Out];
    let input = |e: &Engine| e.stats().directions[Direction::In];

    // On time, with a little jitter
    run_for(&mut e, &mut bus, 200, PERIOD);
    run_for(&mut e, &mut bus, 1, PERIOD + 300);
    run_for(&mut e, &mut bus, 200, PERIOD);
    assert_eq!(out(&e).transfers, 401);
    assert!(
        input(&e).transfers >= 400,
        "in transfers {}",
        input(&e).transfers
    );
    assert_eq!(out(&e).max_gap, Duration::from_micros(PERIOD + 300));
    assert_eq!((e.stats().incidents(), bus.stats_changed), (0, 0));

    // Just short of the queue's 6666 us plus 1 ms: not late; past it: late, both ways (the
    // in completion waited as long)
    run_for(&mut e, &mut bus, 1, 7600);
    assert_eq!(out(&e).late, 0, "a 7.6 ms gap was late");
    run_for(&mut e, &mut bus, 1, 9000);
    run_for(&mut e, &mut bus, 1, PERIOD); // the in completion queued behind it
    assert_eq!((out(&e).late, input(&e).late), (1, 1));
    assert_eq!(out(&e).max_gap, Duration::from_micros(9000));
    assert_eq!(input(&e).max_gap, Duration::from_micros(9000));
    assert_eq!(bus.stats_changed, 2, "told of the two late ones");

    // An error and a short transfer, on whatever in transfer is queued
    let told = bus.stats_changed;
    let transfer = next_in(&mut e, &mut bus);
    e.complete(&mut bus, transfer, Status::Failed);
    let transfer = next_in(&mut e, &mut bus);
    let short = transfer.len() - 64;
    e.complete(&mut bus, transfer, Status::Completed(short));
    assert_eq!((input(&e).errors, input(&e).short_transfers), (1, 1));
    assert_eq!((out(&e).errors, out(&e).short_transfers), (0, 0));
    assert_eq!(
        bus.stats_changed,
        told + 2,
        "told of the error and the short one"
    );

    // A restart: its aborts are no gap, and a long pause before the next start isn't late
    e.suspend(&mut bus);
    bus.now += Duration::from_secs(10);
    e.start(&mut bus).expect("restart");
    run_for(&mut e, &mut bus, 50, PERIOD);
    assert_eq!(
        (out(&e).late, input(&e).late),
        (1, 1),
        "the restart was late"
    );
    assert_eq!(
        (out(&e).errors, input(&e).errors),
        (0, 1),
        "the aborts were errors"
    );
    e.stop(&mut bus);

    // A new engine: from zero
    let (mut e, mut bus) = start();
    assert_eq!(e.stats().incidents(), 0);
    assert_eq!(out(&e).transfers, 0);
    assert_eq!(out(&e).max_gap, Duration::ZERO);
    e.stop(&mut bus);
}
