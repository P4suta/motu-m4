use crate::{wave, AudioError, AudioInputs, AudioOutputs, SampleRate, WaveInput};
use motu_m4_core::RecordingFrames;
use std::io::{Read, Seek, Write};

const BLOCK_FRAMES: usize = 256;
const IDLE_POLLS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFault {
    Again,
    Interrupted,
    Xrun,
    Suspended,
    Disconnected,
    System(i32),
}

impl std::fmt::Display for PcmFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Again => f.write_str("device isn't ready"),
            Self::Interrupted => f.write_str("operation interrupted"),
            Self::Xrun => f.write_str("buffer overrun or underrun"),
            Self::Suspended => f.write_str("device suspended"),
            Self::Disconnected => f.write_str("device disconnected"),
            Self::System(code) => write!(f, "{}", std::io::Error::from_raw_os_error(*code)),
        }
    }
}

pub trait PcmStream {
    fn read(&mut self, samples: &mut [i32]) -> Result<usize, PcmFault>;
    fn write(&mut self, samples: &[i32]) -> Result<usize, PcmFault>;
    fn wait(&mut self, milliseconds: u32) -> Result<bool, PcmFault>;
    fn recover(&mut self, fault: PcmFault) -> Result<(), PcmFault>;
    fn drain(&mut self) -> Result<(), PcmFault>;
    fn stop(&mut self) -> Result<(), PcmFault>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NativeReport {
    pub captured_frames: u64,
    pub played_frames: u64,
    pub recoveries: u64,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureRequest {
    rate: SampleRate,
    frames: u64,
    inputs: AudioInputs,
}

impl CaptureRequest {
    pub fn new(rate: SampleRate, frames: u64, inputs: AudioInputs) -> Result<Self, AudioError> {
        let frames = RecordingFrames::new(frames, inputs.channels())
            .map_err(|_| {
                AudioError::Invalid("recording exceeds the WAV size limit or has no frames")
            })?
            .get();
        Ok(Self {
            rate,
            frames,
            inputs,
        })
    }
    pub fn rate(self) -> SampleRate {
        self.rate
    }
    pub fn frames(self) -> u64 {
        self.frames
    }
}

struct Guard<'a, S: PcmStream> {
    stream: &'a mut S,
    closed: bool,
}

impl<'a, S: PcmStream> Guard<'a, S> {
    fn new(stream: &'a mut S) -> Self {
        Self {
            stream,
            closed: false,
        }
    }
    fn close(&mut self, errors: &mut Vec<AudioError>) {
        self.closed = true;
        if let Err(fault) = self.stream.stop() {
            errors.push(AudioError::Pcm {
                operation: "stop audio",
                fault,
            });
        }
    }
}

impl<S: PcmStream> Drop for Guard<'_, S> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.stream.stop();
        }
    }
}

fn finish<T>(result: Result<T, AudioError>, errors: Vec<AudioError>) -> Result<T, AudioError> {
    if errors.is_empty() {
        return result;
    }
    Err(AudioError::Cleanup {
        primary: result.err().map(Box::new),
        errors,
    })
}

fn transfer<S: PcmStream>(
    stream: &mut S,
    samples: &mut [i32],
    playback: bool,
    report: &mut NativeReport,
    cancel: &mut impl FnMut() -> bool,
) -> Result<Option<usize>, AudioError> {
    let mut recoveries = 0;
    for _ in 0..IDLE_POLLS {
        if cancel() {
            report.cancelled = true;
            return Ok(None);
        }
        let operation = if playback {
            "play audio"
        } else {
            "capture audio"
        };
        let result = if playback {
            stream.write(samples)
        } else {
            stream.read(samples)
        };
        let fault = match result {
            Ok(frames) if frames > samples.len() / 4 => {
                return Err(AudioError::Invalid(
                    "audio device returned more frames than its buffer",
                ))
            }
            Ok(frames) if frames > 0 => return Ok(Some(frames)),
            Ok(_) => PcmFault::Again,
            Err(fault) => fault,
        };
        match fault {
            PcmFault::Again | PcmFault::Interrupted => {}
            PcmFault::Xrun | PcmFault::Suspended if recoveries < 3 => match stream.recover(fault) {
                Ok(()) => {
                    report.recoveries += 1;
                    recoveries += 1;
                }
                Err(PcmFault::Again | PcmFault::Interrupted) => {}
                Err(fault) => {
                    return Err(AudioError::Pcm {
                        operation: "recover audio",
                        fault,
                    })
                }
            },
            fault => return Err(AudioError::Pcm { operation, fault }),
        }
        match stream.wait(50) {
            Ok(_)
            | Err(PcmFault::Again | PcmFault::Interrupted | PcmFault::Xrun | PcmFault::Suspended) =>
                {}
            Err(fault) => {
                return Err(AudioError::Pcm {
                    operation: "wait for audio",
                    fault,
                })
            }
        }
    }
    Err(AudioError::Invalid(
        "audio device made no progress within 20 bounded waits",
    ))
}

fn write_all<S: PcmStream>(
    stream: &mut S,
    samples: &mut [i32],
    report: &mut NativeReport,
    cancel: &mut impl FnMut() -> bool,
) -> Result<(), AudioError> {
    let mut offset = 0;
    while offset < samples.len() {
        let Some(frames) = transfer(stream, &mut samples[offset..], true, report, cancel)? else {
            break;
        };
        offset += frames * 4;
        report.played_frames += frames as u64;
    }
    Ok(())
}

fn drain<S: PcmStream>(
    stream: &mut S,
    report: &mut NativeReport,
    cancel: &mut impl FnMut() -> bool,
) -> Result<(), AudioError> {
    for _ in 0..IDLE_POLLS {
        if cancel() {
            report.cancelled = true;
            return Ok(());
        }
        match stream.drain() {
            Ok(()) => return Ok(()),
            Err(PcmFault::Again | PcmFault::Interrupted) => match stream.wait(50) {
                Ok(_) | Err(PcmFault::Again | PcmFault::Interrupted) => {}
                Err(fault) => {
                    if stream.drain().is_ok() {
                        return Ok(());
                    }
                    return Err(AudioError::Pcm {
                        operation: "wait for playback drain",
                        fault,
                    });
                }
            },
            Err(fault) => {
                return Err(AudioError::Pcm {
                    operation: "drain playback",
                    fault,
                })
            }
        }
    }
    Err(AudioError::Invalid(
        "playback drain made no progress within 20 bounded waits",
    ))
}

pub fn capture_wave<S: PcmStream, W: Write + Seek>(
    stream: &mut S,
    target: W,
    request: CaptureRequest,
    mut cancel: impl FnMut() -> bool,
) -> Result<NativeReport, AudioError> {
    let mut guard = Guard::new(stream);
    let mut errors = Vec::new();
    let result = (|| {
        let mut writer = wave::writer(target, request.rate, request.inputs)?;
        let mut report = NativeReport::default();
        let mut buffer = [0; BLOCK_FRAMES * 4];
        let result = (|| {
            while report.captured_frames < request.frames {
                let frames =
                    (request.frames - report.captured_frames).min(BLOCK_FRAMES as u64) as usize;
                let Some(count) = transfer(
                    guard.stream,
                    &mut buffer[..frames * 4],
                    false,
                    &mut report,
                    &mut cancel,
                )?
                else {
                    break;
                };
                wave::write_frames(&mut writer, &buffer[..count * 4], request.inputs)?;
                report.captured_frames += count as u64;
            }
            Ok(report)
        })();
        if let Err(error) = writer.finalize() {
            errors.push(AudioError::Wave(error));
        }
        result
    })();
    guard.close(&mut errors);
    finish(result, errors)
}

pub fn play_wave<S: PcmStream, R: Read>(
    stream: &mut S,
    mut input: WaveInput<R>,
    outputs: AudioOutputs,
    mut cancel: impl FnMut() -> bool,
) -> Result<NativeReport, AudioError> {
    let mut guard = Guard::new(stream);
    let result = (|| {
        outputs.route([0; 4], input.channels())?;
        let mut report = NativeReport::default();
        let mut buffer = [0; BLOCK_FRAMES * 4];
        loop {
            if cancel() {
                report.cancelled = true;
                break;
            }
            let frames = input.read(&mut buffer, outputs)?;
            if frames == 0 {
                break;
            }
            write_all(
                guard.stream,
                &mut buffer[..frames * 4],
                &mut report,
                &mut cancel,
            )?;
            if report.cancelled {
                break;
            }
        }
        if !report.cancelled {
            drain(guard.stream, &mut report, &mut cancel)?;
        }
        Ok(report)
    })();
    let mut errors = Vec::new();
    guard.close(&mut errors);
    finish(result, errors)
}

pub fn duplex_wave<C: PcmStream, P: PcmStream, R: Read, W: Write + Seek>(
    capture: &mut C,
    playback: &mut P,
    mut input: WaveInput<R>,
    target: W,
    inputs: AudioInputs,
    outputs: AudioOutputs,
    mut cancel: impl FnMut() -> bool,
) -> Result<NativeReport, AudioError> {
    let mut capture = Guard::new(capture);
    let mut playback = Guard::new(playback);
    let mut errors = Vec::new();
    let result = (|| {
        outputs.route([0; 4], input.channels())?;
        CaptureRequest::new(input.rate(), input.frames(), inputs)?;
        let mut writer = wave::writer(target, input.rate(), inputs)?;
        let mut report = NativeReport::default();
        let mut recorded = [0; BLOCK_FRAMES * 4];
        let mut played = [0; BLOCK_FRAMES * 4];
        let result = (|| {
            loop {
                if cancel() {
                    report.cancelled = true;
                    break;
                }
                let frames = input.read(&mut played, outputs)?;
                if frames == 0 {
                    break;
                }
                let mut offset = 0;
                while offset < frames * 4 {
                    let Some(count) = transfer(
                        capture.stream,
                        &mut recorded[offset..frames * 4],
                        false,
                        &mut report,
                        &mut cancel,
                    )?
                    else {
                        break;
                    };
                    wave::write_frames(&mut writer, &recorded[offset..offset + count * 4], inputs)?;
                    report.captured_frames += count as u64;
                    offset += count * 4;
                }
                if report.cancelled {
                    break;
                }
                write_all(
                    playback.stream,
                    &mut played[..frames * 4],
                    &mut report,
                    &mut cancel,
                )?;
                if report.cancelled {
                    break;
                }
            }
            if !report.cancelled {
                drain(playback.stream, &mut report, &mut cancel)?;
            }
            Ok(report)
        })();
        if let Err(error) = writer.finalize() {
            errors.push(AudioError::Wave(error));
        }
        result
    })();
    playback.close(&mut errors);
    capture.close(&mut errors);
    finish(result, errors)
}
