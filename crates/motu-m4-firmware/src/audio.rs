use crate::{AtmelAudioControl, ControlError, NxpAudioControl};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareFamily {
    Atmel,
    Nxp,
}

pub trait FirmwareControl {
    fn family(&self) -> FirmwareFamily;
    fn instructions(&self) -> u64;
    fn control(&mut self, setup: [u8; 8], data: &mut [u8]) -> Result<usize, ControlError>;
}

pub enum FirmwareAudioControl {
    Atmel(AtmelAudioControl),
    Nxp(NxpAudioControl),
}

impl FirmwareAudioControl {
    pub fn new(bytes: &[u8]) -> Result<Self, ControlError> {
        match NxpAudioControl::new(bytes) {
            Ok(control) => Ok(Self::Nxp(control)),
            Err(ControlError::Identity) => AtmelAudioControl::new(bytes).map(Self::Atmel),
            Err(error) => Err(error),
        }
    }

    pub fn family(&self) -> FirmwareFamily {
        match self {
            Self::Atmel(_) => FirmwareFamily::Atmel,
            Self::Nxp(_) => FirmwareFamily::Nxp,
        }
    }

    pub fn instructions(&self) -> u64 {
        match self {
            Self::Atmel(control) => control.instructions(),
            Self::Nxp(control) => control.instructions(),
        }
    }

    pub fn packets(&self) -> &[Vec<u8>] {
        match self {
            Self::Atmel(control) => control.packets(),
            Self::Nxp(control) => control.packets(),
        }
    }

    pub fn control(&mut self, setup: [u8; 8], data: &mut [u8]) -> Result<usize, ControlError> {
        match self {
            Self::Atmel(control) => control.control(setup, data),
            Self::Nxp(control) => control.control(setup, data),
        }
    }
}

impl FirmwareControl for FirmwareAudioControl {
    fn family(&self) -> FirmwareFamily {
        self.family()
    }
    fn instructions(&self) -> u64 {
        self.instructions()
    }
    fn control(&mut self, setup: [u8; 8], data: &mut [u8]) -> Result<usize, ControlError> {
        self.control(setup, data)
    }
}
