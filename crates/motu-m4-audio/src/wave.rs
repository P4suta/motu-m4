use crate::{AudioChannels, AudioError, AudioInputs, AudioOutputs, SampleRate};
use motu_m4_core::{pcm_from_float, pcm_from_signed};
use std::io::{Read, Seek, Write};

pub struct WaveInput<R> {
    reader: hound::WavReader<R>,
    rate: SampleRate,
    channels: AudioChannels,
    remaining: u64,
    frames: u64,
}

impl<R: Read> WaveInput<R> {
    pub fn new(reader: R) -> Result<Self, AudioError> {
        let reader = hound::WavReader::new(reader)?;
        let spec = reader.spec();
        let rate = SampleRate::new(spec.sample_rate).map_err(|_| {
            AudioError::Invalid("WAV rate must be 44100, 48000, 88200, 96000, 176400, or 192000 Hz")
        })?;
        let channels = AudioChannels::new(spec.channels)?;
        match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Int, 16 | 24 | 32) | (hound::SampleFormat::Float, 32) => {}
            _ => {
                return Err(AudioError::Invalid(
                    "WAV audio must use 16/24/32-bit signed PCM or 32-bit float",
                ))
            }
        }
        if reader.len() == 0 || !reader.len().is_multiple_of(u32::from(channels.get())) {
            return Err(AudioError::Invalid(
                "WAV audio must contain complete nonempty frames",
            ));
        }
        let frames = u64::from(reader.duration());
        Ok(Self {
            reader,
            rate,
            channels,
            remaining: frames,
            frames,
        })
    }

    pub fn rate(&self) -> SampleRate {
        self.rate
    }
    pub fn channels(&self) -> AudioChannels {
        self.channels
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn read(&mut self, output: &mut [i32], routing: AudioOutputs) -> Result<usize, AudioError> {
        if !output.len().is_multiple_of(4) {
            return Err(AudioError::Invalid(
                "playback buffer must contain complete four-channel frames",
            ));
        }
        let mut count = 0;
        let spec = self.reader.spec();
        for target in output.as_chunks_mut::<4>().0 {
            if self.remaining == 0 {
                break;
            }
            let mut frame = [0; 4];
            for sample in frame.iter_mut().take(usize::from(self.channels.get())) {
                *sample = match spec.sample_format {
                    hound::SampleFormat::Int => pcm_from_signed(
                        self.reader
                            .samples::<i32>()
                            .next()
                            .ok_or(AudioError::Invalid("truncated WAV frame"))??,
                        spec.bits_per_sample,
                    )?,
                    hound::SampleFormat::Float => pcm_from_float(
                        self.reader
                            .samples::<f32>()
                            .next()
                            .ok_or(AudioError::Invalid("truncated WAV frame"))??,
                    )?,
                };
            }
            *target = routing.route(frame, self.channels)?;
            count += 1;
            self.remaining -= 1;
        }
        Ok(count)
    }
}

pub(crate) fn writer<W: Write + Seek>(
    target: W,
    rate: SampleRate,
    inputs: AudioInputs,
) -> Result<hound::WavWriter<W>, AudioError> {
    Ok(hound::WavWriter::new(
        target,
        hound::WavSpec {
            channels: inputs.channels().get(),
            sample_rate: rate.hz(),
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
    )?)
}

pub(crate) fn write_frames<W: Write + Seek>(
    writer: &mut hound::WavWriter<W>,
    data: &[i32],
    inputs: AudioInputs,
) -> Result<(), AudioError> {
    if !data.len().is_multiple_of(4) {
        return Err(AudioError::Invalid(
            "capture buffer must contain complete four-channel frames",
        ));
    }
    for frame in data.as_chunks::<4>().0 {
        let selected = inputs.select(*frame);
        for sample in selected
            .into_iter()
            .take(usize::from(inputs.channels().get()))
        {
            writer.write_sample(sample >> 8)?;
        }
    }
    Ok(())
}
