// SPDX-License-Identifier: MIT
//! The bridge's USB lifecycle on a fake A16 and a fake clock, in the scenarios of
//! `docs/development/boot-usb-drop.md` and `tools/libusb-harness/PIPEWIRE.md`: streaming
//! starts on demand and then keeps going, an A16 that leaves the bus ends the driver with
//! status 0, a drop during the boot is reset and recovered with 9 s of quiet, a control
//! timeout resets at once, boot-time drops extend the boot window, and a sixth failure
//! within two minutes ends the driver with status 1.

mod support;

use support::{Event, FakeA16, MS, S, drive};

fn at(seconds: f64) -> u64 {
    (seconds * 1e9) as u64
}

/// The USB requests to the device, from `from` on: everything but submissions, events and
/// logs.
fn requests(fake: &FakeA16, from: u64) -> Vec<(u64, String)> {
    fake.lines("usb ")
        .into_iter()
        .filter(|(t, l)| *t >= from && !l.starts_with("usb submit"))
        .collect()
}

#[test]
fn streams_on_demand_then_keeps_streaming() {
    let fake = FakeA16::new(&[
        (at(5.0), Event::Playback(true)),
        (at(7.0), Event::Playback(false)),
    ]);
    let status = drive(&fake, 12 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    assert!(fake.first("log READY: A16 attached", 0).is_some(), "{log}");
    // The open configures the A16 and sends it nothing until a client plays
    let first_control = fake.first("usb control", 0).expect("a control request");
    assert!(first_control >= at(5.0), "{log}");
    assert_eq!(
        requests(&fake, 0)
            .iter()
            .take(6)
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>(),
        [
            "usb open",
            "usb config",
            "usb claim 0",
            "usb alt 0 1",
            "usb claim 1",
            "usb alt 1 1"
        ]
    );
    // The first start runs the handshake without the restart settle
    assert!(fake.first("usb sleep 120", 0).is_none(), "{log}");
    assert!(
        fake.first("log USB streaming started", 0).is_some(),
        "{log}"
    );
    // After the client stops, the stream goes on: transfers until the SIGTERM, then a
    // cancellation
    let stop = fake.first("event Stop", 0).expect("stop");
    assert!(fake.first("usb submit", at(11.0)).is_some(), "{log}");
    assert_eq!(
        fake.first("usb cancel", 0),
        fake.first("usb cancel", stop),
        "{log}"
    );
    assert!(fake.first("log PipeWire", 0).is_none());
}

#[test]
fn power_off_while_streaming_exits_0() {
    let fake = FakeA16::new(&[
        (at(2.0), Event::Loud(true)),
        (at(3.0), Event::Playback(true)),
        (at(8.0), Event::Leave),
    ]);
    let status = drive(&fake, 30 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    assert!(
        fake.first("log A16 return carries audio", 0).is_some(),
        "{log}"
    );
    assert!(
        fake.first("log A16 left the bus", at(8.0)).is_some(),
        "{log}"
    );
    assert!(
        fake.first("log A16 disconnected", at(8.0)).is_some(),
        "{log}"
    );
    assert!(fake.first("usb reset", 0).is_none(), "{log}");
    assert!(
        fake.first("event Stop", 0).is_none(),
        "ended before the SIGTERM: {log}"
    );
}

#[test]
fn power_off_while_idle_exits_0() {
    let fake = FakeA16::new(&[(at(5.0), Event::Leave)]);
    let status = drive(&fake, 30 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    let left = fake.first("log A16 left the bus", 0).expect("left");
    assert!(
        left - at(5.0) < 20 * MS,
        "the hotplug event ends the idle wait: {log}"
    );
    assert!(fake.first("log A16 disconnected", 0).is_some(), "{log}");
}

#[test]
fn no_a16_exits_0() {
    let fake = FakeA16::new(&[]);
    fake.state().on_bus = false;
    assert_eq!(drive(&fake, 30 * S), 0);
    assert!(
        fake.first("log No A16 on the bus", 0).is_some(),
        "{}",
        fake.dump()
    );
}

/// The requests the reopen after a failure makes, and when the stream resumes: the time of
/// the failure, of the reset, and of the first control request after it. With `streamed`,
/// the engine had streamed before, so its start settles for 120 ms first.
fn reopened(fake: &FakeA16, failure: &str, from: u64, streamed: bool) -> (u64, u64, u64) {
    let log = fake.dump();
    let failed = fake.first(failure, from).expect("a failure");
    let reset = fake.first("usb reset", failed).expect("a reset");
    // In log order from the failure: the close comes at the same fake time
    let lines = fake.state().log.clone();
    let start = lines
        .iter()
        .position(|(t, l)| *t >= failed && l.starts_with(failure))
        .expect("the failure line");
    let calls: Vec<String> = lines[start..]
        .iter()
        .map(|(_, l)| l.clone())
        .filter(|l| l.starts_with("usb ") && !l.starts_with("usb submit"))
        .skip_while(|l| l != "usb open")
        .take(if streamed { 9 } else { 8 })
        .collect();
    let mut expected = vec![
        "usb open",
        "usb reset",
        "usb config",
        "usb claim 0",
        "usb alt 0 1",
        "usb claim 1",
        "usb alt 1 1",
    ];
    if streamed {
        expected.push("usb sleep 120");
    }
    expected.push("usb control c0 56 0000 0000");
    assert_eq!(calls, expected, "{log}");
    let resumed = fake.first("usb control", reset).expect("the handshake");
    let quiet_until = if streamed {
        let settle = fake
            .first("usb sleep 120", reset)
            .expect("the restart settle");
        assert_eq!(resumed, settle + 120 * MS);
        settle
    } else {
        resumed
    };
    assert!(
        quiet_until >= failed + 9 * S,
        "nothing reaches the A16 until 9 s after the failure: {log}"
    );
    (failed, reset, resumed)
}

#[test]
fn boot_drop_resets_waits_and_resumes() {
    let fake = FakeA16::new(&[
        (at(5.0), Event::Playback(true)),
        (at(44.5), Event::Leave),
        (at(45.2), Event::Arrive),
    ]);
    let status = drive(&fake, 70 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    let (failed, reset, _) = reopened(&fake, "log A16 USB failure during its boot", 0, true);
    assert!(failed - at(44.5) < 5 * MS, "{log}");
    // The A16 is waited for while it is off the bus, and reset once it is back
    assert!(reset >= at(45.2) && reset < at(45.4), "{log}");
    assert!(
        fake.first("log USB reset done after 0.15 s", 0).is_some(),
        "{log}"
    );
    assert!(fake.first("log A16 reopened 0.", 0).is_some(), "{log}");
    assert!(
        fake.first("log No USB requests until 9.00 s after the failure", 0)
            .is_some(),
        "{log}"
    );
    assert!(
        fake.first("log USB streaming started, 9.", 0).is_some(),
        "{log}"
    );
    assert!(
        fake.first("usb submit", at(60.0)).is_some(),
        "streaming resumed: {log}"
    );
}

#[test]
fn boot_drop_with_the_a16_gone_for_3_s_exits_0() {
    let fake = FakeA16::new(&[(at(5.0), Event::Playback(true)), (at(44.5), Event::Leave)]);
    let status = drive(&fake, 70 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    let failed = fake
        .first("log A16 USB failure during its boot", 0)
        .expect("failure");
    let gone = fake.first("log A16 disconnected", 0).expect("disconnected");
    assert!(
        gone >= failed + 3 * S && gone < failed + 3 * S + 200 * MS,
        "{log}"
    );
}

#[test]
fn drop_outside_the_boot_window_exits_0() {
    // The boot window ends 50 s after the open at 1 s
    let fake = FakeA16::new(&[
        (at(52.0), Event::Playback(true)),
        (at(60.0), Event::Leave),
        (at(60.5), Event::Arrive),
    ]);
    let status = drive(&fake, 90 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    assert!(fake.first("log A16 left the bus", 0).is_some(), "{log}");
    assert!(fake.first("log A16 disconnected", 0).is_some(), "{log}");
    assert!(fake.first("usb reset", 0).is_none(), "{log}");
}

#[test]
fn drop_after_the_return_carried_audio_exits_0() {
    // Inside the boot window, but the A16 has finished booting
    let fake = FakeA16::new(&[
        (at(5.0), Event::Playback(true)),
        (at(10.0), Event::Loud(true)),
        (at(20.0), Event::Leave),
        (at(20.5), Event::Arrive),
    ]);
    let status = drive(&fake, 60 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    let booted = fake
        .first("log A16 return carries audio", 0)
        .expect("booted");
    // Three consecutive 100 ms windows at the level, the first of them partly
    assert!(booted >= at(10.2) && booted < at(10.32), "{log}");
    assert!(
        fake.first("log A16 USB failure during its boot", 0)
            .is_none(),
        "{log}"
    );
    assert!(fake.first("log A16 disconnected", 0).is_some(), "{log}");
}

#[test]
fn control_timeout_resets_at_once() {
    let fake = FakeA16::new(&[
        (at(2.0), Event::Loud(true)),
        (at(3.0), Event::StopAnswering),
        (at(5.0), Event::Playback(true)),
    ]);
    let status = drive(&fake, 40 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    // The return has not carried audio yet, so the failure counts as a boot-time drop
    let failed = fake
        .first("log A16 USB failure during its boot", 0)
        .expect("failure");
    // One request reached the A16 and timed out after 2 s; the retries went nowhere
    let controls: Vec<_> = requests(&fake, 0)
        .into_iter()
        .filter(|(t, l)| *t < failed && l.starts_with("usb control"))
        .collect();
    assert_eq!(controls.len(), 1, "{log}");
    assert!(failed - at(7.0) < 300 * MS, "{log}");
    let (_, reset, _) = reopened(&fake, "log A16 USB failure during its boot", 0, false);
    assert!(reset - failed < 10 * MS, "the reset follows at once: {log}");
    assert!(
        fake.first("log USB streaming started, 9.", 0).is_some(),
        "{log}"
    );
}

#[test]
fn repeated_boot_drops_extend_the_boot_window() {
    let fake = FakeA16::new(&[
        (at(5.0), Event::Playback(true)),
        (at(44.5), Event::Leave),
        (at(45.0), Event::Arrive),
        // After the 50 s window from the open, inside 5 s after the stream resumed
        (at(55.0), Event::Leave),
        (at(55.6), Event::Arrive),
    ]);
    let status = drive(&fake, 80 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    let drops = fake.lines("log A16 USB failure during its boot");
    assert_eq!(drops.len(), 2, "{log}");
    reopened(
        &fake,
        "log A16 USB failure during its boot",
        drops[1].0,
        true,
    );
    assert!(
        fake.first("usb submit", at(75.0)).is_some(),
        "streaming resumed: {log}"
    );
}

#[test]
fn a_drop_after_the_extended_window_exits_0() {
    let fake = FakeA16::new(&[
        (at(5.0), Event::Playback(true)),
        (at(44.5), Event::Leave),
        (at(45.0), Event::Arrive),
        // The window now ends 5 s after the stream may resume, at 44.5 + 9 + 5 s
        (at(59.0), Event::Leave),
        (at(59.5), Event::Arrive),
    ]);
    let status = drive(&fake, 80 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    assert_eq!(
        fake.lines("log A16 USB failure during its boot").len(),
        1,
        "{log}"
    );
    assert!(
        fake.first("log A16 disconnected", at(59.0)).is_some(),
        "{log}"
    );
}

/// Transfer errors with the A16 on the bus, `gap` apart from 10 s on.
fn failing(count: usize, gap: f64) -> FakeA16 {
    let mut events = vec![
        (at(2.0), Event::Loud(true)),
        (at(3.0), Event::Playback(true)),
    ];
    for i in 0..count {
        events.push((at(10.0 + gap * i as f64), Event::FailTransfer(1)));
    }
    FakeA16::new(&events)
}

#[test]
fn a_sixth_failure_within_two_minutes_exits_1() {
    let fake = failing(6, 11.0);
    let status = drive(&fake, 200 * S);
    let log = fake.dump();
    assert_eq!(status, 1, "{log}");
    assert_eq!(
        fake.lines("log A16 USB failure; resetting").len(),
        5,
        "{log}"
    );
    assert!(
        fake.first("log A16 failed 6 times within 120 s; giving up", 0)
            .is_some(),
        "{log}"
    );
    assert!(
        fake.first("bus USB endpoint 86 failed: transfer status 1", 0)
            .is_some()
            || fake
                .first("bus USB endpoint 05 failed: transfer status 1", 0)
                .is_some(),
        "{log}"
    );
}

#[test]
fn failures_spread_over_more_than_two_minutes_recover() {
    let fake = failing(7, 25.0);
    let status = drive(&fake, 220 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    assert_eq!(
        fake.lines("log A16 USB failure; resetting").len(),
        7,
        "{log}"
    );
    assert!(fake.first("log A16 failed", 0).is_none(), "{log}");
}

#[test]
fn an_a16_that_will_not_reset_ends_the_driver() {
    let fake = FakeA16::new(&[
        (at(2.0), Event::Loud(true)),
        (at(3.0), Event::Playback(true)),
        (at(10.0), Event::FailTransfer(1)),
    ]);
    fake.state().reset_errors =
        std::iter::repeat_n(open_realiser_a16_driver::usb::UsbError::IO, 9).collect();
    let status = drive(&fake, 60 * S);
    let log = fake.dump();
    assert_eq!(status, 1, "{log}");
    // Three reopens of three resets each
    assert_eq!(fake.lines("usb reset").len(), 9, "{log}");
    assert_eq!(
        fake.lines("log USB reset LIBUSB_ERROR_IO after 0.15 s")
            .len(),
        9,
        "{log}"
    );
    assert!(fake.first("log A16 did not recover", 0).is_some(), "{log}");
}

#[test]
fn sigterm_while_streaming_exits_0_and_releases() {
    let fake = FakeA16::new(&[(at(3.0), Event::Playback(true))]);
    let status = drive(&fake, 6 * S);
    let log = fake.dump();
    assert_eq!(status, 0, "{log}");
    let stop = fake.first("event Stop", 0).expect("stop");
    let calls: Vec<String> = requests(&fake, stop).into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        calls,
        [
            "usb cancel",
            "usb cancel",
            "usb release 1",
            "usb release 0",
            "usb close"
        ],
        "{log}"
    );
}
