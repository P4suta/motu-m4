use motu_m4_audio::{
    capture_wave, duplex_wave, play_wave, AudioInputs, AudioOutputs, CaptureRequest, PcmFault,
    PcmStream, SampleRate, WaveInput,
};
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
struct Device {
    frame: usize,
    max_transfer: usize,
    read_faults: VecDeque<PcmFault>,
    writes: Vec<i32>,
    stops: usize,
    waits: usize,
    recoveries: usize,
    disconnect_at: Option<usize>,
    cancel_after_read: Option<Arc<AtomicBool>>,
    stop_fault: Option<PcmFault>,
    drain_fault: Option<PcmFault>,
    oversized_read: bool,
    drain_faults: VecDeque<PcmFault>,
    wait_faults: VecDeque<PcmFault>,
}

impl Device {
    fn new(max_transfer: usize) -> Self {
        Self {
            max_transfer,
            ..Self::default()
        }
    }
}

impl PcmStream for Device {
    fn read(&mut self, samples: &mut [i32]) -> Result<usize, PcmFault> {
        if self.oversized_read {
            return Ok(samples.len() / 4 + 1);
        }
        if let Some(error) = self.read_faults.pop_front() {
            return Err(error);
        }
        if self.disconnect_at == Some(self.frame) {
            return Err(PcmFault::Disconnected);
        }
        let frames = self.max_transfer.min(samples.len() / 4);
        for frame in samples.as_chunks_mut::<4>().0.iter_mut().take(frames) {
            *frame = std::array::from_fn(|channel| ((self.frame * 4 + channel) as i32 - 5) << 8);
            self.frame += 1;
        }
        if let Some(flag) = &self.cancel_after_read {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(frames)
    }
    fn write(&mut self, samples: &[i32]) -> Result<usize, PcmFault> {
        let frames = self.max_transfer.min(samples.len() / 4);
        self.writes.extend_from_slice(&samples[..frames * 4]);
        Ok(frames)
    }
    fn wait(&mut self, _: u32) -> Result<bool, PcmFault> {
        self.waits += 1;
        self.wait_faults.pop_front().map_or(Ok(true), Err)
    }
    fn recover(&mut self, _: PcmFault) -> Result<(), PcmFault> {
        self.recoveries += 1;
        Ok(())
    }
    fn drain(&mut self) -> Result<(), PcmFault> {
        self.drain_faults
            .pop_front()
            .or(self.drain_fault)
            .map_or(Ok(()), Err)
    }
    fn stop(&mut self) -> Result<(), PcmFault> {
        self.stops += 1;
        self.stop_fault.map_or(Ok(()), Err)
    }
}

fn wave() -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(
        &mut output,
        hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..14_i32 {
        writer.write_sample(i - 7).unwrap();
    }
    writer.finalize().unwrap();
    output.into_inner()
}

#[test]
fn capture_selects_channels_and_finalizes_a_wave_after_partial_reads() {
    let mut device = Device::new(3);
    let mut output = Cursor::new(Vec::new());
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::Pair34).unwrap();
    let report = capture_wave(&mut device, &mut output, request, || false).unwrap();
    assert_eq!(report.captured_frames, 7);
    assert!(!report.cancelled);
    assert_eq!(device.stops, 1);
    let mut reader = hound::WavReader::new(Cursor::new(output.into_inner())).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.spec().bits_per_sample, 24);
    assert_eq!(reader.duration(), 7);
    let actual: Vec<i32> = reader.samples::<i32>().map(Result::unwrap).collect();
    let expected: Vec<i32> = (0..7).flat_map(|i| [i * 4 - 3, i * 4 - 2]).collect();
    assert_eq!(actual, expected);
}

#[test]
fn playback_preserves_partial_writes_and_routes_a_stereo_pair() {
    let mut device = Device::new(2);
    let input = WaveInput::new(Cursor::new(wave())).unwrap();
    let report = play_wave(&mut device, input, AudioOutputs::Line, || false).unwrap();
    assert_eq!(report.played_frames, 7);
    let expected: Vec<i32> = (0..7)
        .flat_map(|i| [0, 0, (i * 2 - 7) << 8, (i * 2 - 6) << 8])
        .collect();
    assert_eq!(device.writes, expected);
    assert_eq!(device.stops, 1);
}

#[test]
fn duplex_keeps_the_recorded_and_played_frame_counts_equal() {
    let mut capture = Device::new(3);
    let mut playback = Device::new(2);
    let input = WaveInput::new(Cursor::new(wave())).unwrap();
    let mut output = Cursor::new(Vec::new());
    let report = duplex_wave(
        &mut capture,
        &mut playback,
        input,
        &mut output,
        AudioInputs::All,
        AudioOutputs::All,
        || false,
    )
    .unwrap();
    assert_eq!((report.captured_frames, report.played_frames), (7, 7));
    assert_eq!((capture.stops, playback.stops), (1, 1));
    let reader = hound::WavReader::new(Cursor::new(output.into_inner())).unwrap();
    assert_eq!(reader.duration(), 7);
    assert_eq!(reader.spec().channels, 4);
}

#[test]
fn retries_are_bounded_and_recoveries_are_reported() {
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let mut device = Device::new(3);
    device.read_faults.extend([PcmFault::Again, PcmFault::Xrun]);
    let report = capture_wave(&mut device, Cursor::new(Vec::new()), request, || false).unwrap();
    assert_eq!(report.recoveries, 1);
    assert_eq!(device.recoveries, 1);
    assert!(device.waits > 0);
    let mut stalled = Device::new(0);
    let error = capture_wave(&mut stalled, Cursor::new(Vec::new()), request, || false).unwrap_err();
    assert!(error.to_string().contains("progress"));
    assert!(stalled.waits <= 20);
    assert_eq!(stalled.stops, 1);
}

#[test]
fn disconnection_preserves_a_readable_recording_and_releases_the_device() {
    let mut device = Device::new(3);
    device.disconnect_at = Some(3);
    let mut output = Cursor::new(Vec::new());
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let error = capture_wave(&mut device, &mut output, request, || false).unwrap_err();
    assert!(error.to_string().contains("disconnected"));
    assert_eq!(device.stops, 1);
    let reader = hound::WavReader::new(Cursor::new(output.into_inner())).unwrap();
    assert_eq!(reader.duration(), 3);
}

#[test]
fn cancellation_keeps_complete_frames_and_finishes_the_wave_header() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut device = Device::new(3);
    device.cancel_after_read = Some(flag.clone());
    let mut output = Cursor::new(Vec::new());
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let report = capture_wave(&mut device, &mut output, request, || {
        flag.load(Ordering::SeqCst)
    })
    .unwrap();
    assert!(report.cancelled);
    assert_eq!(device.stops, 1);
    let reader = hound::WavReader::new(Cursor::new(output.into_inner())).unwrap();
    assert_eq!(u64::from(reader.duration()), report.captured_frames);
    assert!(reader.duration() < 7);
}

#[test]
fn primary_failures_survive_cleanup_errors_and_duplex_stops_both_directions() {
    let mut capture = Device::new(3);
    capture.read_faults.push_back(PcmFault::Disconnected);
    capture.stop_fault = Some(PcmFault::System(5));
    let mut playback = Device::new(2);
    playback.stop_fault = Some(PcmFault::System(9));
    let mut output = Cursor::new(Vec::new());
    let error = duplex_wave(
        &mut capture,
        &mut playback,
        WaveInput::new(Cursor::new(wave())).unwrap(),
        &mut output,
        AudioInputs::All,
        AudioOutputs::All,
        || false,
    )
    .unwrap_err();
    let motu_m4_audio::AudioError::Cleanup { primary, errors } = error else {
        panic!("expected primary and cleanup failures");
    };
    assert!(primary.unwrap().to_string().contains("disconnected"));
    assert_eq!(errors.len(), 2);
    assert_eq!((capture.stops, playback.stops), (1, 1));
    assert_eq!(
        hound::WavReader::new(Cursor::new(output.into_inner()))
            .unwrap()
            .duration(),
        0
    );
}

#[test]
fn repeated_recovery_failures_and_nonprogressing_drains_terminate() {
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let mut device = Device::new(3);
    device.read_faults.extend([PcmFault::Xrun; 4]);
    assert!(capture_wave(&mut device, Cursor::new(Vec::new()), request, || false).is_err());
    assert_eq!(device.recoveries, 3);
    assert_eq!(device.stops, 1);
    let mut device = Device::new(3);
    device.drain_fault = Some(PcmFault::Again);
    let error = play_wave(
        &mut device,
        WaveInput::new(Cursor::new(wave())).unwrap(),
        AudioOutputs::All,
        || false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("drain made no progress"));
    assert_eq!(device.waits, 20);
    assert_eq!(device.stops, 1);
}

#[test]
fn an_invalid_driver_count_never_indexes_past_the_capture_buffer() {
    let mut device = Device::new(3);
    device.oversized_read = true;
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let error = capture_wave(&mut device, Cursor::new(Vec::new()), request, || false).unwrap_err();
    assert!(error.to_string().contains("more frames"));
    assert_eq!(device.stops, 1);
}

#[test]
fn callback_unwinding_still_releases_the_pcm_stream() {
    let mut device = Device::new(3);
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture_wave(&mut device, Cursor::new(Vec::new()), request, || {
            panic!("cancel callback failed")
        })
    }));
    assert!(result.is_err());
    assert_eq!(device.stops, 1);
}

struct FullDisk;
impl std::io::Write for FullDisk {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk full"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl std::io::Seek for FullDisk {
    fn seek(&mut self, _: std::io::SeekFrom) -> std::io::Result<u64> {
        Ok(0)
    }
}

#[test]
fn a_recording_file_failure_stops_the_stream_and_reports_the_io_cause() {
    let mut device = Device::new(3);
    let request =
        CaptureRequest::new(SampleRate::new(48_000).unwrap(), 7, AudioInputs::All).unwrap();
    let error = capture_wave(&mut device, FullDisk, request, || false).unwrap_err();
    assert!(error.to_string().contains("disk full"));
    assert_eq!(device.stops, 1);
}

#[test]
fn a_completed_drain_wins_over_a_racing_poll_error() {
    let mut device = Device::new(3);
    device.drain_faults.push_back(PcmFault::Again);
    device.wait_faults.push_back(PcmFault::System(5));
    let report = play_wave(
        &mut device,
        WaveInput::new(Cursor::new(wave())).unwrap(),
        AudioOutputs::All,
        || false,
    )
    .unwrap();
    assert_eq!(report.played_frames, 7);
    assert_eq!(device.stops, 1);
    let mut device = Device::new(3);
    device.drain_fault = Some(PcmFault::Again);
    device.wait_faults.push_back(PcmFault::System(5));
    let error = play_wave(
        &mut device,
        WaveInput::new(Cursor::new(wave())).unwrap(),
        AudioOutputs::All,
        || false,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        motu_m4_audio::AudioError::Pcm {
            fault: PcmFault::System(5),
            ..
        }
    ));
    assert_eq!(device.stops, 1);
}
