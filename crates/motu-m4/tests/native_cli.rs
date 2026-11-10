#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "motu-native-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_motu-m4"));
    command.env(
        "ALSA_CONFIG_PATH",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../motu-m4-audio/tests/fixtures/alsa-null.conf"),
    );
    command
}

#[test]
fn executable_records_plays_and_duplexes_wav_through_native_alsa() {
    let scratch = Scratch::new();
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let recorded = scratch.0.join(format!("capture-{hz}.wav"));
        let output = command()
            .args([
                "audio",
                "capture",
                "--pcm",
                "null",
                "--rate",
                &hz.to_string(),
                "--frames",
                "513",
                "--output",
            ])
            .arg(&recorded)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("transport=alsa"));
        let mut reader = hound::WavReader::open(&recorded).unwrap();
        assert_eq!(reader.duration(), 513);
        assert_eq!(reader.spec().channels, 4);
        assert_eq!(reader.spec().sample_rate, hz);
        assert!(reader.samples::<i32>().all(|sample| sample.unwrap() == 0));
        let output = command()
            .args(["audio", "play", "--pcm", "null", "--input"])
            .arg(&recorded)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let captured = scratch.0.join(format!("duplex-{hz}.wav"));
        let output = command()
            .args(["audio", "duplex", "--pcm", "null", "--input"])
            .arg(&recorded)
            .arg("--output")
            .arg(&captured)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(hound::WavReader::open(&captured).unwrap().duration(), 513);
        let original = fs::read(&recorded).unwrap();
        let output = command()
            .args([
                "audio", "capture", "--pcm", "null", "--frames", "1", "--output",
            ])
            .arg(&recorded)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(&recorded).unwrap(), original);
    }
}

#[test]
fn native_cli_rejects_bad_arguments_without_creating_a_recording() {
    let scratch = Scratch::new();
    let path = scratch.0.join("absent.wav");
    for rate in ["0", "22050", "96001"] {
        let output = command()
            .args([
                "audio", "capture", "--pcm", "null", "--rate", rate, "--frames", "1", "--output",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!path.exists());
    }
    for args in [
        vec!["audio", "capture", "--frames", "0"],
        vec!["desktop", "volume", "--percent", "101"],
    ] {
        assert!(!command().args(args).output().unwrap().status.success());
    }
}

#[test]
fn capture_defaults_to_the_maximum_m4_sample_rate() {
    let scratch = Scratch::new();
    let path = scratch.0.join("capture.wav");
    let output = command()
        .args([
            "audio", "capture", "--pcm", "null", "--frames", "513", "--output",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 192_000);
    assert_eq!(reader.spec().bits_per_sample, 24);
    assert_eq!(reader.spec().channels, 4);
    assert_eq!(reader.duration(), 513);
}

#[test]
fn device_selection_rendering_and_disconnected_removal_use_the_requested_card() {
    let scratch = Scratch::new();
    for (index, id) in [(1, "M4"), (2, "M4_1")] {
        let proc = scratch.0.join(format!("asound/card{index}"));
        let sys = scratch.0.join(format!("sound/card{index}"));
        fs::create_dir_all(&proc).unwrap();
        fs::create_dir_all(sys.join("device")).unwrap();
        fs::write(proc.join("id"), id).unwrap();
        fs::write(proc.join("usbid"), "07fd:000b").unwrap();
        fs::write(sys.join("product"), "M4").unwrap();
    }
    let output = command()
        .args([
            "audio",
            "devices",
            "--json",
            "--card",
            "M4_1",
            "--asound-root",
        ])
        .arg(scratch.0.join("asound"))
        .arg("--sound-root")
        .arg(scratch.0.join("sound"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let cards: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["id"], "M4_1");
    let snapshot = scratch.0.join("graph.json");
    fs::write(
        &snapshot,
        include_bytes!("../../motu-m4-audio/tests/fixtures/pipewire-m4.json"),
    )
    .unwrap();
    let output = command()
        .args(["desktop", "render", "--card", "M4", "--asound-root"])
        .arg(scratch.0.join("asound"))
        .arg("--sound-root")
        .arg(scratch.0.join("sound"))
        .arg("--snapshot")
        .arg(&snapshot)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = String::from_utf8(output.stdout.clone()).unwrap();
    let configuration: serde_json::Value = serde_json::from_str(
        rendered
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n")
            .as_str(),
    )
    .unwrap();
    for module in configuration["context.modules"].as_array().unwrap() {
        let args = &module["args"];
        let exposed = if args["capture.props"]["media.class"] == "Audio/Sink" {
            &args["capture.props"]
        } else {
            &args["playback.props"]
        };
        assert_eq!(exposed["node.force-rate"], 192_000);
        assert_eq!(args["capture.props"]["resample.quality"], 14);
        assert_eq!(args["playback.props"]["resample.quality"], 14);
    }
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let override_output = command()
            .args(["desktop", "render", "--card", "M4", "--asound-root"])
            .arg(scratch.0.join("asound"))
            .arg("--sound-root")
            .arg(scratch.0.join("sound"))
            .arg("--snapshot")
            .arg(&snapshot)
            .arg("--rate")
            .arg(hz.to_string())
            .output()
            .unwrap();
        assert!(
            override_output.status.success(),
            "{}",
            String::from_utf8_lossy(&override_output.stderr)
        );
        let text = String::from_utf8(override_output.stdout).unwrap();
        let graph: serde_json::Value =
            serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
        for module in graph["context.modules"].as_array().unwrap() {
            let args = &module["args"];
            let exposed = if args["capture.props"]["media.class"] == "Audio/Sink" {
                &args["capture.props"]
            } else {
                &args["playback.props"]
            };
            assert_eq!(exposed["node.force-rate"], hz);
        }
    }
    let invalid = command()
        .args(["desktop", "render", "--rate", "22050"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--rate"));
    let directory = scratch.0.join("config/pipewire/pipewire.conf.d");
    fs::create_dir_all(&directory).unwrap();
    let installed = directory.join("60-motu-m4-M4.conf");
    fs::write(&installed, &output.stdout).unwrap();
    let unrelated = directory.join("another-device.conf");
    fs::write(&unrelated, b"keep").unwrap();
    fs::remove_dir_all(scratch.0.join("asound")).unwrap();
    let output = command()
        .args(["desktop", "remove", "--card", "M4", "--config-dir"])
        .arg(scratch.0.join("config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!installed.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
}
