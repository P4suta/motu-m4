use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../.firmware-corpus/{name}.image"))
}

#[test]
#[ignore = "Requires verified images exported with just firmware-corpus"]
fn executable_uses_official_firmware_for_control_and_simulated_audio() {
    let scratch = std::env::temp_dir().join(format!("motu-firmware-cli-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    let expected: Vec<u8> = (0_u32..76)
        .flat_map(|sample| (sample << 8).to_le_bytes())
        .collect();
    fs::write(scratch.join("input.raw"), &expected).unwrap();
    fs::write(scratch.join("long.raw"), vec![0; 307 * 16]).unwrap();
    for name in ["atmel", "nxp"] {
        for rate in ["44100", "48000", "88200", "96000", "176400", "192000"] {
            for mode in ["capture", "play", "duplex"] {
                let mut command = Command::new(env!("CARGO_BIN_EXE_motu-m4"));
                command
                    .current_dir(&scratch)
                    .args(["simulate", mode, "--rate", rate, "--firmware"])
                    .arg(corpus(name));
                if mode == "capture" {
                    command.args(["--frames", "19"]);
                } else {
                    command.args(["--input", "input.raw", "--received", "received.raw"]);
                }
                if mode != "play" {
                    command.args(["--output", "capture.raw"]);
                }
                let result = command.output().unwrap();
                assert!(
                    result.status.success(),
                    "{name} {rate} {mode}: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert!(String::from_utf8_lossy(&result.stdout)
                    .contains("firmware-control+simulated-audio"));
                if mode != "play" {
                    assert_eq!(fs::read(scratch.join("capture.raw")).unwrap(), expected);
                    fs::remove_file(scratch.join("capture.raw")).unwrap();
                }
                if mode != "capture" {
                    let played = fs::read(scratch.join("received.raw")).unwrap();
                    assert_eq!(&played[..expected.len()], expected);
                    assert!(played[expected.len()..].iter().all(|byte| *byte == 0));
                    fs::remove_file(scratch.join("received.raw")).unwrap();
                }
                let mut cancel = Command::new(env!("CARGO_BIN_EXE_motu-m4"));
                cancel
                    .current_dir(&scratch)
                    .args(["simulate", mode, "--rate", rate, "--firmware"])
                    .arg(corpus(name))
                    .args(["--cancel-after-packets", "1"]);
                if mode == "capture" {
                    cancel.args(["--frames", "307"]);
                } else {
                    cancel.args(["--input", "long.raw", "--received", "received.raw"]);
                }
                if mode != "play" {
                    cancel.args(["--output", "capture.raw"]);
                }
                let cancelled = cancel.output().unwrap();
                assert!(!cancelled.status.success());
                assert!(String::from_utf8_lossy(&cancelled.stderr)
                    .contains("attached=true, clock=48000"));
                assert!(!scratch.join("capture.raw").exists());
                assert!(!scratch.join("received.raw").exists());
            }
        }
    }
    fs::remove_dir_all(scratch).unwrap();
}
