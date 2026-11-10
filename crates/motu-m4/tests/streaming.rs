use motu_m4::{capture, play, FrameCount, Operation, Scenario, SimulatedM4};

#[test]
fn captures_exact_frames_and_restores_the_original_clock_and_driver() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let recorded = capture(&mut device, FrameCount::new(19).unwrap()).unwrap();
    assert_eq!(recorded.audio.len(), 19 * 16);
    for (frame, bytes) in recorded.audio.as_chunks::<16>().0.iter().enumerate() {
        for (channel, sample) in bytes.as_chunks::<4>().0.iter().enumerate() {
            let actual = u32::from_le_bytes(*sample);
            assert_eq!(actual, ((frame * 4 + channel) as u32) << 8);
        }
    }
    assert!(device.is_attached());
    assert_eq!(device.clock(), 44_100);
}

#[test]
fn playback_obeys_drifting_implicit_feedback_and_preserves_audio() {
    let audio: Vec<u8> = (0..19 * 16).map(|index| (index % 251) as u8).collect();
    let mut device = SimulatedM4::new(Scenario::Drift);
    let report = play(&mut device, &audio).unwrap();
    assert_eq!(report.frames, 19);
    assert_eq!(report.packet_lengths, [80, 112, 96, 96]);
    let canonical: Vec<u8> = audio
        .iter()
        .enumerate()
        .map(|(index, byte)| if index.is_multiple_of(4) { 0 } else { *byte })
        .collect();
    assert_eq!(&device.played()[..audio.len()], canonical);
    assert!(device.played()[audio.len()..].iter().all(|byte| *byte == 0));
    assert!(device.is_attached());
    assert_eq!(device.clock(), 44_100);
}

#[test]
fn drift_accumulates_an_extra_frame_over_a_complete_virtual_clock_period() {
    let mut device = SimulatedM4::new(Scenario::Drift);
    let result = capture(&mut device, FrameCount::new(12_289).unwrap()).unwrap();
    assert_eq!(result.report.packet_lengths.len(), 2048);
    assert_eq!(
        result
            .report
            .packet_lengths
            .iter()
            .map(|length| usize::from(*length) / 16)
            .sum::<usize>(),
        12_289
    );
    assert_eq!(result.audio.len(), 12_289 * 16);
    assert!(device.is_attached());
}

#[test]
fn playback_clears_the_unused_low_bits_of_each_24_bit_sample() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let source = [
        0x78, 0x56, 0x34, 0x12, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0x80, 3, 2, 1, 0,
    ];
    play(&mut device, &source).unwrap();
    assert_eq!(
        &device.played()[..16],
        [0, 0x56, 0x34, 0x12, 0, 0xff, 0xff, 0xff, 0, 0, 0, 0x80, 0, 2, 1, 0]
    );
}

#[test]
fn every_injected_stream_failure_attempts_recovery() {
    for operation in [
        Operation::ReadClock,
        Operation::SetClock,
        Operation::StartCapture,
        Operation::StartPlayback,
        Operation::Capture,
        Operation::Playback,
    ] {
        let mut device = SimulatedM4::new(Scenario::Steady);
        device.fail_once(operation);
        let error = play(&mut device, &[0; 64]).unwrap_err();
        assert!(error.primary.is_some(), "{operation:?}");
        assert!(error.recovery.is_empty(), "{operation:?}");
        assert!(device.is_attached(), "{operation:?}");
        assert_eq!(device.clock(), 44_100, "{operation:?}");
    }
}

#[test]
fn reports_the_stream_error_and_all_recovery_errors() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    device.fail_once(Operation::Capture);
    device.fail_once(Operation::StopCapture);
    device.fail_once(Operation::RestoreClock);
    let error = capture(&mut device, FrameCount::new(12).unwrap()).unwrap_err();
    assert_eq!(error.primary.unwrap().operation, Operation::Capture);
    assert_eq!(error.recovery.len(), 2);
    assert_eq!(error.recovery[0].operation, Operation::StopCapture);
    assert_eq!(error.recovery[1].operation, Operation::RestoreClock);
    assert!(device.is_attached());
}

#[test]
fn malformed_feedback_and_stalled_streams_fail_without_leaking_ownership() {
    for scenario in [Scenario::Malformed, Scenario::Stalled, Scenario::Timeout] {
        let mut device = SimulatedM4::new(scenario);
        assert!(play(&mut device, &[0; 1024]).is_err(), "{scenario:?}");
        assert!(device.is_attached(), "{scenario:?}");
    }
}

#[test]
fn physical_disconnection_cannot_report_successful_restoration() {
    let mut device = SimulatedM4::new(Scenario::Disconnect);
    let error = play(&mut device, &[0; 1024]).unwrap_err();
    assert_eq!(
        error.primary.unwrap().cause,
        motu_m4::FailureCause::Transport(motu_m4::TransportError::Disconnected)
    );
    assert!(!error.recovery.is_empty());
    assert!(!device.is_attached());
}

#[test]
fn cancellation_stops_capture_and_restores_ownership() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let mut calls = 0;
    let error = motu_m4::capture_with_cancel(&mut device, FrameCount::new(100).unwrap(), || {
        calls += 1;
        calls == 3
    })
    .unwrap_err();
    assert_eq!(
        error.primary.unwrap().cause,
        motu_m4::FailureCause::Cancelled
    );
    assert!(error.recovery.is_empty());
    assert!(device.is_attached());
    assert_eq!(device.clock(), 44_100);
}

#[test]
fn recovery_failures_prevent_a_successful_result() {
    for operation in [
        Operation::StopPlayback,
        Operation::StopCapture,
        Operation::RestoreClock,
        Operation::VerifyRestoreClock,
        Operation::Release,
        Operation::Attach,
    ] {
        let mut device = SimulatedM4::new(Scenario::Steady);
        device.fail_once(operation);
        let error = play(&mut device, &[0; 64]).unwrap_err();
        assert!(error.primary.is_none());
        assert!(error
            .recovery
            .iter()
            .any(|failure| failure.operation == operation));
    }
}

#[test]
fn a_failed_takeover_preserves_the_original_binding() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    device.fail_once(Operation::Takeover);
    let error = capture(&mut device, FrameCount::new(1).unwrap()).unwrap_err();
    assert_eq!(error.primary.unwrap().operation, Operation::Takeover);
    assert!(error.recovery.is_empty());
    assert!(device.is_attached());
    assert_eq!(device.events(), [Operation::Takeover]);
}

#[test]
fn invalid_input_never_takes_the_device() {
    for input in [vec![], vec![0; 15], vec![0; 17]] {
        let mut device = SimulatedM4::new(Scenario::Steady);
        assert!(play(&mut device, &input).is_err());
        assert!(device.events().is_empty());
    }
}

#[test]
fn stalled_input_is_detected_after_32_empty_packets() {
    let mut device = SimulatedM4::new(Scenario::Stalled);
    let error = play(&mut device, &[0; 16]).unwrap_err();
    assert_eq!(error.primary.unwrap().cause, motu_m4::FailureCause::Stalled);
    assert_eq!(
        device
            .events()
            .iter()
            .filter(|operation| **operation == Operation::Capture)
            .count(),
        32
    );
    assert!(device.is_attached());
}
