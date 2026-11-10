use crate::SampleRate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormatError {
    Channels,
    Encoding,
    SampleRange,
    FrameLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingFrames(u64);

impl RecordingFrames {
    pub fn new(frames: u64, channels: AudioChannels) -> Result<Self, AudioFormatError> {
        let limit = (u64::from(u32::MAX) - 256) / (u64::from(channels.get()) * 3);
        if frames == 0 || frames > limit {
            return Err(AudioFormatError::FrameLimit);
        }
        Ok(Self(frames))
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChannels {
    Mono,
    Stereo,
    Four,
}

impl AudioChannels {
    pub fn new(channels: u16) -> Result<Self, AudioFormatError> {
        match channels {
            1 => Ok(Self::Mono),
            2 => Ok(Self::Stereo),
            4 => Ok(Self::Four),
            _ => Err(AudioFormatError::Channels),
        }
    }

    pub const fn get(self) -> u16 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Four => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioInputs {
    Input1,
    Input2,
    Input3,
    Input4,
    Pair12,
    Pair34,
    All,
}

impl AudioInputs {
    pub const fn channels(self) -> AudioChannels {
        match self {
            Self::Input1 | Self::Input2 | Self::Input3 | Self::Input4 => AudioChannels::Mono,
            Self::Pair12 | Self::Pair34 => AudioChannels::Stereo,
            Self::All => AudioChannels::Four,
        }
    }

    pub const fn select(self, frame: [i32; 4]) -> [i32; 4] {
        match self {
            Self::Input1 => [frame[0], 0, 0, 0],
            Self::Input2 => [frame[1], 0, 0, 0],
            Self::Input3 => [frame[2], 0, 0, 0],
            Self::Input4 => [frame[3], 0, 0, 0],
            Self::Pair12 => [frame[0], frame[1], 0, 0],
            Self::Pair34 => [frame[2], frame[3], 0, 0],
            Self::All => frame,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOutputs {
    Main,
    Line,
    All,
}

impl AudioOutputs {
    pub fn route(
        self,
        frame: [i32; 4],
        channels: AudioChannels,
    ) -> Result<[i32; 4], AudioFormatError> {
        if channels == AudioChannels::Four {
            return if self == Self::All {
                Ok(frame)
            } else {
                Err(AudioFormatError::Channels)
            };
        }
        let right = if channels == AudioChannels::Mono {
            frame[0]
        } else {
            frame[1]
        };
        Ok(match self {
            Self::Main | Self::All => [frame[0], right, 0, 0],
            Self::Line => [0, 0, frame[0], right],
        })
    }
}

pub fn pcm_from_signed(sample: i32, bits: u16) -> Result<i32, AudioFormatError> {
    match bits {
        16 if (-32_768..=32_767).contains(&sample) => Ok(sample << 16),
        24 if (-8_388_608..=8_388_607).contains(&sample) => Ok(sample << 8),
        32 => Ok(sample & !255),
        16 | 24 => Err(AudioFormatError::SampleRange),
        _ => Err(AudioFormatError::Encoding),
    }
}

pub fn pcm_from_float(sample: f32) -> Result<i32, AudioFormatError> {
    if !sample.is_finite() {
        return Err(AudioFormatError::SampleRange);
    }
    Ok((sample.clamp(-1.0, 1.0) * 2_147_483_648.0) as i32 & !255)
}

pub fn native_recording_frames(
    rate: SampleRate,
    seconds: u32,
    channels: AudioChannels,
) -> Result<u64, AudioFormatError> {
    let frames = u64::from(rate.hz()) * u64::from(seconds);
    RecordingFrames::new(frames, channels).map(RecordingFrames::get)
}
