use crate::{AudioError, PcmFault, PcmStream, SampleRate};
use alsa::pcm::{Access, Format, HwParams, State, PCM};
use alsa::{Direction, ValueOr};
use std::ffi::CString;

pub struct NativePcm {
    pcm: PCM,
    direction: Direction,
}

impl NativePcm {
    pub fn capture(name: &str, rate: SampleRate) -> Result<Self, AudioError> {
        Self::open(name, rate, Direction::Capture)
    }
    pub fn playback(name: &str, rate: SampleRate) -> Result<Self, AudioError> {
        Self::open(name, rate, Direction::Playback)
    }

    fn open(name: &str, rate: SampleRate, direction: Direction) -> Result<Self, AudioError> {
        let name = CString::new(name)
            .map_err(|_| AudioError::Invalid("audio device name contains a zero byte"))?;
        let pcm = PCM::open(&name, direction, true)?;
        let params = HwParams::any(&pcm)?;
        params.set_channels(4)?;
        params.set_rate(rate.hz(), ValueOr::Nearest)?;
        params.set_format(Format::s32())?;
        params.set_access(Access::RWInterleaved)?;
        let period = params.set_period_size_near(256, ValueOr::Nearest)?;
        params.set_buffer_size_near(period * 4)?;
        pcm.hw_params(&params)?;
        drop(params);
        let actual = pcm.hw_params_current()?;
        if actual.get_rate()? != rate.hz()
            || actual.get_channels()? != 4
            || actual.get_format()? != Format::s32()
        {
            return Err(AudioError::Invalid(
                "audio device changed the requested rate, channels, or sample format",
            ));
        }
        let period = actual.get_period_size()?;
        let buffer = actual.get_buffer_size()?;
        if period <= 0 || buffer < period {
            return Err(AudioError::Invalid(
                "audio device returned invalid buffer sizes",
            ));
        }
        drop(actual);
        let software = pcm.sw_params_current()?;
        software.set_avail_min(period)?;
        software.set_start_threshold(if direction == Direction::Capture {
            1
        } else {
            (buffer / period) * period
        })?;
        pcm.sw_params(&software)?;
        drop(software);
        pcm.prepare()?;
        Ok(Self { pcm, direction })
    }
}

fn fault(error: alsa::Error) -> PcmFault {
    match error.errno() {
        libc::EAGAIN => PcmFault::Again,
        libc::EINTR => PcmFault::Interrupted,
        libc::EPIPE => PcmFault::Xrun,
        libc::ESTRPIPE => PcmFault::Suspended,
        libc::ENODEV | libc::ENOTTY => PcmFault::Disconnected,
        code => PcmFault::System(code),
    }
}

impl PcmStream for NativePcm {
    fn read(&mut self, samples: &mut [i32]) -> Result<usize, PcmFault> {
        if self.direction != Direction::Capture || !samples.len().is_multiple_of(4) {
            return Err(PcmFault::System(libc::EINVAL));
        }
        self.pcm
            .io_i32()
            .map_err(fault)?
            .readi(samples)
            .map_err(fault)
    }
    fn write(&mut self, samples: &[i32]) -> Result<usize, PcmFault> {
        if self.direction != Direction::Playback || !samples.len().is_multiple_of(4) {
            return Err(PcmFault::System(libc::EINVAL));
        }
        self.pcm
            .io_i32()
            .map_err(fault)?
            .writei(samples)
            .map_err(fault)
    }
    fn wait(&mut self, milliseconds: u32) -> Result<bool, PcmFault> {
        self.pcm.wait(Some(milliseconds.min(50))).map_err(fault)
    }
    fn recover(&mut self, reason: PcmFault) -> Result<(), PcmFault> {
        match reason {
            PcmFault::Xrun => self.pcm.prepare().map_err(fault),
            PcmFault::Suspended => match self.pcm.resume().map_err(fault) {
                Ok(()) => Ok(()),
                Err(PcmFault::Again) => Err(PcmFault::Again),
                Err(_) => self.pcm.prepare().map_err(fault),
            },
            reason => Err(reason),
        }
    }
    fn drain(&mut self) -> Result<(), PcmFault> {
        match self.pcm.drain().map_err(fault) {
            Err(PcmFault::Again | PcmFault::Interrupted)
                if self.pcm.status().map_err(fault)?.get_state() == State::Setup =>
            {
                Ok(())
            }
            result => result,
        }
    }
    fn stop(&mut self) -> Result<(), PcmFault> {
        self.pcm.drop().map_err(fault)
    }
}
