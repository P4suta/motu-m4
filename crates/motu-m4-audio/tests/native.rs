#![cfg(target_os = "linux")]

use motu_m4_audio::{
    capture_wave, duplex_wave, play_wave, AudioInputs, AudioOutputs, CaptureRequest, NativePcm,
    SampleRate, WaveInput,
};
use motu_m4_audio::{PcmFault, PcmStream};
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

fn isolated(test: &str) -> bool {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alsa-null.conf");
    if std::env::var_os("ALSA_CONFIG_PATH").as_deref() == Some(config.as_os_str()) {
        return false;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("ALSA_CONFIG_PATH", config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn alsa_null_transfers_real_library_buffers_at_every_m4_rate() {
    if isolated("alsa_null_transfers_real_library_buffers_at_every_m4_rate") {
        return;
    }
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let rate = SampleRate::new(hz).unwrap();
        let mut device = NativePcm::capture("null", rate).unwrap();
        let mut output = Cursor::new(Vec::new());
        let request = CaptureRequest::new(rate, 513, AudioInputs::All).unwrap();
        let report = capture_wave(&mut device, &mut output, request, || false).unwrap();
        assert_eq!(report.captured_frames, 513);
        let bytes = output.into_inner();
        let mut reader = hound::WavReader::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(reader.duration(), 513);
        assert_eq!(reader.spec().sample_rate, hz);
        assert!(reader.samples::<i32>().all(|sample| sample.unwrap() == 0));
        let mut device = NativePcm::playback("null", rate).unwrap();
        let report = play_wave(
            &mut device,
            WaveInput::new(Cursor::new(bytes.clone())).unwrap(),
            AudioOutputs::All,
            || false,
        )
        .unwrap();
        assert_eq!(report.played_frames, 513);
        let mut capture = NativePcm::capture("null", rate).unwrap();
        let mut playback = NativePcm::playback("null", rate).unwrap();
        let report = duplex_wave(
            &mut capture,
            &mut playback,
            WaveInput::new(Cursor::new(bytes)).unwrap(),
            Cursor::new(Vec::new()),
            AudioInputs::All,
            AudioOutputs::All,
            || false,
        )
        .unwrap();
        assert_eq!((report.captured_frames, report.played_frames), (513, 513));
    }
}

#[test]
fn native_calls_reject_wrong_directions_incomplete_frames_and_zero_bytes() {
    if isolated("native_calls_reject_wrong_directions_incomplete_frames_and_zero_bytes") {
        return;
    }
    let rate = SampleRate::new(48_000).unwrap();
    assert!(NativePcm::capture("bad\0name", rate).is_err());
    let mut capture = NativePcm::capture("null", rate).unwrap();
    let mut playback = NativePcm::playback("null", rate).unwrap();
    assert!(matches!(capture.write(&[0; 4]), Err(PcmFault::System(_))));
    assert!(matches!(
        playback.read(&mut [0; 4]),
        Err(PcmFault::System(_))
    ));
    assert!(matches!(
        capture.read(&mut [0; 3]),
        Err(PcmFault::System(_))
    ));
    assert!(matches!(playback.write(&[0; 3]), Err(PcmFault::System(_))));
}
