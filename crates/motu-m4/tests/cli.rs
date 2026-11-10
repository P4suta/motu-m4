use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "motu-m4-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_motu-m4"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn executable_captures_and_replays_samples_through_the_simulated_device() {
    let scratch = Scratch::new();
    let output = scratch.run(&[
        "simulate",
        "capture",
        "--frames",
        "19",
        "--scenario",
        "drift",
        "--output",
        "take.raw",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let audio = fs::read(scratch.0.join("take.raw")).unwrap();
    let expected: Vec<u8> = (0u32..76)
        .flat_map(|sample| (sample * 256).to_le_bytes())
        .collect();
    assert_eq!(audio, expected);
    let output = scratch.run(&[
        "simulate",
        "play",
        "--input",
        "take.raw",
        "--received",
        "device.raw",
        "--scenario",
        "drift",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = fs::read(scratch.0.join("device.raw")).unwrap();
    assert_eq!(&received[..audio.len()], audio);
    assert!(received[audio.len()..].iter().all(|byte| *byte == 0));
    assert_eq!(received.len(), 384);
}

#[test]
fn malformed_feedback_fails_and_restores_the_simulated_device() {
    let scratch = Scratch::new();
    let output = scratch.run(&[
        "simulate",
        "capture",
        "--frames",
        "19",
        "--scenario",
        "malformed",
        "--output",
        "take.raw",
    ]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Packet"), "{error}");
    assert!(error.contains("attached=true"), "{error}");
    assert!(error.contains("clock=44100"), "{error}");
    assert!(!scratch.0.join("take.raw").exists());
}

#[test]
fn help_and_invalid_commands_follow_the_exit_code_contract() {
    let scratch = Scratch::new();
    let help = scratch.run(&["--help"]);
    assert!(help.status.success());
    assert!(!help.stdout.contains(&0x1b));
    assert!(String::from_utf8_lossy(&help.stdout).contains("simulate"));
    for args in [
        vec!["capture", "--takeover"],
        vec![
            "simulate", "capture", "--frames", "0", "--output", "bad.raw",
        ],
        vec![
            "simulate", "capture", "--frames", "480001", "--output", "bad.raw",
        ],
    ] {
        assert_eq!(scratch.run(&args).status.code(), Some(2));
    }
    assert!(!scratch.0.join("bad.raw").exists());
}

#[test]
fn capture_refuses_to_overwrite_an_existing_file() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("take.raw"), b"keep").unwrap();
    let output = scratch.run(&[
        "simulate", "capture", "--frames", "1", "--output", "take.raw",
    ]);
    assert!(!output.status.success());
    assert_eq!(fs::read(scratch.0.join("take.raw")).unwrap(), b"keep");
}

#[test]
fn malformed_audio_is_rejected_before_a_received_file_is_created() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("bad.raw"), [0; 15]).unwrap();
    let output = scratch.run(&[
        "simulate",
        "play",
        "--input",
        "bad.raw",
        "--received",
        "device.raw",
    ]);
    assert!(!output.status.success());
    assert!(!scratch.0.join("device.raw").exists());
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../motu-m4-core/tests/fixtures/m4-usb-descriptors.bin")
}

#[test]
fn probe_validates_a_fixture_and_rejects_a_different_product() {
    let scratch = Scratch::new();
    fs::copy(fixture(), scratch.0.join("device.bin")).unwrap();
    let output = scratch.run(&["probe", "--descriptors", "device.bin"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary = String::from_utf8_lossy(&output.stdout);
    assert!(summary.contains("playback=0x02"));
    assert!(summary.contains("capture=0x83"));
    assert!(summary.contains("access=read-only"));
    assert!(!scratch
        .run(&["probe", "--descriptors", "device.bin", "--product", "M2"])
        .status
        .success());
    fs::write(scratch.0.join("broken.bin"), [0; 32]).unwrap();
    assert!(!scratch
        .run(&["probe", "--descriptors", "broken.bin"])
        .status
        .success());
}

#[test]
fn probe_reads_virtual_sysfs_without_a_physical_device() {
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.0.join("usb/1-1:1.0")).unwrap();
    for (name, vendor, product) in [
        ("1-1", "07fd\n", "M4\n"),
        ("1-2", "07fd\n", "M2\n"),
        ("1-3", "1234\n", "M4\n"),
        ("1-4", "07fd\n", "M4\n"),
    ] {
        let path = scratch.0.join("usb").join(name);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("idVendor"), vendor).unwrap();
        fs::write(path.join("product"), product).unwrap();
        fs::copy(fixture(), path.join("descriptors")).unwrap();
    }
    let output = scratch.run(&["probe", "--sysfs-root", "usb"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 2);
    fs::remove_dir_all(scratch.0.join("usb/1-1")).unwrap();
    fs::remove_dir_all(scratch.0.join("usb/1-4")).unwrap();
    assert!(!scratch
        .run(&["probe", "--sysfs-root", "usb"])
        .status
        .success());
    fs::write(scratch.0.join("usb/1-2/idVendor"), [0xff]).unwrap();
    assert!(!scratch
        .run(&["probe", "--sysfs-root", "usb"])
        .status
        .success());
}

#[test]
fn discovery_reports_read_errors_even_when_another_device_is_valid() {
    let scratch = Scratch::new();
    let valid = scratch.0.join("usb/1-1");
    let invalid = scratch.0.join("usb/1-2");
    fs::create_dir_all(&valid).unwrap();
    fs::create_dir_all(&invalid).unwrap();
    fs::write(valid.join("idVendor"), "07fd\n").unwrap();
    fs::write(valid.join("product"), "M4\n").unwrap();
    fs::copy(fixture(), valid.join("descriptors")).unwrap();
    fs::write(invalid.join("idVendor"), [0xff]).unwrap();
    let output = scratch.run(&["probe", "--sysfs-root", "usb"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8"));
}

#[test]
fn playback_accepts_the_size_limit_and_rejects_an_oversized_file() {
    let scratch = Scratch::new();
    let path = scratch.0.join("large.raw");
    let file = fs::File::create(&path).unwrap();
    file.set_len(7_680_000).unwrap();
    let output = scratch.run(&["simulate", "play", "--input", "large.raw"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("played=480000"));
    let duplex = scratch.run(&[
        "simulate",
        "duplex",
        "--input",
        "large.raw",
        "--output",
        "limit.raw",
    ]);
    assert!(
        duplex.status.success(),
        "{}",
        String::from_utf8_lossy(&duplex.stderr)
    );
    assert_eq!(
        fs::metadata(scratch.0.join("limit.raw")).unwrap().len(),
        7_680_000
    );
    file.set_len(7_680_001).unwrap();
    let output = scratch.run(&["simulate", "play", "--input", "large.raw"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("input exceeds"));
    let duplex = scratch.run(&[
        "simulate",
        "duplex",
        "--input",
        "large.raw",
        "--output",
        "oversized.raw",
    ]);
    assert!(!duplex.status.success());
    assert!(String::from_utf8_lossy(&duplex.stderr).contains("input exceeds"));
    assert!(!scratch.0.join("oversized.raw").exists());
}

#[test]
fn executable_fault_scenarios_report_failure_without_creating_audio() {
    let scratch = Scratch::new();
    for scenario in [
        "disconnect",
        "timeout",
        "stalled",
        "short-control",
        "unsupported-rate",
        "malformed-ranges",
        "invalid-clock",
        "clock-mismatch",
    ] {
        let output = scratch.run(&[
            "simulate",
            "capture",
            "--frames",
            "19",
            "--scenario",
            scenario,
            "--output",
            "take.raw",
        ]);
        assert!(!output.status.success(), "{scenario}");
        assert!(!scratch.0.join("take.raw").exists());
        if scenario == "disconnect" {
            assert!(String::from_utf8_lossy(&output.stderr).contains("attached=false"));
        }
    }
}

#[test]
fn executable_duplex_and_cancellation_cover_every_sample_rate() {
    let scratch = Scratch::new();
    let source: Vec<u8> = (0u32..76)
        .flat_map(|sample| (sample << 8).to_le_bytes())
        .collect();
    fs::write(scratch.0.join("input.raw"), &source).unwrap();
    for rate in ["44100", "48000", "88200", "96000", "176400", "192000"] {
        let captured = format!("capture-{rate}.raw");
        let received = format!("received-{rate}.raw");
        let output = scratch.run(&[
            "simulate",
            "duplex",
            "--input",
            "input.raw",
            "--output",
            &captured,
            "--received",
            &received,
            "--rate",
            rate,
            "--scenario",
            "negative-drift",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("rate={rate}")));
        assert_eq!(fs::read(scratch.0.join(captured)).unwrap(), source);
        let played = fs::read(scratch.0.join(received)).unwrap();
        assert_eq!(&played[..source.len()], source);
        assert!(played[source.len()..].iter().all(|byte| *byte == 0));
        let cancelled = scratch.run(&[
            "simulate",
            "duplex",
            "--input",
            "input.raw",
            "--output",
            "cancelled.raw",
            "--rate",
            rate,
            "--cancel-after-packets",
            "0",
        ]);
        assert_eq!(cancelled.status.code(), Some(1));
        let error = String::from_utf8_lossy(&cancelled.stderr);
        assert!(error.contains("Cancelled"));
        assert!(error.contains("attached=true"));
        assert!(!scratch.0.join("cancelled.raw").exists());
    }
}

#[test]
fn invalid_rates_fail_argument_validation_and_duplex_outputs_are_not_partial() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("input.raw"), [0; 304]).unwrap();
    for rate in ["0", "12345", "48001", "4294967296"] {
        let output = scratch.run(&[
            "simulate", "capture", "--frames", "1", "--rate", rate, "--output", "bad.raw",
        ]);
        assert_eq!(output.status.code(), Some(2));
        assert!(!scratch.0.join("bad.raw").exists());
    }
    fs::write(scratch.0.join("existing.raw"), b"keep").unwrap();
    let output = scratch.run(&[
        "simulate",
        "duplex",
        "--input",
        "input.raw",
        "--output",
        "capture.raw",
        "--received",
        "existing.raw",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(scratch.0.join("existing.raw")).unwrap(), b"keep");
    assert!(!scratch.0.join("capture.raw").exists());
    let output = scratch.run(&[
        "simulate",
        "duplex",
        "--input",
        "input.raw",
        "--output",
        "same.raw",
        "--received",
        "same.raw",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!scratch.0.join("same.raw").exists());
}

#[test]
fn executable_uses_firmware_profiles_for_capture_playback_duplex_and_cancellation() {
    let scratch = Scratch::new();
    let expected: Vec<u8> = (0_u32..76)
        .flat_map(|sample| (sample << 8).to_le_bytes())
        .collect();
    fs::write(scratch.0.join("source.raw"), &expected).unwrap();
    for profile in [
        "recorded",
        "atmel-template",
        "nxp-template",
        "original-synthetic",
    ] {
        for mode in ["capture", "play", "duplex"] {
            let mut args = vec!["simulate", mode, "--profile", profile, "--rate", "44100"];
            if mode == "capture" {
                args.extend(["--frames", "19"]);
            } else {
                args.extend(["--input", "source.raw", "--received", "played.raw"]);
            }
            if mode != "play" {
                args.extend(["--output", "captured.raw"]);
            }
            let result = scratch.run(&args);
            assert!(
                result.status.success(),
                "{profile} {mode}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            if mode != "play" {
                assert_eq!(fs::read(scratch.0.join("captured.raw")).unwrap(), expected);
                fs::remove_file(scratch.0.join("captured.raw")).unwrap();
            }
            if mode != "capture" {
                let audio = fs::read(scratch.0.join("played.raw")).unwrap();
                assert_eq!(&audio[..expected.len()], expected);
                assert!(audio[expected.len()..].iter().all(|byte| *byte == 0));
                fs::remove_file(scratch.0.join("played.raw")).unwrap();
            }
            args.extend(["--cancel-after-packets", "1"]);
            assert!(!scratch.run(&args).status.success());
            assert!(!scratch.0.join("captured.raw").exists());
            assert!(!scratch.0.join("played.raw").exists());
        }
    }
    assert_eq!(
        scratch
            .run(&[
                "simulate",
                "capture",
                "--profile",
                "unknown",
                "--output",
                "bad.raw"
            ])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn firmware_inspection_reads_only_the_supplied_file_and_refuses_bad_headers() {
    let scratch = Scratch::new();
    let mut image = vec![0; 72];
    image[..4].copy_from_slice(&8_u32.to_le_bytes());
    image[4..13].copy_from_slice(b"2.2.9+641");
    fs::write(scratch.0.join("firmware.image"), &image).unwrap();
    let result = scratch.run(&["inspect-firmware", "--input", "firmware.image"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary = String::from_utf8_lossy(&result.stdout);
    assert!(summary.contains("version=2.2.9+641"));
    assert!(summary.contains("payload=8"));
    assert!(summary.contains("access=read-only"));
    assert_eq!(fs::read(scratch.0.join("firmware.image")).unwrap(), image);
    for length in [0, 63, 71, 73, 1_048_577] {
        fs::write(scratch.0.join("bad.image"), vec![0; length]).unwrap();
        assert!(!scratch
            .run(&["inspect-firmware", "--input", "bad.image"])
            .status
            .success());
    }
    assert!(!scratch
        .run(&["inspect-firmware", "--input", "missing.image"])
        .status
        .success());
}
