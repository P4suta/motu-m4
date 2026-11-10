use crate::{
    run_stream, AudioTransport, DeviceProfile, FirmwareM4, FrameCount, SampleRate, Scenario,
    SimulatedM4, StreamRequest, TransportError,
};
use motu_m4_firmware::{ControlError, FirmwareControl, FirmwareFamily};

enum Fault {
    None,
    ShortInitial,
    BadInitial,
    ResponseBytes,
    ResponseLength,
    Execution,
    PrimaryThenRecovery,
}

struct StubControl {
    device: SimulatedM4,
    calls: u64,
    fault: Fault,
    failed_sets: usize,
}

impl StubControl {
    fn new(fault: Fault) -> Self {
        let mut device = SimulatedM4::with_profile(DeviceProfile::NxpTemplate, Scenario::Steady)
            .with_initial_rate(SampleRate::new(48_000).unwrap());
        device.takeover().unwrap();
        Self {
            device,
            calls: 0,
            fault,
            failed_sets: 0,
        }
    }
}

impl FirmwareControl for StubControl {
    fn family(&self) -> FirmwareFamily {
        FirmwareFamily::Nxp
    }
    fn instructions(&self) -> u64 {
        self.calls
    }
    fn control(&mut self, setup: [u8; 8], data: &mut [u8]) -> Result<usize, ControlError> {
        self.calls += 1;
        if self.calls == 1 {
            data.copy_from_slice(&48_000_u32.to_le_bytes());
            return match self.fault {
                Fault::ShortInitial => Ok(3),
                Fault::BadInitial => {
                    data.fill(0);
                    Ok(4)
                }
                _ => Ok(4),
            };
        }
        if matches!(self.fault, Fault::Execution) {
            return Err(ControlError::Stall);
        }
        if matches!(self.fault, Fault::PrimaryThenRecovery) && setup[0] == 0x21 {
            self.failed_sets += 1;
            return Err(if self.failed_sets == 1 {
                ControlError::Machine(motu_m4_firmware::Error::Budget)
            } else {
                ControlError::Stall
            });
        }
        let layout = motu_m4_core::Layout::parse(self.device.descriptors(), "M4").unwrap();
        let setup_builder = match setup {
            [0xa1, 2, 0, 1, 0, 2, low, high] => {
                layout.clock_ranges(u16::from_le_bytes([low, high]))
            }
            [0xa1, 1, 0, 1, 0, 2, 4, 0] => layout.read_clock(),
            [0x21, 1, 0, 1, 0, 2, 4, 0] => layout.set_clock(),
            [0xa1, 1, 0, 2, 0, 2, 1, 0] => layout.clock_validity().unwrap(),
            _ => panic!("unsupported stub request {setup:?}"),
        };
        assert_eq!(setup_builder.bytes(), setup);
        self.device.wait_us(1000);
        let count = self.device.control(setup_builder, data).unwrap();
        match self.fault {
            Fault::ResponseBytes => data[0] ^= 1,
            Fault::ResponseLength => return Ok(count - 1),
            _ => {}
        }
        Ok(count)
    }
}

#[test]
fn transport_refuses_corrupt_replies_and_preserves_causal_diagnostics() {
    for fault in [Fault::ShortInitial, Fault::BadInitial] {
        assert!(FirmwareM4::with_control(StubControl::new(fault)).is_err());
    }
    for fault in [
        Fault::ResponseBytes,
        Fault::ResponseLength,
        Fault::Execution,
    ] {
        let mut device = FirmwareM4::with_control(StubControl::new(fault)).unwrap();
        device.takeover().unwrap();
        let layout = motu_m4_core::Layout::parse(device.descriptors(), "M4").unwrap();
        let mut bytes = vec![0; 74];
        assert_eq!(
            device.control(layout.clock_ranges(74), &mut bytes),
            Err(TransportError::Firmware)
        );
        assert!(device.control_instructions() > 1);
        match device.firmware_failure().unwrap() {
            ControlError::Mismatch {
                setup,
                expected,
                actual,
                expected_count,
                actual_count,
            } => {
                assert_eq!(*setup, [0xa1, 2, 0, 1, 0, 2, 74, 0]);
                assert!(*actual_count != *expected_count || actual != expected);
                assert_eq!(*expected_count, 74);
                assert_eq!(actual, &bytes);
            }
            ControlError::Stall => {}
            error => panic!("{error:?}"),
        }
        device.release().unwrap();
        device.attach().unwrap();
        assert!(device.simulated().is_attached());
    }
}

#[test]
fn injected_control_port_runs_a_complete_session_and_restores_ownership() {
    let mut device = FirmwareM4::with_control(StubControl::new(Fault::None)).unwrap();
    assert_eq!(device.elapsed_us(), 0);
    device.wait_us(17);
    assert_eq!(device.elapsed_us(), 17);
    device.wait_us(23);
    assert_eq!(device.elapsed_us(), 40);
    let result = run_stream(
        &mut device,
        SampleRate::new(192_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(19).unwrap()),
        |_| false,
    )
    .unwrap();
    assert_eq!(result.audio.len(), 19 * 16);
    assert!(device.simulated().is_attached());
    assert_eq!(device.simulated().clock(), 48_000);
    assert!(device.firmware_failure().is_none());
    assert_eq!(device.elapsed_us(), device.simulated().elapsed_us());
}

#[test]
fn firmware_errors_explain_identity_and_execution_context() {
    assert!(ControlError::Identity.to_string().contains("SHA-256"));
    assert!(ControlError::Request.to_string().contains("buffer"));
    assert!(ControlError::Descriptor.to_string().contains("endpoint"));
}

#[test]
fn cleanup_failures_preserve_the_primary_firmware_error() {
    let mut device =
        FirmwareM4::with_control(StubControl::new(Fault::PrimaryThenRecovery)).unwrap();
    let result = run_stream(
        &mut device,
        SampleRate::new(192_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(19).unwrap()),
        |_| false,
    )
    .unwrap_err();
    assert_eq!(
        result.primary.unwrap().operation,
        crate::Operation::SetClock
    );
    assert!(!result.recovery.is_empty());
    assert!(matches!(
        device.firmware_failure(),
        Some(ControlError::Machine(motu_m4_firmware::Error::Budget))
    ));
    assert!(device.simulated().is_attached());
    run_stream(
        &mut device,
        SampleRate::new(192_000).unwrap(),
        StreamRequest::Capture(FrameCount::new(19).unwrap()),
        |_| false,
    )
    .unwrap_err();
    assert!(matches!(
        device.firmware_failure(),
        Some(ControlError::Stall)
    ));
}
