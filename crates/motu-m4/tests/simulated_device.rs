use motu_m4::{AudioTransport, Scenario, SimulatedM4, TransportError};
use motu_m4_core::Layout;

#[test]
fn the_device_enforces_ownership_clock_requests_and_interface_numbers() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let layout = Layout::parse(device.descriptors(), "M4").unwrap();
    assert_eq!(device.set_interface(2, 1), Err(TransportError::NotOwned));
    device.takeover().unwrap();
    assert_eq!(device.takeover(), Err(TransportError::InvalidRequest));
    assert_eq!(
        device.control(layout.read_clock(), &mut [0; 3]),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(
        device.control(layout.set_clock(), &mut 12_345u32.to_le_bytes()),
        Err(TransportError::InvalidRequest)
    );
    device.wait_us(1000);
    assert_eq!(
        device.set_interface(9, 1),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(
        device.set_interface(1, 1),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(
        device.capture_packet(0x83, &mut [0; 405]),
        Err(TransportError::InvalidRequest)
    );
    device.release().unwrap();
    device.attach().unwrap();
    assert_eq!(device.attach(), Err(TransportError::InvalidRequest));
}

#[test]
fn the_device_requires_fresh_feedback_for_each_playback_packet() {
    let mut device = SimulatedM4::new(Scenario::Drift);
    let layout = Layout::parse(device.descriptors(), "M4").unwrap();
    device.takeover().unwrap();
    device
        .control(layout.set_clock(), &mut 48_000u32.to_le_bytes())
        .unwrap();
    device.wait_us(1000);
    device.set_interface(2, 1).unwrap();
    device.set_interface(1, 1).unwrap();
    assert_eq!(
        device.playback_packet(2, &[0; 80]),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(device.capture_packet(0x83, &mut [0; 405]), Ok(80));
    assert_eq!(
        device.playback_packet(2, &[0; 96]),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(
        device.playback_packet(2, &[1; 80]),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(device.playback_packet(2, &[0; 80]), Ok(()));
    assert_eq!(
        device.playback_packet(2, &[0; 80]),
        Err(TransportError::InvalidRequest)
    );
}

#[test]
fn endpoint_and_clock_checks_apply_independently() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let layout = Layout::parse(device.descriptors(), "M4").unwrap();
    device.takeover().unwrap();
    device.set_interface(2, 1).unwrap();
    device.set_interface(1, 1).unwrap();
    assert_eq!(device.capture_packet(0x83, &mut [0; 405]), Ok(80));
    assert_eq!(
        device.playback_packet(2, &[0; 96]),
        Err(TransportError::InvalidRequest)
    );
    device
        .control(layout.set_clock(), &mut 48_000u32.to_le_bytes())
        .unwrap();
    device.wait_us(1000);
    assert_eq!(
        device.capture_packet(0x82, &mut [0; 405]),
        Err(TransportError::InvalidRequest)
    );
    assert_eq!(
        device.capture_packet(0x83, &mut [0; 1]),
        Err(TransportError::InvalidRequest)
    );
    device.capture_packet(0x83, &mut [0; 405]).unwrap();
    assert_eq!(
        device.playback_packet(3, &[0; 96]),
        Err(TransportError::InvalidRequest)
    );
    device.playback_packet(2, &[0; 96]).unwrap();
    device.set_interface(1, 0).unwrap();
    device.capture_packet(0x83, &mut [0; 405]).unwrap();
    assert_eq!(
        device.playback_packet(2, &[0; 96]),
        Err(TransportError::InvalidRequest)
    );
}

#[test]
fn interface_stop_and_restart_states_follow_the_device_contract() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    device.takeover().unwrap();
    for (interface, alternate) in [
        (2, 1),
        (1, 1),
        (2, 0),
        (2, 1),
        (2, 0),
        (1, 0),
        (1, 0),
        (2, 0),
        (2, 1),
        (1, 1),
        (1, 0),
        (2, 0),
    ] {
        device.set_interface(interface, alternate).unwrap();
    }
    device.release().unwrap();
    device.attach().unwrap();
    assert!(device.is_attached());
}

#[test]
fn scripted_packet_faults_have_exact_boundaries() {
    for scenario in [Scenario::Malformed, Scenario::Disconnect, Scenario::Stalled] {
        let mut device = SimulatedM4::new(scenario);
        let layout = Layout::parse(device.descriptors(), "M4").unwrap();
        device.takeover().unwrap();
        device
            .control(layout.set_clock(), &mut 48_000u32.to_le_bytes())
            .unwrap();
        device.wait_us(1000);
        device.set_interface(2, 1).unwrap();
        let mut buffer = [0; 405];
        match scenario {
            Scenario::Malformed => assert_eq!(device.capture_packet(0x83, &mut buffer), Ok(95)),
            Scenario::Disconnect => {
                assert_eq!(device.capture_packet(0x83, &mut buffer), Ok(96));
                assert_eq!(
                    device.capture_packet(0x83, &mut buffer),
                    Err(TransportError::Disconnected)
                );
            }
            Scenario::Stalled => {
                for _ in 0..32 {
                    assert_eq!(device.capture_packet(0x83, &mut buffer), Ok(0));
                }
                assert_eq!(
                    device.capture_packet(0x83, &mut buffer),
                    Err(TransportError::Timeout)
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn control_buffers_must_match_the_setup_length_before_any_state_transition() {
    let mut device = SimulatedM4::new(Scenario::Steady);
    let layout = Layout::parse(device.descriptors(), "M4").unwrap();
    device.takeover().unwrap();
    let before = device.events().to_vec();
    for (setup, expected) in [
        (layout.read_clock(), 4),
        (layout.set_clock(), 4),
        (layout.clock_validity().unwrap(), 1),
        (layout.clock_ranges(2), 2),
        (layout.clock_ranges(74), 74),
    ] {
        for length in [0, 1, 2, 3, 4, 5, 74] {
            if length == expected {
                continue;
            }
            let mut buffer = vec![0; length];
            assert_eq!(
                device.control(setup, &mut buffer),
                Err(TransportError::InvalidRequest),
                "setup {:?} length {length}",
                setup.bytes()
            );
            assert_eq!(device.events(), before);
            assert_eq!(device.clock(), 44_100);
        }
    }
}

#[test]
fn a_malformed_range_scenario_changes_only_the_full_parameter_block() {
    let mut device = SimulatedM4::new(Scenario::MalformedRanges);
    let layout = Layout::parse(device.descriptors(), "M4").unwrap();
    device.takeover().unwrap();
    let mut header = [0; 2];
    assert_eq!(device.control(layout.clock_ranges(2), &mut header), Ok(2));
    assert_eq!(header, [6, 0]);
    device.wait_us(1000);
    let mut full = [0; 74];
    assert_eq!(device.control(layout.clock_ranges(74), &mut full), Ok(74));
    assert_eq!(&full[..2], [0, 0]);
}

#[test]
fn virtual_clock_faults_preserve_their_declared_packet_timeline() {
    for (scenario, prefix, total) in [
        (Scenario::Drift, [80, 112, 96, 96], 12_289),
        (Scenario::NegativeDrift, [112, 80, 96, 96], 12_287),
        (Scenario::Intermittent, [0, 96, 96, 96], 11_562),
    ] {
        let mut device = SimulatedM4::new(scenario);
        let layout = Layout::parse(device.descriptors(), "M4").unwrap();
        device.takeover().unwrap();
        device
            .control(layout.set_clock(), &mut 48_000u32.to_le_bytes())
            .unwrap();
        device.wait_us(1000);
        device.set_interface(2, 1).unwrap();
        let mut frames = 0;
        for packet in 0..2048 {
            let length = device.capture_packet(0x83, &mut [0; 405]).unwrap();
            if let Some(expected) = prefix.get(packet) {
                assert_eq!(length, *expected, "{scenario:?} interval {packet}");
            }
            if scenario == Scenario::Intermittent {
                assert_eq!(length, if packet % 17 == 0 { 0 } else { 96 });
            }
            frames += length / 16;
        }
        assert_eq!(frames, total, "{scenario:?}");
    }
}
