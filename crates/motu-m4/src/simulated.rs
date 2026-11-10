use crate::{AudioTransport, Operation, TransportError};
use motu_m4_core::ControlSetup;
use std::collections::BTreeSet;

const DESCRIPTORS: &[u8] =
    include_bytes!("../../motu-m4-core/tests/fixtures/m4-usb-descriptors.bin");
const ATMEL: &[u8] =
    include_bytes!("../../motu-m4-core/tests/fixtures/m4-atmel-firmware-descriptors.bin");
const NXP: &[u8] =
    include_bytes!("../../motu-m4-core/tests/fixtures/m4-nxp-firmware-descriptors.bin");
const CLOCK_RANGES: &[u8] =
    include_bytes!("../../motu-m4-core/tests/fixtures/m-series-clock-ranges.bin");
const ORIGINAL: [u8; 393] = {
    let mut bytes = *include_bytes!("../../motu-m4-core/tests/fixtures/m4-usb-descriptors.bin");
    bytes[10] = 8;
    bytes
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceProfile {
    Recorded,
    AtmelTemplate,
    NxpTemplate,
    OriginalSynthetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Steady,
    Drift,
    NegativeDrift,
    Intermittent,
    Disconnect,
    Malformed,
    Stalled,
    Timeout,
    ShortControl,
    UnsupportedRate,
    MalformedRanges,
    InvalidClock,
    ClockMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Streams {
    Idle,
    Capture,
    Playback,
    Duplex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    Attached,
    Detached,
    Disconnected,
    Owned(Streams),
}

pub struct SimulatedM4 {
    profile: DeviceProfile,
    elapsed_us: u64,
    next_control_us: u64,
    control_times: Vec<u64>,
    scenario: Scenario,
    binding: Binding,
    clock: u32,
    clock_writes: usize,
    packets: usize,
    pending_frames: Option<usize>,
    frame: u32,
    played: Vec<u8>,
    events: Vec<Operation>,
    failures: BTreeSet<Operation>,
}

impl SimulatedM4 {
    pub fn new(scenario: Scenario) -> Self {
        Self::with_profile(DeviceProfile::Recorded, scenario)
    }
    pub fn with_profile(profile: DeviceProfile, scenario: Scenario) -> Self {
        Self {
            profile,
            elapsed_us: 0,
            next_control_us: 0,
            control_times: Vec::new(),
            scenario,
            binding: Binding::Attached,
            clock: 44_100,
            clock_writes: 0,
            packets: 0,
            pending_frames: None,
            frame: 0,
            played: Vec::new(),
            events: Vec::new(),
            failures: BTreeSet::new(),
        }
    }
    pub fn fail_once(&mut self, operation: Operation) {
        self.failures.insert(operation);
    }
    pub fn with_initial_rate(mut self, rate: motu_m4_core::SampleRate) -> Self {
        self.clock = rate.hz();
        self
    }
    pub fn is_attached(&self) -> bool {
        self.binding == Binding::Attached
    }
    pub fn clock(&self) -> u32 {
        self.clock
    }
    pub fn played(&self) -> &[u8] {
        &self.played
    }
    pub fn events(&self) -> &[Operation] {
        &self.events
    }
    pub fn control_times(&self) -> &[u64] {
        &self.control_times
    }

    fn ready(&self) -> Result<(), TransportError> {
        let boot_us = if self.profile == DeviceProfile::OriginalSynthetic {
            4_000_000
        } else {
            0
        };
        if self.elapsed_us < boot_us || self.elapsed_us < self.next_control_us {
            return Err(TransportError::NotReady);
        }
        Ok(())
    }

    fn event(&mut self, operation: Operation) -> Result<(), TransportError> {
        self.events.push(operation);
        if self.failures.remove(&operation) {
            return Err(TransportError::Injected);
        }
        Ok(())
    }
    fn owned(&self) -> Result<Streams, TransportError> {
        match self.binding {
            Binding::Owned(streams) => Ok(streams),
            Binding::Attached | Binding::Detached => Err(TransportError::NotOwned),
            Binding::Disconnected => Err(TransportError::Disconnected),
        }
    }
}

impl AudioTransport for SimulatedM4 {
    fn descriptors(&self) -> &[u8] {
        match self.profile {
            DeviceProfile::Recorded => DESCRIPTORS,
            DeviceProfile::AtmelTemplate => ATMEL,
            DeviceProfile::NxpTemplate => NXP,
            DeviceProfile::OriginalSynthetic => &ORIGINAL,
        }
    }
    fn product(&self) -> &str {
        "M4"
    }
    fn elapsed_us(&self) -> u64 {
        self.elapsed_us
    }
    fn wait_us(&mut self, microseconds: u32) {
        self.elapsed_us = self.elapsed_us.saturating_add(u64::from(microseconds));
    }
    fn takeover(&mut self) -> Result<(), TransportError> {
        self.event(Operation::Takeover)?;
        if self.binding != Binding::Attached {
            return Err(TransportError::InvalidRequest);
        }
        self.binding = Binding::Owned(Streams::Idle);
        self.clock_writes = 0;
        self.packets = 0;
        self.frame = 0;
        self.played.clear();
        self.pending_frames = None;
        Ok(())
    }
    fn control(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<usize, TransportError> {
        self.owned()?;
        let request = setup.bytes();
        let [_, _, _, _, _, _, low, high] = request;
        if data.len() != usize::from(u16::from_le_bytes([low, high])) {
            return Err(TransportError::InvalidRequest);
        }
        self.ready()?;
        self.control_times.push(self.elapsed_us);
        if self.profile != DeviceProfile::OriginalSynthetic {
            self.next_control_us = self.elapsed_us.saturating_add(1000);
        }
        match request {
            [0xa1, 2, 0, 1, 0, 2, _, _] => {
                self.event(Operation::ReadRanges)?;
                let response = if self.scenario == Scenario::UnsupportedRate {
                    [&[1, 0][..], &CLOCK_RANGES[2..14]].concat()
                } else {
                    CLOCK_RANGES.to_vec()
                };
                if data.len() != 2 && data.len() != response.len() {
                    return Err(TransportError::InvalidRequest);
                }
                data.copy_from_slice(
                    response
                        .get(..data.len())
                        .ok_or(TransportError::InvalidRequest)?,
                );
                if self.scenario == Scenario::MalformedRanges && data.len() > 2 {
                    let first = data.first_mut().ok_or(TransportError::InvalidRequest)?;
                    *first = 0;
                }
            }
            [0xa1, 1, 0, 1, 0, 2, 4, 0] => {
                let operation = match self.clock_writes {
                    0 => Operation::ReadClock,
                    1 => Operation::VerifyClock,
                    _ => Operation::VerifyRestoreClock,
                };
                self.event(operation)?;
                data.copy_from_slice(&self.clock.to_le_bytes());
            }
            [0xa1, 1, 0, 2, 0, 2, 1, 0] => {
                self.event(Operation::ClockValidity)?;
                data.copy_from_slice(&[u8::from(self.scenario != Scenario::InvalidClock)]);
            }
            [0x21, 1, 0, 1, 0, 2, 4, 0] => {
                let operation = if self.clock_writes == 0 {
                    Operation::SetClock
                } else {
                    Operation::RestoreClock
                };
                self.clock_writes = self
                    .clock_writes
                    .checked_add(1)
                    .ok_or(TransportError::InvalidRequest)?;
                self.event(operation)?;
                let bytes =
                    <[u8; 4]>::try_from(&*data).map_err(|_| TransportError::InvalidRequest)?;
                let rate = u32::from_le_bytes(bytes);
                if ![44_100, 48_000, 88_200, 96_000, 176_400, 192_000].contains(&rate) {
                    return Err(TransportError::InvalidRequest);
                }
                if self.scenario != Scenario::ClockMismatch || self.clock_writes != 1 {
                    self.clock = rate;
                }
            }
            _ => return Err(TransportError::InvalidRequest),
        }
        if self.scenario == Scenario::ShortControl {
            Ok(data.len().saturating_sub(1))
        } else {
            Ok(data.len())
        }
    }
    fn set_interface(&mut self, interface: u8, alternate: u8) -> Result<(), TransportError> {
        let streams = self.owned()?;
        self.ready()?;
        let operation = match (interface, alternate) {
            (1, 1) => Operation::StartPlayback,
            (2, 1) => Operation::StartCapture,
            (1, 0) => Operation::StopPlayback,
            (2, 0) => Operation::StopCapture,
            _ => return Err(TransportError::InvalidRequest),
        };
        self.event(operation)?;
        let next = match (interface, alternate, streams) {
            (2, 1, Streams::Idle | Streams::Capture) => Streams::Capture,
            (2, 1, Streams::Playback | Streams::Duplex) => Streams::Duplex,
            (1, 1, Streams::Capture | Streams::Duplex) => Streams::Duplex,
            (1, 0, Streams::Playback | Streams::Idle) => Streams::Idle,
            (1, 0, Streams::Duplex | Streams::Capture) => Streams::Capture,
            (2, 0, Streams::Capture | Streams::Idle) => Streams::Idle,
            (2, 0, Streams::Duplex | Streams::Playback) => Streams::Playback,
            _ => return Err(TransportError::InvalidRequest),
        };
        self.binding = Binding::Owned(next);
        Ok(())
    }
    fn capture_packet(&mut self, endpoint: u8, data: &mut [u8]) -> Result<usize, TransportError> {
        let expected = if self.profile == DeviceProfile::NxpTemplate {
            0x82
        } else {
            0x83
        };
        if endpoint != expected
            || !matches!(self.owned()?, Streams::Capture | Streams::Duplex)
            || ![44_100, 48_000, 88_200, 96_000, 176_400, 192_000].contains(&self.clock)
        {
            return Err(TransportError::InvalidRequest);
        }
        self.event(Operation::Capture)?;
        self.elapsed_us = self.elapsed_us.saturating_add(125);
        if self.packets >= 100_000 {
            return Err(TransportError::Timeout);
        }
        let before = self.packets as u64 * u64::from(self.clock) / 8000;
        let after = (self.packets as u64 + 1) * u64::from(self.clock) / 8000;
        let nominal =
            usize::try_from(after - before).map_err(|_| TransportError::InvalidRequest)?;
        let frames = match self.scenario {
            Scenario::Steady
            | Scenario::Malformed
            | Scenario::ShortControl
            | Scenario::UnsupportedRate
            | Scenario::MalformedRanges
            | Scenario::InvalidClock
            | Scenario::ClockMismatch => nominal,
            Scenario::Drift => match self.packets % 2048 {
                0 => nominal - 1,
                1 | 2047 => nominal + 1,
                _ => nominal,
            },
            Scenario::NegativeDrift => match self.packets % 2048 {
                0 => nominal + 1,
                1 | 2047 => nominal - 1,
                _ => nominal,
            },
            Scenario::Intermittent if self.packets.is_multiple_of(17) => 0,
            Scenario::Intermittent => nominal,
            Scenario::Disconnect if self.packets > 0 => {
                self.binding = Binding::Disconnected;
                self.pending_frames = None;
                return Err(TransportError::Disconnected);
            }
            Scenario::Disconnect => nominal,
            Scenario::Stalled if self.packets < 32 => 0,
            Scenario::Stalled => return Err(TransportError::Timeout),
            Scenario::Timeout => return Err(TransportError::Timeout),
        };
        self.packets = self
            .packets
            .checked_add(1)
            .ok_or(TransportError::InvalidRequest)?;
        self.pending_frames = Some(frames);
        let length = frames * 16;
        let target = data
            .get_mut(..length)
            .ok_or(TransportError::InvalidRequest)?;
        for frame in target.as_chunks_mut::<16>().0 {
            for (channel, sample) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let value = (self.frame % 0x0020_0000) * 4
                    + u32::try_from(channel).map_err(|_| TransportError::InvalidRequest)?;
                sample.copy_from_slice(&(value << 8).to_le_bytes());
            }
            self.frame = self
                .frame
                .checked_add(1)
                .ok_or(TransportError::InvalidRequest)?;
        }
        if self.scenario == Scenario::Malformed {
            Ok(length - 1)
        } else {
            Ok(length)
        }
    }
    fn playback_packet(&mut self, endpoint: u8, data: &[u8]) -> Result<(), TransportError> {
        if endpoint != 2
            || self.owned()? != Streams::Duplex
            || ![44_100, 48_000, 88_200, 96_000, 176_400, 192_000].contains(&self.clock)
        {
            return Err(TransportError::InvalidRequest);
        }
        let feedback_frames = self.pending_frames.ok_or(TransportError::InvalidRequest)?;
        if data.len() != feedback_frames * 16 {
            return Err(TransportError::InvalidRequest);
        }
        if data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|sample| sample.first() != Some(&0))
        {
            return Err(TransportError::InvalidRequest);
        }
        self.event(Operation::Playback)?;
        self.played.extend_from_slice(data);
        self.pending_frames = None;
        Ok(())
    }
    fn release(&mut self) -> Result<(), TransportError> {
        self.event(Operation::Release)?;
        self.owned()?;
        self.binding = Binding::Detached;
        Ok(())
    }
    fn attach(&mut self) -> Result<(), TransportError> {
        self.event(Operation::Attach)?;
        if self.binding == Binding::Disconnected {
            return Err(TransportError::Disconnected);
        }
        if self.binding != Binding::Detached {
            return Err(TransportError::InvalidRequest);
        }
        self.binding = Binding::Attached;
        Ok(())
    }
}
