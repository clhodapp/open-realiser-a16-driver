// SPDX-License-Identifier: MIT
//! Settings, rate changes and restarts on the A16: a setting asks for a restart, bad values
//! are refused; a rate the A16 lists asks for a restart; a restart aborts what is in flight
//! and runs the handshake again, and stop leaves nothing in flight; one transfer in flight is
//! refused, and one kept from before runs as two.

mod common;

use a16_engine::{
    Direction, EngineState, SampleRate, Setting, SettingChange, SettingError, TransferType,
};
use common::{FakeBus, a16_engine, pipe};

#[test]
fn settings_rates_and_restarts() {
    let mut bus = FakeBus::new();
    let mut e = a16_engine();
    e.start(&mut bus).expect("start");
    assert!(bus.complete_next(&mut e), "nothing in flight after start");

    assert_eq!(
        e.sample_rate(),
        Some(SampleRate::Hz48000),
        "the A16 starts at 48 kHz"
    );
    assert!(matches!(
        e.change_setting(Setting::PcmOutQueue, 4),
        Err(SettingError::OutOfRange { .. })
    ));
    assert_eq!(
        e.change_setting(Setting::PcmOutQueue, 3),
        Ok(SettingChange::Unchanged)
    );
    assert_eq!(
        e.change_setting(Setting::PcmOutQueue, 2),
        Ok(SettingChange::NeedsRestart)
    );

    assert!(bus.queued() > 0, "nothing in flight before the restart");
    e.restart(&mut bus).expect("restart");
    assert_eq!(
        e.settings()[Setting::PcmOutQueue],
        2,
        "restart lost the setting"
    );
    assert_eq!(e.in_flight(Direction::Out), 3);
    assert!(
        bus.queued() > 0 && e.state() == EngineState::Streaming,
        "not streaming after restart"
    );

    assert_eq!(SampleRate::from_hz(192000), None, "a rate the A16 lacks");
    assert_eq!(e.change_rate(SampleRate::Hz48000), SettingChange::Unchanged);
    assert_eq!(
        e.change_rate(SampleRate::Hz44100),
        SettingChange::NeedsRestart
    );
    e.restart(&mut bus).expect("rate restart");
    assert_eq!(e.sample_rate(), Some(SampleRate::Hz44100));

    // Transfers are whole packets: A16 out 964-byte interrupt, 20 frames; in 1024, 16 frames
    assert_eq!(pipe(TransferType::Interrupt, 1024, 1).packet_frames(64), 16);
    assert_eq!(pipe(TransferType::Bulk, 512, 1).packet_frames(64), 8);
    assert_eq!(pipe(TransferType::Interrupt, 964, 1).packet_frames(48), 20);
    assert_eq!(pipe(TransferType::Bulk, 512, 1).packet_frames(48), 10);
    assert_eq!(
        pipe(TransferType::Isochronous, 1024, 1).packet_frames(64),
        0
    );
    assert_eq!(
        (
            e.packet_frames(Direction::Out),
            e.packet_frames(Direction::In)
        ),
        (20, 16)
    );
    for value in [0, 2] {
        assert!(
            matches!(
                e.change_setting(Setting::PcmOutFrames, value),
                Err(SettingError::NotWholePackets { .. })
            ),
            "{value}: 10 / 30-frame out transfers taken on 964-byte packets"
        );
        assert!(
            matches!(
                e.change_setting(Setting::PcmInFrames, value),
                Err(SettingError::NotWholePackets { .. })
            ),
            "{value}: 8 / 24-frame in transfers taken on 1024-byte packets"
        );
    }
    assert_eq!(
        e.change_setting(Setting::PcmOutFrames, 1),
        Ok(SettingChange::NeedsRestart)
    );
    assert_eq!(
        e.change_setting(Setting::PcmInFrames, 1),
        Ok(SettingChange::NeedsRestart)
    );

    // A value kept from before that doesn't fit runs at the next size up, or for the
    // largest size, the next size down
    e.set_setting(Setting::PcmInFrames, 0);
    e.set_setting(Setting::PcmOutFrames, 0);
    e.restart(&mut bus).expect("restart");
    assert_eq!(e.settings()[Setting::PcmInFrames], 1, "8 frames kept");
    assert_eq!(e.settings()[Setting::PcmOutFrames], 1, "10 frames kept");
    e.set_setting(Setting::PcmInFrames, 24);
    e.restart(&mut bus).expect("restart");
    assert_eq!(e.settings()[Setting::PcmInFrames], 23, "200 frames kept");
    assert_eq!(e.transfer_frames(Direction::In), 192);

    // One transfer in flight isn't offered; one kept from before runs as two
    for setting in [Setting::PcmOutQueue, Setting::PcmInQueue] {
        assert_eq!(
            e.change_setting(setting, 0),
            Err(SettingError::QueueTooShort { setting })
        );
        e.set_setting(setting, 0);
    }
    e.restart(&mut bus).expect("restart");
    assert_eq!(e.settings()[Setting::PcmOutQueue], 1);
    assert_eq!(e.settings()[Setting::PcmInQueue], 1);
    assert_eq!(
        (e.in_flight(Direction::Out), e.in_flight(Direction::In)),
        (2, 2)
    );

    e.stop(&mut bus);
    assert_eq!(bus.queued(), 0, "transfers in flight after stop");
    assert_eq!(e.state(), EngineState::Stopped);
}

#[test]
fn suspend_keeps_the_device_published() {
    let mut bus = FakeBus::new();
    let mut e = a16_engine();
    e.suspend(&mut bus);
    assert_eq!(
        e.state(),
        EngineState::Stopped,
        "a suspend before any start"
    );
    e.start(&mut bus).expect("start");
    assert!(e.start(&mut bus).is_err(), "a second start while streaming");
    e.suspend(&mut bus);
    assert_eq!((e.state(), bus.queued()), (EngineState::Suspended, 0));
    e.start(&mut bus).expect("start after suspend");
    assert_eq!(e.state(), EngineState::Streaming);
}
