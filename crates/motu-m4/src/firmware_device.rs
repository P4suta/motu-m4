use crate::{AudioTransport, DeviceProfile, Scenario, SimulatedM4, TransportError};
use motu_m4_core::ControlSetup;
use motu_m4_firmware::{ControlError, FirmwareAudioControl, FirmwareControl, FirmwareFamily};
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transport_tests;

pub struct FirmwareM4<C = FirmwareAudioControl> {
    simulated: SimulatedM4,
    control: C,
    failure: Option<ControlError>,
}

impl FirmwareM4 {
    pub fn new(firmware: &[u8]) -> Result<Self, ControlError> {
        Self::with_control(FirmwareAudioControl::new(firmware)?)
    }
}

impl<C: FirmwareControl> FirmwareM4<C> {
    pub fn with_control(mut control: C) -> Result<Self, ControlError> {
        let mut bytes = [0; 4];
        if control.control([0xa1, 1, 0, 1, 0, 2, 4, 0], &mut bytes)? != 4 {
            return Err(ControlError::Descriptor);
        }
        let rate = motu_m4_core::SampleRate::new(u32::from_le_bytes(bytes))
            .map_err(|_| ControlError::Descriptor)?;
        Ok(Self {
            simulated: SimulatedM4::with_profile(
                match control.family() {
                    FirmwareFamily::Atmel => DeviceProfile::AtmelTemplate,
                    FirmwareFamily::Nxp => DeviceProfile::NxpTemplate,
                },
                Scenario::Steady,
            )
            .with_initial_rate(rate),
            control,
            failure: None,
        })
    }

    pub fn simulated(&self) -> &SimulatedM4 {
        &self.simulated
    }
    pub fn control_instructions(&self) -> u64 {
        self.control.instructions()
    }
    pub fn firmware_failure(&self) -> Option<&ControlError> {
        self.failure.as_ref()
    }
}

impl<C: FirmwareControl> AudioTransport for FirmwareM4<C> {
    fn descriptors(&self) -> &[u8] {
        self.simulated.descriptors()
    }
    fn product(&self) -> &str {
        self.simulated.product()
    }
    fn elapsed_us(&self) -> u64 {
        self.simulated.elapsed_us()
    }
    fn wait_us(&mut self, microseconds: u32) {
        self.simulated.wait_us(microseconds);
    }
    fn takeover(&mut self) -> Result<(), TransportError> {
        self.simulated.takeover()?;
        self.failure = None;
        Ok(())
    }
    fn control(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<usize, TransportError> {
        let mut expected = data.to_vec();
        let expected_count = self.simulated.control(setup, &mut expected)?;
        let count = self.control.control(setup.bytes(), data).map_err(|error| {
            self.failure.get_or_insert(error);
            TransportError::Firmware
        })?;
        if count != expected_count || data != expected {
            self.failure.get_or_insert(ControlError::Mismatch {
                setup: setup.bytes(),
                expected,
                actual: data.to_vec(),
                expected_count,
                actual_count: count,
            });
            return Err(TransportError::Firmware);
        }
        Ok(count)
    }
    fn set_interface(&mut self, interface: u8, alternate: u8) -> Result<(), TransportError> {
        self.simulated.set_interface(interface, alternate)
    }
    fn capture_packet(&mut self, endpoint: u8, data: &mut [u8]) -> Result<usize, TransportError> {
        self.simulated.capture_packet(endpoint, data)
    }
    fn playback_packet(&mut self, endpoint: u8, data: &[u8]) -> Result<(), TransportError> {
        self.simulated.playback_packet(endpoint, data)
    }
    fn release(&mut self) -> Result<(), TransportError> {
        self.simulated.release()
    }
    fn attach(&mut self) -> Result<(), TransportError> {
        self.simulated.attach()
    }
}
