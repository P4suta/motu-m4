use motu_m4_core::{
    ClockRangeCount, ClockRanges, ControlSetup, Error, FrameCount, Layout, PacketLength, PcmSample,
    Recovery, RecoveryAction, SampleRate, FRAME_BYTES, SAMPLE_RATE,
};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    Validate,
    Takeover,
    ReadRanges,
    ReadClock,
    SetClock,
    VerifyClock,
    ClockValidity,
    StartCapture,
    StartPlayback,
    Capture,
    Playback,
    StopPlayback,
    StopCapture,
    RestoreClock,
    VerifyRestoreClock,
    Release,
    Attach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Injected,
    Disconnected,
    Timeout,
    InvalidRequest,
    NotOwned,
    NotReady,
    Firmware,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureCause {
    Transport(TransportError),
    Protocol(Error),
    ShortControl,
    Stalled,
    Cancelled,
    InvalidAudio,
    InvalidClock,
    ClockMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub operation: Operation,
    pub cause: FailureCause,
}

#[derive(Debug)]
pub struct RunError {
    pub primary: Option<Failure>,
    pub recovery: Vec<Failure>,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(failure) = self.primary {
            write!(f, "{:?}: {:?}", failure.operation, failure.cause)?;
        }
        for failure in &self.recovery {
            write!(f, "; recovery {:?}: {:?}", failure.operation, failure.cause)?;
        }
        Ok(())
    }
}

impl std::error::Error for RunError {}

/// Transport calls must return within a finite deadline.
pub trait AudioTransport {
    fn descriptors(&self) -> &[u8];
    fn product(&self) -> &str;
    /// Elapsed time since this transport first observed the device.
    fn elapsed_us(&self) -> u64;
    /// Advance elapsed time by at least the requested duration before returning.
    fn wait_us(&mut self, microseconds: u32);
    /// Successful takeover must own idle audio interfaces without changing the clock.
    /// A failed takeover must leave the original driver attached.
    fn takeover(&mut self) -> Result<(), TransportError>;
    fn control(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<usize, TransportError>;
    fn set_interface(&mut self, interface: u8, alternate: u8) -> Result<(), TransportError>;
    fn capture_packet(&mut self, endpoint: u8, data: &mut [u8]) -> Result<usize, TransportError>;
    fn playback_packet(&mut self, endpoint: u8, data: &[u8]) -> Result<(), TransportError>;
    fn release(&mut self) -> Result<(), TransportError>;
    fn attach(&mut self) -> Result<(), TransportError>;
}

#[derive(Debug)]
pub struct SessionReport {
    pub frames: u32,
    pub rate: SampleRate,
    pub packet_lengths: Vec<u16>,
}

#[derive(Debug)]
pub struct StreamOutput {
    pub audio: Vec<u8>,
    pub report: SessionReport,
}

#[derive(Clone, Copy, Debug)]
pub enum StreamRequest<'a> {
    Capture(FrameCount),
    Playback(&'a [u8]),
    Duplex(&'a [u8]),
}

impl<'a> StreamRequest<'a> {
    fn playback(self) -> Option<&'a [u8]> {
        match self {
            Self::Capture(_) => None,
            Self::Playback(audio) | Self::Duplex(audio) => Some(audio),
        }
    }

    fn records(self) -> bool {
        match self {
            Self::Capture(_) | Self::Duplex(_) => true,
            Self::Playback(_) => false,
        }
    }

    fn frames(self) -> Result<FrameCount, Failure> {
        match self {
            Self::Capture(frames) => Ok(frames),
            Self::Playback(audio) | Self::Duplex(audio) => {
                let invalid = || failure(Operation::Validate, FailureCause::InvalidAudio);
                if audio.len() % usize::from(FRAME_BYTES) != 0 {
                    return Err(invalid());
                }
                let count =
                    u32::try_from(audio.len() / usize::from(FRAME_BYTES)).map_err(|_| invalid())?;
                FrameCount::new(count).map_err(|_| invalid())
            }
        }
    }
}

fn failure(operation: Operation, cause: FailureCause) -> Failure {
    Failure { operation, cause }
}

fn io<T>(operation: Operation, result: Result<T, TransportError>) -> Result<T, Failure> {
    result.map_err(|error| failure(operation, FailureCause::Transport(error)))
}

fn control<T: AudioTransport>(
    device: &mut T,
    setup: ControlSetup,
    data: &mut [u8],
    operation: Operation,
) -> Result<(), Failure> {
    let result = device.control(setup, data);
    device.wait_us(setup.post_delay_us());
    if io(operation, result)? != data.len() {
        return Err(failure(operation, FailureCause::ShortControl));
    }
    Ok(())
}

fn read_clock<T: AudioTransport>(
    device: &mut T,
    layout: Layout,
    operation: Operation,
) -> Result<u32, Failure> {
    let mut bytes = [0; 4];
    control(device, layout.read_clock(), &mut bytes, operation)?;
    Ok(u32::from_le_bytes(bytes))
}

fn negotiate<T: AudioTransport>(
    device: &mut T,
    layout: Layout,
    rate: SampleRate,
    previous: SampleRate,
) -> Result<(), Failure> {
    let mut header = [0; 2];
    control(
        device,
        layout.clock_ranges(2),
        &mut header,
        Operation::ReadRanges,
    )?;
    let protocol = |error| failure(Operation::ReadRanges, FailureCause::Protocol(error));
    let count = ClockRangeCount::parse(&header).map_err(protocol)?;
    let mut data = vec![0; usize::from(count.bytes())];
    control(
        device,
        layout.clock_ranges(count.bytes()),
        &mut data,
        Operation::ReadRanges,
    )?;
    let ranges = ClockRanges::parse(&data).map_err(protocol)?;
    if !ranges.supports(rate) || !ranges.supports(previous) {
        return Err(protocol(Error::Clock));
    }
    Ok(())
}

struct SessionGuard<'a, T: AudioTransport> {
    device: &'a mut T,
    layout: Layout,
    recovery: Recovery,
}

impl<T: AudioTransport> SessionGuard<'_, T> {
    fn recover(&mut self) -> Vec<Failure> {
        let device = &mut *self.device;
        let layout = self.layout;
        let mut errors = Vec::new();
        while let Some((action, remaining)) = self.recovery.next() {
            self.recovery = remaining;
            let result = match action {
                RecoveryAction::StopPlayback => io(
                    Operation::StopPlayback,
                    device.set_interface(layout.playback().interface(), 0),
                ),
                RecoveryAction::StopCapture => io(
                    Operation::StopCapture,
                    device.set_interface(layout.capture().interface(), 0),
                ),
                RecoveryAction::RestoreClock(rate) => (|| {
                    control(
                        device,
                        layout.set_clock(),
                        &mut rate.to_le_bytes(),
                        Operation::RestoreClock,
                    )?;
                    if read_clock(device, layout, Operation::VerifyRestoreClock)? != rate {
                        return Err(failure(
                            Operation::VerifyRestoreClock,
                            FailureCause::ClockMismatch,
                        ));
                    }
                    Ok(())
                })(),
                RecoveryAction::Release => io(Operation::Release, device.release()),
                RecoveryAction::Attach => io(Operation::Attach, device.attach()),
            };
            if let Err(error) = result {
                errors.push(error);
            }
        }
        errors
    }
}

impl<T: AudioTransport> Drop for SessionGuard<'_, T> {
    fn drop(&mut self) {
        let _ = self.recover();
    }
}

fn session<T: AudioTransport, R>(
    device: &mut T,
    duplex: bool,
    rate: SampleRate,
    run: impl FnOnce(&mut T, Layout) -> Result<R, Failure>,
) -> Result<R, RunError> {
    let layout =
        Layout::parse(device.descriptors(), device.product()).map_err(|error| RunError {
            primary: Some(failure(Operation::Validate, FailureCause::Protocol(error))),
            recovery: Vec::new(),
        })?;
    let mut guard = SessionGuard {
        device,
        layout,
        recovery: Recovery::Complete,
    };
    let result = (|| {
        let device = &mut *guard.device;
        if rate.max_packet_bytes() > u32::from(layout.capture().max_packet())
            || rate.max_packet_bytes() > u32::from(layout.playback().max_packet())
        {
            return Err(failure(
                Operation::Validate,
                FailureCause::Protocol(Error::Packet),
            ));
        }
        let remaining_boot =
            u64::from(layout.hardware().boot_delay_us()).saturating_sub(device.elapsed_us());
        device.wait_us(remaining_boot as u32);
        io(Operation::Takeover, device.takeover())?;
        guard.recovery = Recovery::Claim;
        let previous = read_clock(device, layout, Operation::ReadClock)?;
        let previous_rate = SampleRate::new(previous)
            .map_err(|error| failure(Operation::ReadClock, FailureCause::Protocol(error)))?;
        negotiate(device, layout, rate, previous_rate)?;
        let changed_clock = previous != rate.hz();
        if changed_clock {
            guard.recovery = Recovery::Clock(previous);
            control(
                device,
                layout.set_clock(),
                &mut rate.hz().to_le_bytes(),
                Operation::SetClock,
            )?;
            if read_clock(device, layout, Operation::VerifyClock)? != rate.hz() {
                return Err(failure(Operation::VerifyClock, FailureCause::ClockMismatch));
            }
        }
        if let Some(setup) = layout.clock_validity() {
            let mut valid = [0];
            control(device, setup, &mut valid, Operation::ClockValidity)?;
            if valid != [1] {
                return Err(failure(
                    Operation::ClockValidity,
                    FailureCause::InvalidClock,
                ));
            }
        }
        guard.recovery = if changed_clock {
            Recovery::Capture(previous)
        } else {
            Recovery::CaptureUnchanged
        };
        io(
            Operation::StartCapture,
            device.set_interface(layout.capture().interface(), 1),
        )?;
        if duplex {
            guard.recovery = if changed_clock {
                Recovery::Duplex(previous)
            } else {
                Recovery::DuplexUnchanged
            };
            io(
                Operation::StartPlayback,
                device.set_interface(layout.playback().interface(), 1),
            )?;
        }
        run(device, layout)
    })();
    let errors = guard.recover();
    match result {
        Ok(value) if errors.is_empty() => Ok(value),
        Ok(_) => Err(RunError {
            primary: None,
            recovery: errors,
        }),
        Err(error) => Err(RunError {
            primary: Some(error),
            recovery: errors,
        }),
    }
}

fn packet<T: AudioTransport>(
    device: &mut T,
    layout: Layout,
    buffer: &mut [u8],
    empty: &mut u8,
) -> Result<PacketLength, Failure> {
    let actual = io(
        Operation::Capture,
        device.capture_packet(layout.capture().address(), buffer),
    )?;
    let actual = u32::try_from(actual)
        .map_err(|_| failure(Operation::Capture, FailureCause::Protocol(Error::Packet)))?;
    let length = PacketLength::from_feedback(
        actual,
        layout.capture().max_packet(),
        layout.playback().max_packet(),
    )
    .map_err(|error| failure(Operation::Capture, FailureCause::Protocol(error)))?;
    if length.bytes() == 0 {
        *empty += 1;
        if *empty >= 32 {
            return Err(failure(Operation::Capture, FailureCause::Stalled));
        }
    } else {
        *empty = 0;
    }
    Ok(length)
}

pub fn capture<T: AudioTransport>(
    device: &mut T,
    frames: FrameCount,
) -> Result<StreamOutput, RunError> {
    capture_with_cancel(device, frames, || false)
}

pub fn capture_with_cancel<T: AudioTransport>(
    device: &mut T,
    frames: FrameCount,
    mut cancelled: impl FnMut() -> bool,
) -> Result<StreamOutput, RunError> {
    let rate = SampleRate::new(SAMPLE_RATE).map_err(|error| RunError {
        primary: Some(failure(Operation::Validate, FailureCause::Protocol(error))),
        recovery: Vec::new(),
    })?;
    run_stream(device, rate, StreamRequest::Capture(frames), |_| {
        cancelled()
    })
}

pub fn play<T: AudioTransport>(device: &mut T, audio: &[u8]) -> Result<SessionReport, RunError> {
    let rate = SampleRate::new(SAMPLE_RATE).map_err(|error| RunError {
        primary: Some(failure(Operation::Validate, FailureCause::Protocol(error))),
        recovery: Vec::new(),
    })?;
    run_stream(device, rate, StreamRequest::Playback(audio), |_| false).map(|output| output.report)
}

/// The cancellation callback receives the number of completed capture packets.
/// Playback requests leave the returned audio empty.
/// Unwinding attempts the same cleanup as normal completion.
pub fn run_stream<T: AudioTransport>(
    device: &mut T,
    rate: SampleRate,
    request: StreamRequest<'_>,
    mut cancelled: impl FnMut(u32) -> bool,
) -> Result<StreamOutput, RunError> {
    let early = |primary| RunError {
        primary: Some(primary),
        recovery: Vec::new(),
    };
    let frames = request.frames().map_err(early)?;
    if cancelled(0) {
        return Err(early(failure(Operation::Validate, FailureCause::Cancelled)));
    }
    session(
        device,
        request.playback().is_some(),
        rate,
        |device, layout| {
            let mut captured = Vec::new();
            let mut feedback = vec![0; usize::from(layout.capture().max_packet())];
            let mut output = vec![0; usize::from(layout.playback().max_packet())];
            let mut remaining = frames.get();
            let mut source = request.playback();
            let mut empty = 0;
            let mut packet_lengths = Vec::new();
            let mut completed = 0;
            while remaining > 0 {
                if cancelled(completed) {
                    return Err(failure(Operation::Capture, FailureCause::Cancelled));
                }
                let length = packet(device, layout, &mut feedback, &mut empty)?;
                let take = remaining.min(u32::from(length.frames()));
                let bytes = usize::try_from(take * u32::from(FRAME_BYTES))
                    .map_err(|_| failure(Operation::Capture, FailureCause::InvalidAudio))?;
                if request.records() {
                    captured.extend_from_slice(
                        feedback.get(..bytes).ok_or_else(|| {
                            failure(Operation::Capture, FailureCause::InvalidAudio)
                        })?,
                    );
                }
                if let Some(audio) = source {
                    let (head, tail) = audio.split_at(bytes);
                    let target = output
                        .get_mut(..usize::from(length.bytes()))
                        .ok_or_else(|| failure(Operation::Playback, FailureCause::InvalidAudio))?;
                    target.fill(0);
                    target
                        .get_mut(..bytes)
                        .ok_or_else(|| failure(Operation::Playback, FailureCause::InvalidAudio))?
                        .copy_from_slice(head);
                    for sample in target.as_chunks_mut::<4>().0 {
                        *sample = PcmSample::from_bits(u32::from_le_bytes(*sample)).to_le_bytes();
                    }
                    io(
                        Operation::Playback,
                        device.playback_packet(layout.playback().address(), target),
                    )?;
                    source = Some(tail);
                }
                remaining -= take;
                completed += 1;
                packet_lengths.push(length.bytes());
            }
            Ok(StreamOutput {
                audio: captured,
                report: SessionReport {
                    frames: frames.get(),
                    rate,
                    packet_lengths,
                },
            })
        },
    )
}
