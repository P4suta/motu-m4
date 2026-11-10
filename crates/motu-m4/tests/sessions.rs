use motu_m4::{
    run_stream, FrameCount, Operation, SampleRate, Scenario, SimulatedM4, StreamRequest,
};

const RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

fn samples(frames: u32) -> Vec<u8> {
    (0..frames * 4)
        .flat_map(|sample| (sample << 8).to_le_bytes())
        .collect()
}

#[test]
fn every_rate_captures_one_second_with_exact_fractional_packet_scheduling() {
    for hz in RATES {
        let mut device = SimulatedM4::new(Scenario::Steady);
        let output = run_stream(
            &mut device,
            SampleRate::new(hz).unwrap(),
            StreamRequest::Capture(FrameCount::new(hz).unwrap()),
            |_| false,
        )
        .unwrap();
        assert_eq!(output.audio, samples(hz), "{hz}");
        assert_eq!(output.report.rate.hz(), hz);
        assert_eq!(output.report.packet_lengths.len(), 8000);
        for (packet, length) in output.report.packet_lengths.iter().enumerate() {
            let before = packet as u64 * u64::from(hz) / 8000;
            let after = (packet + 1) as u64 * u64::from(hz) / 8000;
            assert_eq!(
                u64::from(*length),
                (after - before) * 16,
                "{hz} packet {packet}"
            );
        }
        assert!(device.is_attached());
        assert_eq!(device.clock(), 44_100);
    }
}

#[test]
fn duplex_preserves_both_directions_at_every_rate_and_clock_scenario() {
    let source: Vec<u8> = (0u32..76)
        .flat_map(|sample| (0x8000_0000 | sample << 8).to_le_bytes())
        .collect();
    for hz in RATES {
        for scenario in [
            Scenario::Steady,
            Scenario::Drift,
            Scenario::NegativeDrift,
            Scenario::Intermittent,
        ] {
            let mut device = SimulatedM4::new(scenario);
            let output = run_stream(
                &mut device,
                SampleRate::new(hz).unwrap(),
                StreamRequest::Duplex(&source),
                |_| false,
            )
            .unwrap();
            assert_eq!(output.audio, samples(19), "{hz} {scenario:?}");
            assert_eq!(output.report.frames, 19);
            assert_eq!(&device.played()[..source.len()], source);
            assert!(device.played()[source.len()..]
                .iter()
                .all(|byte| *byte == 0));
            assert!(device.is_attached());
            assert_eq!(device.clock(), 44_100);
        }
    }
}

#[test]
fn cancellation_covers_all_modes_before_ownership_and_during_transfer() {
    let source = samples(100);
    for request in [
        StreamRequest::Capture(FrameCount::new(100).unwrap()),
        StreamRequest::Playback(&source),
        StreamRequest::Duplex(&source),
    ] {
        for after in [0, 1, 2] {
            let mut device = SimulatedM4::new(Scenario::Steady);
            let error = run_stream(
                &mut device,
                SampleRate::new(48_000).unwrap(),
                request,
                |packets| packets >= after,
            )
            .unwrap_err();
            assert_eq!(
                error.primary.unwrap().cause,
                motu_m4::FailureCause::Cancelled
            );
            assert!(error.recovery.is_empty());
            assert!(device.is_attached());
            assert_eq!(device.clock(), 44_100);
            let reads = device
                .events()
                .iter()
                .filter(|op| **op == Operation::Capture)
                .count();
            assert_eq!(reads, after as usize);
            if after == 0 {
                assert!(device.events().is_empty());
            }
        }
    }
}

#[test]
fn clock_negotiation_refuses_bad_responses_before_starting_streams() {
    for (scenario, cause) in [
        (Scenario::ShortControl, motu_m4::FailureCause::ShortControl),
        (
            Scenario::UnsupportedRate,
            motu_m4::FailureCause::Protocol(motu_m4_core::Error::Clock),
        ),
        (
            Scenario::MalformedRanges,
            motu_m4::FailureCause::Protocol(motu_m4_core::Error::Clock),
        ),
        (Scenario::InvalidClock, motu_m4::FailureCause::InvalidClock),
        (
            Scenario::ClockMismatch,
            motu_m4::FailureCause::ClockMismatch,
        ),
    ] {
        let mut device = SimulatedM4::new(scenario);
        let error = run_stream(
            &mut device,
            SampleRate::new(48_000).unwrap(),
            StreamRequest::Capture(FrameCount::new(1).unwrap()),
            |_| false,
        )
        .unwrap_err();
        assert_eq!(error.primary.unwrap().cause, cause, "{scenario:?}");
        assert!(error.recovery.is_empty(), "{scenario:?}: {error:?}");
        assert!(!device.events().contains(&Operation::StartCapture));
        assert!(device.is_attached());
        assert_eq!(device.clock(), 44_100);
    }
}

#[test]
fn negotiation_transport_failures_attempt_all_acquired_resource_cleanup() {
    for operation in [
        Operation::ReadRanges,
        Operation::VerifyClock,
        Operation::ClockValidity,
    ] {
        let mut device = SimulatedM4::new(Scenario::Steady);
        device.fail_once(operation);
        let error = run_stream(
            &mut device,
            SampleRate::new(96_000).unwrap(),
            StreamRequest::Capture(FrameCount::new(1).unwrap()),
            |_| false,
        )
        .unwrap_err();
        assert_eq!(error.primary.unwrap().operation, operation);
        assert!(error.recovery.is_empty());
        assert!(device.is_attached());
        assert_eq!(device.clock(), 44_100);
    }
}

#[test]
fn repeat_sessions_reset_the_simulator_clock_phase_and_payload() {
    let mut device = SimulatedM4::new(Scenario::Drift);
    for _ in 0..3 {
        let source = samples(19);
        let output = run_stream(
            &mut device,
            SampleRate::new(48_000).unwrap(),
            StreamRequest::Duplex(&source),
            |_| false,
        )
        .unwrap();
        assert_eq!(output.audio, source);
        assert_eq!(output.report.packet_lengths, [80, 112, 96, 96]);
        assert_eq!(device.played().len(), 384);
        assert!(device.is_attached());
    }
}

#[test]
fn playback_discards_feedback_audio_and_returns_the_exact_session_report() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let output = run_stream(
        &mut device,
        SampleRate::new(48_000).unwrap(),
        StreamRequest::Playback(&samples(19)),
        |_| false,
    )
    .unwrap();
    assert!(output.audio.is_empty());
    assert_eq!(output.report.frames, 19);
    assert_eq!(output.report.packet_lengths, [96, 96, 96, 96]);
    assert_eq!(device.played().len(), 384);
    assert!(device.is_attached());
}

#[test]
fn playback_sends_one_matching_packet_for_each_input_interval_including_empty_feedback() {
    let mut device = SimulatedM4::new(Scenario::Intermittent);
    let output = run_stream(
        &mut device,
        SampleRate::new(48_000).unwrap(),
        StreamRequest::Duplex(&samples(100)),
        |_| false,
    )
    .unwrap();
    assert_eq!(output.report.packet_lengths.first(), Some(&0));
    let captures = device
        .events()
        .iter()
        .filter(|op| **op == Operation::Capture)
        .count();
    let playbacks = device
        .events()
        .iter()
        .filter(|op| **op == Operation::Playback)
        .count();
    assert_eq!(captures, output.report.packet_lengths.len());
    assert_eq!(playbacks, captures);
    assert_eq!(output.audio, samples(100));
}

#[test]
fn callback_unwinding_releases_every_acquired_stream_resource() {
    let source = samples(100);
    for request in [
        StreamRequest::Capture(FrameCount::new(100).unwrap()),
        StreamRequest::Playback(&source),
        StreamRequest::Duplex(&source),
    ] {
        let mut device = SimulatedM4::new(Scenario::Steady);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_stream(
                &mut device,
                SampleRate::new(48_000).unwrap(),
                request,
                |packets| {
                    assert_ne!(packets, 1, "callback unwound");
                    false
                },
            )
        }));
        assert!(result.is_err());
        assert!(device.is_attached(), "{request:?}");
        assert_eq!(device.clock(), 44_100);
        assert!(device.events().ends_with(&[
            Operation::StopCapture,
            Operation::RestoreClock,
            Operation::VerifyRestoreClock,
            Operation::Release,
            Operation::Attach
        ]));
    }
}
