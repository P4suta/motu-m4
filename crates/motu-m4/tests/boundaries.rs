use motu_m4::{
    run_stream, AudioTransport, FailureCause, FrameCount, Operation, SampleRate, Scenario,
    SimulatedM4, StreamRequest, TransportError,
};
use motu_m4_core::{ControlSetup, Error};

struct Stub {
    device: SimulatedM4,
    descriptors: Vec<u8>,
    fault: Option<([u8; 8], usize, Vec<u8>, usize)>,
    matches: usize,
}

impl Stub {
    fn new() -> Self {
        let device = SimulatedM4::new(Scenario::Steady);
        Self {
            descriptors: device.descriptors().to_vec(),
            device,
            fault: None,
            matches: 0,
        }
    }
}

impl AudioTransport for Stub {
    fn descriptors(&self) -> &[u8] {
        &self.descriptors
    }
    fn product(&self) -> &str {
        "M4"
    }
    fn elapsed_us(&self) -> u64 {
        self.device.elapsed_us()
    }
    fn wait_us(&mut self, microseconds: u32) {
        self.device.wait_us(microseconds);
    }
    fn takeover(&mut self) -> Result<(), TransportError> {
        self.device.takeover()
    }
    fn control(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<usize, TransportError> {
        let actual = self.device.control(setup, data)?;
        if let Some((request, nth, bytes, result)) = &self.fault {
            if setup.bytes() == *request {
                self.matches += 1;
                if self.matches == *nth {
                    if bytes.len() == data.len() {
                        data.copy_from_slice(bytes);
                    }
                    return Ok(*result);
                }
            }
        }
        Ok(actual)
    }
    fn set_interface(&mut self, interface: u8, alternate: u8) -> Result<(), TransportError> {
        self.device.set_interface(interface, alternate)
    }
    fn capture_packet(&mut self, endpoint: u8, data: &mut [u8]) -> Result<usize, TransportError> {
        self.device.capture_packet(endpoint, data)
    }
    fn playback_packet(&mut self, endpoint: u8, data: &[u8]) -> Result<(), TransportError> {
        self.device.playback_packet(endpoint, data)
    }
    fn release(&mut self) -> Result<(), TransportError> {
        self.device.release()
    }
    fn attach(&mut self) -> Result<(), TransportError> {
        self.device.attach()
    }
}

#[test]
fn every_control_transfer_requires_the_exact_response_length() {
    for (setup, occurrence, length) in [
        ([0xa1, 2, 0, 1, 0, 2, 2, 0], 1, 2),
        ([0xa1, 2, 0, 1, 0, 2, 74, 0], 1, 74),
        ([0xa1, 1, 0, 1, 0, 2, 4, 0], 1, 4),
        ([0xa1, 1, 0, 1, 0, 2, 4, 0], 2, 4),
        ([0xa1, 1, 0, 1, 0, 2, 4, 0], 3, 4),
        ([0x21, 1, 0, 1, 0, 2, 4, 0], 1, 4),
        ([0x21, 1, 0, 1, 0, 2, 4, 0], 2, 4),
        ([0xa1, 1, 0, 2, 0, 2, 1, 0], 1, 1),
    ] {
        for actual in [0, length - 1, length + 1, usize::MAX] {
            let mut stub = Stub::new();
            stub.fault = Some((setup, occurrence, Vec::new(), actual));
            let error = run_stream(
                &mut stub,
                SampleRate::new(48_000).unwrap(),
                StreamRequest::Capture(FrameCount::new(1).unwrap()),
                |_| false,
            )
            .unwrap_err();
            assert!(
                error
                    .primary
                    .iter()
                    .chain(&error.recovery)
                    .any(|failure| failure.cause == FailureCause::ShortControl),
                "{setup:?} occurrence {occurrence} actual {actual}: {error:?}"
            );
            assert!(stub.device.is_attached());
            assert_eq!(stub.device.clock(), 44_100);
        }
    }
}

#[test]
fn restoration_must_read_back_the_original_rate() {
    let mut stub = Stub::new();
    stub.fault = Some((
        [0xa1, 1, 0, 1, 0, 2, 4, 0],
        3,
        48_000u32.to_le_bytes().to_vec(),
        4,
    ));
    let error = run_stream(
        &mut stub,
        SampleRate::new(48_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(1).unwrap()),
        |_| false,
    )
    .unwrap_err();
    assert!(error.primary.is_none());
    assert_eq!(error.recovery.len(), 1);
    assert_eq!(error.recovery[0].operation, Operation::VerifyRestoreClock);
    assert_eq!(error.recovery[0].cause, FailureCause::ClockMismatch);
    assert!(stub.device.is_attached());
}

#[test]
fn insufficient_endpoint_bandwidth_and_invalid_descriptors_never_take_ownership() {
    for offset in [178, 233] {
        let mut stub = Stub::new();
        stub.descriptors[offset..offset + 2].copy_from_slice(&383u16.to_le_bytes());
        let error = run_stream(
            &mut stub,
            SampleRate::new(192_000).unwrap(),
            StreamRequest::Capture(FrameCount::new(1).unwrap()),
            |_| false,
        )
        .unwrap_err();
        assert_eq!(
            error.primary.unwrap().cause,
            FailureCause::Protocol(Error::Packet)
        );
        assert!(stub.device.events().is_empty());
    }
    let mut stub = Stub::new();
    stub.descriptors[0] = 0;
    let error = run_stream(
        &mut stub,
        SampleRate::new(48_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(1).unwrap()),
        |_| false,
    )
    .unwrap_err();
    assert_eq!(error.primary.unwrap().operation, Operation::Validate);
    assert!(stub.device.events().is_empty());
    let mut exact = Stub::new();
    exact.descriptors[233..235].copy_from_slice(&400u16.to_le_bytes());
    run_stream(
        &mut exact,
        SampleRate::new(192_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(1).unwrap()),
        |_| false,
    )
    .unwrap();
    assert!(exact.device.is_attached());
}

#[test]
fn absent_clock_validity_is_not_queried_and_reserved_reply_values_fail() {
    let mut stub = Stub::new();
    stub.descriptors[58] = 3;
    run_stream(
        &mut stub,
        SampleRate::new(48_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(1).unwrap()),
        |_| false,
    )
    .unwrap();
    assert!(!stub.device.events().contains(&Operation::ClockValidity));
    for value in [0, 2, 255] {
        let mut stub = Stub::new();
        stub.fault = Some(([0xa1, 1, 0, 2, 0, 2, 1, 0], 1, vec![value], 1));
        let error = run_stream(
            &mut stub,
            SampleRate::new(48_000).unwrap(),
            StreamRequest::Capture(FrameCount::new(1).unwrap()),
            |_| false,
        )
        .unwrap_err();
        assert_eq!(error.primary.unwrap().cause, FailureCause::InvalidClock);
        assert!(stub.device.is_attached());
    }
}

#[test]
fn an_invalid_original_clock_is_never_saved_as_a_restoration_target() {
    for previous in [0u32, 12_345, u32::MAX] {
        let mut stub = Stub::new();
        stub.fault = Some((
            [0xa1, 1, 0, 1, 0, 2, 4, 0],
            1,
            previous.to_le_bytes().to_vec(),
            4,
        ));
        let error = run_stream(
            &mut stub,
            SampleRate::new(48_000).unwrap(),
            StreamRequest::Capture(FrameCount::new(1).unwrap()),
            |_| false,
        )
        .unwrap_err();
        assert_eq!(error.primary.unwrap().operation, Operation::ReadClock);
        assert!(error.recovery.is_empty());
        assert!(!stub.device.events().contains(&Operation::SetClock));
        assert!(!stub.device.events().contains(&Operation::RestoreClock));
        assert!(stub.device.is_attached());
    }
}
