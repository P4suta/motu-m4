#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

#[cfg(kani)]
mod proofs;

mod audio;
mod clocks;
mod firmware;
mod topology;
pub use audio::{
    native_recording_frames, pcm_from_float, pcm_from_signed, AudioChannels, AudioFormatError,
    AudioInputs, AudioOutputs, RecordingFrames,
};
pub use clocks::{ClockRangeCount, ClockRanges, RateRange, SampleRate};
pub use firmware::{FirmwareError, FirmwareImage, UsbDescriptorCandidate, MAX_FIRMWARE_BYTES};

pub const FRAME_BYTES: u16 = 16;
pub const SAMPLE_RATE: u32 = 48_000;
pub const MAX_FRAMES: u32 = 480_000;
const MIN_PROFILE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmSample(u32);

impl PcmSample {
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & 0xffff_ff00)
    }
    pub const fn to_le_bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    Complete,
    Attachment,
    Claim,
    Clock(u32),
    Capture(u32),
    Duplex(u32),
    CaptureUnchanged,
    DuplexUnchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    StopPlayback,
    StopCapture,
    RestoreClock(u32),
    Release,
    Attach,
}

impl Recovery {
    pub fn next(self) -> Option<(RecoveryAction, Self)> {
        match self {
            Self::Complete => None,
            Self::Attachment => Some((RecoveryAction::Attach, Self::Complete)),
            Self::Claim => Some((RecoveryAction::Release, Self::Attachment)),
            Self::Clock(rate) => Some((RecoveryAction::RestoreClock(rate), Self::Claim)),
            Self::Capture(rate) => Some((RecoveryAction::StopCapture, Self::Clock(rate))),
            Self::Duplex(rate) => Some((RecoveryAction::StopPlayback, Self::Capture(rate))),
            Self::CaptureUnchanged => Some((RecoveryAction::StopCapture, Self::Claim)),
            Self::DuplexUnchanged => Some((RecoveryAction::StopPlayback, Self::CaptureUnchanged)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    WrongDevice,
    Descriptor,
    Layout,
    Packet,
    FrameLimit,
    Clock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    Original,
    Revised,
}

impl Hardware {
    pub const fn boot_delay_us(self) -> u32 {
        match self {
            Self::Original => 4_000_000,
            Self::Revised => 0,
        }
    }

    pub const fn control_delay_us(self) -> u32 {
        match self {
            Self::Original => 0,
            Self::Revised => 1_000,
        }
    }

    pub fn identify(vendor: u16, product: u16, name: &str) -> Result<Self, Error> {
        if vendor != 0x07fd || name != "M4" {
            return Err(Error::WrongDevice);
        }
        match product {
            0x0008 => Ok(Self::Original),
            0x000b => Ok(Self::Revised),
            _ => Err(Error::WrongDevice),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketLength(u16);

impl PacketLength {
    pub fn from_feedback(actual: u32, capture_max: u16, playback_max: u16) -> Result<Self, Error> {
        if actual > u32::from(capture_max)
            || actual > u32::from(playback_max)
            || actual % u32::from(FRAME_BYTES) != 0
        {
            return Err(Error::Packet);
        }
        u16::try_from(actual).map(Self).map_err(|_| Error::Packet)
    }

    pub const fn bytes(self) -> u16 {
        self.0
    }

    pub const fn frames(self) -> u16 {
        self.0 / FRAME_BYTES
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    interface: u8,
    address: u8,
    max_packet: u16,
    terminal: u8,
}

impl Endpoint {
    pub const fn interface(self) -> u8 {
        self.interface
    }
    pub const fn address(self) -> u8 {
        self.address
    }
    pub const fn max_packet(self) -> u16 {
        self.max_packet
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    hardware: Hardware,
    playback: Endpoint,
    capture: Endpoint,
    control_interface: u8,
    clock_id: u8,
    clock_validity: bool,
}

impl Layout {
    pub fn parse(bytes: &[u8], product: &str) -> Result<Self, Error> {
        profile_length(bytes.len())?;
        let (device, rest) = descriptor(bytes)?;
        if device.len() != 18 || field(device, 1)? != 1 || field(device, 17)? != 1 {
            return Err(Error::Descriptor);
        }
        let hardware = Hardware::identify(word(device, 8)?, word(device, 10)?, product)
            .map_err(|_| Error::Layout)?;
        let (configuration, _) = descriptor(rest)?;
        if configuration.len() != 9
            || field(configuration, 1)? != 2
            || usize::from(word(configuration, 2)?) != rest.len()
        {
            return Err(Error::Descriptor);
        }
        let mut input = rest;
        let mut interface = Interface::default();
        let mut control = None;
        let mut clock = None;
        let mut clock_validity = false;
        let mut selector = None;
        let mut playback = None;
        let mut capture = None;
        let mut topology = topology::AudioTopology::default();
        while !input.is_empty() {
            let (d, remaining) = descriptor(input)?;
            input = remaining;
            match field(d, 1)? {
                4 => {
                    if d.len() != 9 {
                        return Err(Error::Descriptor);
                    }
                    interface = Interface {
                        number: field(d, 2)?,
                        alternate: field(d, 3)?,
                        class: field(d, 5)?,
                        subclass: field(d, 6)?,
                        protocol: field(d, 7)?,
                        channels: false,
                        format: false,
                        terminal: 0,
                    };
                    if interface.is_control() && control.replace(interface.number).is_some() {
                        return Err(Error::Layout);
                    }
                }
                0x24 if interface.is_control() => {
                    topology.add(d)?;
                    match field(d, 2)? {
                        0x0a => {
                            if d.len() != 8
                                || field(d, 3)? == 0
                                || ![3, 7].contains(&field(d, 5)?)
                                || clock.replace(field(d, 3)?).is_some()
                            {
                                return Err(Error::Layout);
                            }
                            clock_validity = field(d, 5)? == 7;
                        }
                        0x0b => {
                            let source = field(d, 5)?;
                            if d.len() != 8
                                || field(d, 4)? != 1
                                || ![0, 1, 3].contains(&field(d, 6)?)
                                || selector.replace((field(d, 3)?, source)).is_some()
                            {
                                return Err(Error::Layout);
                            }
                        }
                        _ => {}
                    }
                }
                0x24 if interface.is_stream() => match field(d, 2)? {
                    1 => {
                        if d.len() != 16
                            || field(d, 5)? != 1
                            || field(d, 10)? != 4
                            || d.get(6..10) != Some(&[1, 0, 0, 0])
                            || interface.channels
                        {
                            return Err(Error::Layout);
                        }
                        interface.channels = true;
                        interface.terminal = field(d, 3)?;
                    }
                    2 => {
                        if d.len() != 6 || d.get(3..6) != Some(&[1, 4, 24]) || interface.format {
                            return Err(Error::Layout);
                        }
                        interface.format = true;
                    }
                    _ => return Err(Error::Layout),
                },
                5 if interface.is_stream() => {
                    if d.len() != 7 || !interface.channels || !interface.format || field(d, 6)? != 1
                    {
                        return Err(Error::Layout);
                    }
                    let address = field(d, 2)?;
                    let max_packet = word(d, 4)?;
                    if address & 0x70 != 0
                        || address & 15 == 0
                        || !(16..=1024).contains(&max_packet)
                    {
                        return Err(Error::Layout);
                    }
                    let endpoint = Endpoint {
                        interface: interface.number,
                        address,
                        max_packet,
                        terminal: interface.terminal,
                    };
                    let slot = match (address & 0x80, field(d, 3)?) {
                        (0, 0x05) => &mut playback,
                        (0x80, 0x25) => &mut capture,
                        _ => return Err(Error::Layout),
                    };
                    if slot.replace(endpoint).is_some() {
                        return Err(Error::Layout);
                    }
                }
                _ => {}
            }
        }
        let playback = playback.ok_or(Error::Layout)?;
        let capture = capture.ok_or(Error::Layout)?;
        let clock_id = clock.ok_or(Error::Layout)?;
        let (selector_id, selector_source) = selector.ok_or(Error::Layout)?;
        if playback.interface == capture.interface || selector_source != clock_id {
            return Err(Error::Layout);
        }
        topology.check(clock_id, selector_id, playback.terminal, capture.terminal)?;
        Ok(Self {
            hardware,
            playback,
            capture,
            control_interface: control.ok_or(Error::Layout)?,
            clock_id,
            clock_validity,
        })
    }

    pub const fn hardware(self) -> Hardware {
        self.hardware
    }
    pub const fn playback(self) -> Endpoint {
        self.playback
    }
    pub const fn capture(self) -> Endpoint {
        self.capture
    }
    pub const fn control_interface(self) -> u8 {
        self.control_interface
    }
    pub const fn clock_id(self) -> u8 {
        self.clock_id
    }

    pub fn read_clock(self) -> ControlSetup {
        self.clock_request(0xa1)
    }
    pub fn set_clock(self) -> ControlSetup {
        self.clock_request(0x21)
    }

    pub fn clock_ranges(self, length: u16) -> ControlSetup {
        let [low, high] = length.to_le_bytes();
        self.control_setup([
            0xa1,
            2,
            0,
            1,
            self.control_interface,
            self.clock_id,
            low,
            high,
        ])
    }

    pub fn clock_validity(self) -> Option<ControlSetup> {
        if self.clock_validity {
            Some(self.control_setup([0xa1, 1, 0, 2, self.control_interface, self.clock_id, 1, 0]))
        } else {
            None
        }
    }

    fn clock_request(self, direction: u8) -> ControlSetup {
        self.control_setup([
            direction,
            1,
            0,
            1,
            self.control_interface,
            self.clock_id,
            4,
            0,
        ])
    }

    fn control_setup(self, bytes: [u8; 8]) -> ControlSetup {
        ControlSetup {
            bytes,
            post_delay_us: self.hardware.control_delay_us(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlSetup {
    bytes: [u8; 8],
    post_delay_us: u32,
}

impl ControlSetup {
    pub const fn bytes(self) -> [u8; 8] {
        self.bytes
    }
    pub const fn post_delay_us(self) -> u32 {
        self.post_delay_us
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameCount(u32);

impl FrameCount {
    pub fn new(frames: u32) -> Result<Self, Error> {
        if frames == 0 || frames > MAX_FRAMES {
            return Err(Error::FrameLimit);
        }
        Ok(Self(frames))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
    pub const fn bytes(self) -> u32 {
        self.0 * FRAME_BYTES as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorLength(u8);

impl DescriptorLength {
    pub fn new(length: u8, remaining: usize) -> Result<Self, Error> {
        if length < 2 || usize::from(length) > remaining {
            return Err(Error::Descriptor);
        }
        Ok(Self(length))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

fn descriptor(bytes: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let length = DescriptorLength::new(*bytes.first().ok_or(Error::Descriptor)?, bytes.len())?;
    bytes
        .split_at_checked(usize::from(length.get()))
        .ok_or(Error::Descriptor)
}

fn profile_length(length: usize) -> Result<(), Error> {
    if length < MIN_PROFILE_BYTES {
        return Err(Error::Descriptor);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{profile_length, Error};

    #[test]
    fn structural_profile_size_has_an_exact_boundary() {
        for length in [0, 1, 126, 127] {
            assert_eq!(profile_length(length), Err(Error::Descriptor));
        }
        for length in [128, 129, 393, usize::MAX] {
            assert_eq!(profile_length(length), Ok(()));
        }
    }
}

fn field(bytes: &[u8], offset: usize) -> Result<u8, Error> {
    bytes.get(offset).copied().ok_or(Error::Descriptor)
}

fn word(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes([
        field(bytes, offset)?,
        field(bytes, offset + 1)?,
    ]))
}

#[derive(Default)]
struct Interface {
    number: u8,
    alternate: u8,
    class: u8,
    subclass: u8,
    protocol: u8,
    channels: bool,
    format: bool,
    terminal: u8,
}

impl Interface {
    fn is_control(&self) -> bool {
        self.class == 1 && self.subclass == 1 && self.protocol == 0x20 && self.alternate == 0
    }
    fn is_stream(&self) -> bool {
        self.class == 1 && self.subclass == 2 && self.protocol == 0x20 && self.alternate == 1
    }
}
