use motu_m4::{
    run_stream, AudioTransport, DeviceProfile, FrameCount, Operation, SampleRate, Scenario,
    SimulatedM4, StreamRequest, TransportError,
};
use motu_m4_core::Layout;

const PROFILES: [DeviceProfile; 4] = [
    DeviceProfile::Recorded,
    DeviceProfile::AtmelTemplate,
    DeviceProfile::NxpTemplate,
    DeviceProfile::OriginalSynthetic,
];

#[test]
fn firmware_profiles_run_all_modes_rates_and_feedback_scenarios() {
    let source: Vec<u8> = (0..76_u32)
        .flat_map(|sample| (sample << 8).to_le_bytes())
        .collect();
    for profile in PROFILES {
        for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            for scenario in [
                Scenario::Steady,
                Scenario::Drift,
                Scenario::NegativeDrift,
                Scenario::Intermittent,
            ] {
                for request in [
                    StreamRequest::Capture(FrameCount::new(19).unwrap()),
                    StreamRequest::Playback(&source),
                    StreamRequest::Duplex(&source),
                ] {
                    let mut device = SimulatedM4::with_profile(profile, scenario);
                    let output =
                        run_stream(&mut device, SampleRate::new(hz).unwrap(), request, |_| {
                            false
                        })
                        .unwrap_or_else(|error| {
                            panic!("{profile:?} {hz} {scenario:?} {request:?}: {error}")
                        });
                    if matches!(request, StreamRequest::Playback(_)) {
                        assert!(output.audio.is_empty());
                    } else {
                        assert_eq!(output.audio, source);
                    }
                    if !matches!(request, StreamRequest::Capture(_)) {
                        assert_eq!(&device.played()[..source.len()], source);
                        assert!(device.played()[source.len()..]
                            .iter()
                            .all(|byte| *byte == 0));
                    }
                    assert!(device.is_attached());
                    assert_eq!(device.clock(), 44_100);
                    if profile == DeviceProfile::OriginalSynthetic {
                        assert!(device.elapsed_us() >= 4_000_000);
                    } else {
                        assert!(device
                            .control_times()
                            .windows(2)
                            .all(|times| times[1] - times[0] >= 1000));
                    }
                }
            }
        }
    }
}

#[test]
fn simulator_refuses_back_to_back_class_requests_at_the_exact_deadline() {
    for profile in [
        DeviceProfile::Recorded,
        DeviceProfile::AtmelTemplate,
        DeviceProfile::NxpTemplate,
    ] {
        let mut device = SimulatedM4::with_profile(profile, Scenario::Steady);
        assert!(device.control_times().is_empty());
        let layout = Layout::parse(device.descriptors(), "M4").unwrap();
        device.takeover().unwrap();
        assert_eq!(device.control(layout.read_clock(), &mut [0; 4]), Ok(4));
        assert_eq!(device.control_times(), &[0]);
        device.wait_us(999);
        assert_eq!(
            device.control(layout.read_clock(), &mut [0; 4]),
            Err(TransportError::NotReady)
        );
        assert_eq!(device.set_interface(2, 1), Err(TransportError::NotReady));
        device.wait_us(1);
        assert_eq!(device.control(layout.read_clock(), &mut [0; 4]), Ok(4));
        assert_eq!(device.control_times(), &[0, 1000]);
    }
}

#[test]
fn control_trace_records_every_request_and_the_final_recovery_wait() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    run_stream(
        &mut device,
        SampleRate::new(48_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(1).unwrap()),
        |_| false,
    )
    .unwrap();
    assert_eq!(
        device.control_times(),
        &[0, 1000, 2000, 3000, 4000, 5000, 6125, 7125]
    );
    assert_eq!(device.elapsed_us(), 8125);
}

#[test]
fn original_boot_delay_uses_elapsed_time_and_has_an_exact_boundary() {
    let mut device = SimulatedM4::with_profile(DeviceProfile::OriginalSynthetic, Scenario::Steady);
    let layout = Layout::parse(device.descriptors(), "M4").unwrap();
    device.takeover().unwrap();
    device.wait_us(3_999_999);
    assert_eq!(
        device.control(layout.read_clock(), &mut [0; 4]),
        Err(TransportError::NotReady)
    );
    device.wait_us(1);
    assert_eq!(device.control(layout.read_clock(), &mut [0; 4]), Ok(4));
    assert_eq!(device.control(layout.read_clock(), &mut [0; 4]), Ok(4));
    device.release().unwrap();
    device.attach().unwrap();
    let before = device.elapsed_us();
    run_stream(
        &mut device,
        SampleRate::new(44_100).unwrap(),
        StreamRequest::Capture(FrameCount::new(1).unwrap()),
        |_| false,
    )
    .unwrap();
    assert_eq!(device.elapsed_us() - before, 125);
}

#[test]
fn unchanged_clock_never_writes_even_on_cancellation_or_unwinding() {
    let source = vec![0; 1600];
    for request in [
        StreamRequest::Capture(FrameCount::new(100).unwrap()),
        StreamRequest::Playback(&source),
        StreamRequest::Duplex(&source),
    ] {
        for stop in 0..3 {
            let mut device = SimulatedM4::new(Scenario::Steady);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_stream(
                    &mut device,
                    SampleRate::new(44_100).unwrap(),
                    request,
                    |packets| {
                        assert!(!(stop == 2 && packets == 1), "cancel callback unwound");
                        stop == 1 && packets == 1
                    },
                )
            }));
            match stop {
                0 => {
                    result.unwrap().unwrap();
                }
                1 => {
                    assert!(result.unwrap().is_err());
                }
                2 => {
                    assert!(result.is_err());
                }
                _ => unreachable!(),
            }
            assert!(!device.events().contains(&Operation::SetClock));
            assert!(!device.events().contains(&Operation::RestoreClock));
            assert!(device.is_attached());
            assert_eq!(device.clock(), 44_100);
        }
    }
}

#[test]
fn failed_controls_still_wait_before_recovery_and_report_the_primary_error() {
    for operation in [
        Operation::ReadClock,
        Operation::ReadRanges,
        Operation::SetClock,
        Operation::VerifyClock,
        Operation::ClockValidity,
        Operation::RestoreClock,
        Operation::VerifyRestoreClock,
    ] {
        let mut device = SimulatedM4::new(Scenario::Steady);
        device.fail_once(operation);
        let error = run_stream(
            &mut device,
            SampleRate::new(48_000).unwrap(),
            StreamRequest::Capture(FrameCount::new(1).unwrap()),
            |_| false,
        )
        .unwrap_err();
        assert!(error
            .primary
            .iter()
            .chain(error.recovery.iter())
            .any(|failure| failure.operation == operation));
        assert!(device.is_attached());
        assert!(device.elapsed_us() >= device.control_times().last().unwrap() + 1000);
        assert!(device
            .control_times()
            .windows(2)
            .all(|times| times[1] - times[0] >= 1000));
    }
}

#[test]
fn firmware_profiles_preserve_faults_and_recovery_across_both_capture_addresses() {
    for profile in PROFILES {
        for scenario in [
            Scenario::Disconnect,
            Scenario::Malformed,
            Scenario::Stalled,
            Scenario::Timeout,
            Scenario::ShortControl,
            Scenario::UnsupportedRate,
            Scenario::MalformedRanges,
            Scenario::InvalidClock,
            Scenario::ClockMismatch,
        ] {
            let mut device = SimulatedM4::with_profile(profile, scenario);
            let source = vec![0; 1600];
            let error = run_stream(
                &mut device,
                SampleRate::new(48_000).unwrap(),
                StreamRequest::Duplex(&source),
                |_| false,
            )
            .unwrap_err();
            assert!(error.primary.is_some(), "{profile:?} {scenario:?}");
            if scenario == Scenario::Disconnect {
                assert!(!device.is_attached());
                assert!(!error.recovery.is_empty());
            } else {
                assert!(device.is_attached(), "{profile:?} {scenario:?}: {error}");
                assert_eq!(device.clock(), 44_100);
                assert!(
                    error.recovery.is_empty(),
                    "{profile:?} {scenario:?}: {error}"
                );
            }
        }
    }
}
