#![cfg(target_os = "linux")]

use motu_m4_audio::{
    discover_cards, install_routing, DesktopAction, DesktopEndpoint, DesktopHost, PipeWireGraph,
    SampleRate, SystemDesktop, VolumePercent,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "motu-pipewire-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        for child in [
            "runtime",
            "runtime/pulse",
            "config/pipewire/pipewire.conf.d",
            "config/wireplumber/main.lua.d",
            "config/wireplumber/policy.lua.d",
            "asound/card1",
            "sound/card1/device",
            "state",
            "cache",
        ] {
            fs::create_dir_all(path.join(child)).unwrap();
        }
        fs::set_permissions(path.join("runtime"), fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Daemon(Child);
impl Daemon {
    fn spawn(root: &Path, program: &str, args: &[&str], log: &str) -> Self {
        let log = File::create(root.join(log)).unwrap();
        let child = Command::new(program)
            .args(args)
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env_remove("PIPEWIRE_RUNTIME_DIR")
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("PULSE_RUNTIME_PATH", root.join("runtime/pulse"))
            .env("GIO_USE_VFS", "local")
            .env_remove("PIPEWIRE_CONFIG_DIR")
            .env_remove("PIPEWIRE_REMOTE")
            .env_remove("WIREPLUMBER_CONFIG_DIR")
            .env_remove("WIREPLUMBER_DATA_DIR")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .process_group(0)
            .spawn()
            .unwrap();
        Self(child)
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", "--", &format!("-{}", self.0.id())])
            .status();
        let _ = self.0.wait();
    }
}

fn ready<T>(root: &Path, stage: &str, mut operation: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(result) = operation() {
            return result;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let logs: Vec<String> = [
                "pipewire.log",
                "main.log",
                "policy.log",
                "pulse.log",
                "playback.log",
                "clock.log",
            ]
            .iter()
            .map(|name| fs::read_to_string(root.join(name)).unwrap_or_default())
            .collect();
            let graph = client(root, "pw-dump").output().unwrap();
            let objects: Vec<Value> = serde_json::from_slice(&graph.stdout).unwrap_or_default();
            let state: Vec<Value> = objects.into_iter().filter_map(|object| match object["type"].as_str() {
                Some("PipeWire:Interface:Node") => Some(json!({"id": object["id"], "name": object["info"]["props"]["node.name"], "target": object["info"]["props"]["target.object"]})),
                Some("PipeWire:Interface:Link") => Some(json!({"link": object["info"]})),
                _ => None,
            }).collect();
            panic!("isolated PipeWire {stage} didn't become ready: {logs:?}; graph={state:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn client(root: &Path, program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env_remove("PIPEWIRE_RUNTIME_DIR")
        .env_remove("PIPEWIRE_REMOTE")
        .stdin(Stdio::null());
    command
}

fn route_links(objects: &[Value]) -> usize {
    let routes: std::collections::HashSet<u64> = objects
        .iter()
        .filter(|object| object["type"] == "PipeWire:Interface:Node")
        .filter(|object| {
            object["info"]["props"]["node.name"]
                .as_str()
                .is_some_and(|name| {
                    name.starts_with("capture.motu_m4.M4.")
                        || name.starts_with("playback.motu_m4.M4.")
                })
        })
        .filter_map(|object| object["id"].as_u64())
        .collect();
    objects
        .iter()
        .filter(|object| object["type"] == "PipeWire:Interface:Link")
        .filter(|object| {
            object["info"]["input-node-id"]
                .as_u64()
                .is_some_and(|id| routes.contains(&id))
                || object["info"]["output-node-id"]
                    .as_u64()
                    .is_some_and(|id| routes.contains(&id))
        })
        .count()
}

#[test]
fn generated_routes_load_link_and_accept_real_session_manager_controls() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    fs::write(root.join("asound/card1/id"), "M4").unwrap();
    fs::write(root.join("asound/card1/usbid"), "07fd:000b").unwrap();
    fs::write(root.join("sound/card1/product"), "M4").unwrap();
    let card = discover_cards(&root.join("asound"), &root.join("sound"))
        .unwrap()
        .remove(0);
    let graph = PipeWireGraph::parse(include_bytes!("fixtures/pipewire-m4.json")).unwrap();
    install_routing(
        &root.join("config"),
        &graph
            .routing(&card)
            .unwrap()
            .with_processing_rate(SampleRate::new(192_000).unwrap()),
    )
    .unwrap();
    let main = fs::read_to_string("/usr/share/pipewire/pipewire.conf").unwrap();
    fs::write(root.join("config/pipewire/pipewire.conf"), main).unwrap();
    let positions = json!(["AUX0", "AUX1", "AUX2", "AUX3"]);
    let fixture = json!({
        "context.modules": [{ "name": "libpipewire-module-loopback", "args": {
            "capture.props": { "target.object": "test.other-input", "audio.position": ["FL"], "stream.dont-remix": true, "node.passive": true, "node.dont-fallback": true },
            "playback.props": { "node.name": "test.other-microphone", "media.class": "Audio/Source", "audio.position": ["MONO"] }
        }}],
        "context.objects": [
            { "factory": "adapter", "args": {
                "factory.name": "support.null-audio-sink", "node.name": "alsa_output.M4.pro-output-0",
                "media.class": "Audio/Sink", "audio.channels": 4, "audio.position": positions,
                "adapter.auto-port-config": { "mode": "dsp", "monitor": true, "position": "preserve" }
            }},
            { "factory": "adapter", "args": {
                "factory.name": "support.null-audio-sink", "node.name": "alsa_input.M4.pro-input-0",
                "media.class": "Audio/Source/Virtual", "audio.channels": 4, "audio.position": positions,
                "monitor.passthrough": true,
                "adapter.auto-port-config": { "mode": "dsp", "monitor": true, "position": "preserve" }
            }},
            { "factory": "adapter", "args": {
                "factory.name": "support.null-audio-sink", "node.name": "test.other-output",
                "media.class": "Audio/Sink", "audio.position": ["FL", "FR"],
                "adapter.auto-port-config": { "mode": "dsp", "monitor": true, "position": "preserve" }
            }},
            { "factory": "adapter", "args": {
                "factory.name": "support.null-audio-sink", "node.name": "test.other-input",
                "media.class": "Audio/Source/Virtual", "audio.position": ["FL", "FR"],
                "monitor.passthrough": true,
                "adapter.auto-port-config": { "mode": "dsp", "monitor": true, "position": "preserve" }
            }}
        ]
    });
    fs::write(
        root.join("config/pipewire/pipewire.conf.d/10-virtual-m4.conf"),
        serde_json::to_vec_pretty(&fixture).unwrap(),
    )
    .unwrap();
    fs::write(root.join("config/wireplumber/main.lua.d/89-disable-monitors.lua"), "alsa_monitor.enabled = false\nv4l2_monitor.enabled = false\nlibcamera_monitor.enabled = false\n").unwrap();
    let _pipewire = Daemon::spawn(root, "pipewire", &[], "pipewire.log");
    ready(root, "native socket", || {
        root.join("runtime/pipewire-0").exists().then_some(())
    });
    let _main = Daemon::spawn(
        root,
        "dbus-run-session",
        &["--", "wireplumber", "-c", "main.conf"],
        "main.log",
    );
    let _policy = Daemon::spawn(
        root,
        "dbus-run-session",
        &["--", "wireplumber", "-c", "policy.conf"],
        "policy.log",
    );
    let mut host = SystemDesktop::in_runtime(&root.join("runtime"));
    let actual = ready(root, "channel links", || {
        let bytes = host.execute(DesktopAction::Dump).ok()?;
        let graph = PipeWireGraph::parse(&bytes).ok()?;
        if !DesktopEndpoint::ALL
            .into_iter()
            .all(|endpoint| graph.virtual_node(&card, endpoint).is_ok())
        {
            return None;
        }
        let objects: Vec<Value> = serde_json::from_slice(&bytes).ok()?;
        let links = route_links(&objects);
        (links >= 10).then_some(objects)
    });
    let names: HashMap<u64, &str> = actual
        .iter()
        .filter(|object| object["type"] == "PipeWire:Interface:Node")
        .map(|object| {
            (
                object["id"].as_u64().unwrap(),
                object["info"]["props"]["node.name"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(!actual
        .iter()
        .any(|object| object["type"] == "PipeWire:Interface:Device"));
    let ports: HashMap<u64, &Value> = actual
        .iter()
        .filter(|object| object["type"] == "PipeWire:Interface:Port")
        .map(|object| (object["id"].as_u64().unwrap(), object))
        .collect();
    let mut channel_links = Vec::new();
    for link in actual
        .iter()
        .filter(|object| object["type"] == "PipeWire:Interface:Link")
    {
        let info = &link["info"];
        let input = names
            .get(&info["input-node-id"].as_u64().unwrap())
            .copied()
            .unwrap();
        let output = names
            .get(&info["output-node-id"].as_u64().unwrap())
            .copied()
            .unwrap();
        let input_channel = ports[&info["input-port-id"].as_u64().unwrap()]["info"]["props"]
            ["audio.channel"]
            .as_str()
            .unwrap();
        let output_channel = ports[&info["output-port-id"].as_u64().unwrap()]["info"]["props"]
            ["audio.channel"]
            .as_str()
            .unwrap();
        if input == "alsa_output.M4.pro-output-0" || output == "alsa_input.M4.pro-input-0" {
            channel_links.push((
                input.to_owned(),
                output.to_owned(),
                input_channel.to_owned(),
                output_channel.to_owned(),
            ));
        }
    }
    assert_eq!(channel_links.len(), 10);
    for (input, output, input_channel, output_channel) in &channel_links {
        assert_eq!(input_channel, output_channel, "{input} <- {output}");
    }
    assert!(channel_links
        .iter()
        .any(
            |(input, output, input_channel, _)| input == "alsa_output.M4.pro-output-0"
                && output == "playback.motu_m4.M4.line"
                && input_channel == "AUX2"
        ));
    assert!(channel_links
        .iter()
        .any(
            |(input, output, _, output_channel)| input == "capture.motu_m4.M4.input_1"
                && output == "alsa_input.M4.pro-input-0"
                && output_channel == "AUX0"
        ));
    let silence = root.join("silence.wav");
    let mut writer = hound::WavWriter::create(
        &silence,
        hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..48_000 * 2 * 15 {
        writer.write_sample(0_i16).unwrap();
    }
    writer.finalize().unwrap();
    let playback = Daemon::spawn(
        root,
        "pw-play",
        &["--target=motu_m4.M4.main", silence.to_str().unwrap()],
        "playback.log",
    );
    ready(root, "active 192000 Hz processing", || {
        let output = client(root, "pw-top")
            .args(["--batch-mode", "--iterations=2"])
            .output()
            .ok()?;
        fs::write(root.join("clock.log"), &output.stdout).ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        text.lines()
            .any(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                fields.first() == Some(&"R")
                    && fields.get(3) == Some(&"192000")
                    && line.contains("alsa_output.M4.pro-output-0")
            })
            .then_some(())
    });
    drop(playback);
    let graph = host.snapshot().unwrap();
    let main = graph.virtual_node(&card, DesktopEndpoint::Main).unwrap();
    host.execute(DesktopAction::Default(main)).unwrap();
    host.execute(DesktopAction::Volume {
        node: main,
        percent: VolumePercent::new(37).unwrap(),
    })
    .unwrap();
    host.execute(DesktopAction::Mute {
        node: main,
        muted: true,
    })
    .unwrap();
    let volume = host.execute(DesktopAction::GetVolume(main)).unwrap();
    let volume = String::from_utf8(volume).unwrap();
    assert!(volume.contains("0.37"), "{volume}");
    assert!(volume.contains("MUTED"), "{volume}");
    host.execute(DesktopAction::Mute {
        node: main,
        muted: false,
    })
    .unwrap();
    let input = graph.virtual_node(&card, DesktopEndpoint::Input1).unwrap();
    host.execute(DesktopAction::Default(input)).unwrap();
    let _pulse = Daemon::spawn(root, "pipewire-pulse", &[], "pulse.log");
    ready(root, "PulseAudio socket", || {
        root.join("runtime/pulse/native").exists().then_some(())
    });
    for (kind, endpoints) in [
        ("sinks", vec!["motu_m4.M4.main", "motu_m4.M4.line"]),
        (
            "sources",
            vec![
                "motu_m4.M4.input_1",
                "motu_m4.M4.input_2",
                "motu_m4.M4.inputs_1_2",
                "motu_m4.M4.inputs_3_4",
                "motu_m4.M4.main.monitor",
            ],
        ),
    ] {
        ready(root, "PulseAudio endpoints", || {
            let output = Command::new("pactl")
                .args(["list", "short", kind])
                .env(
                    "PULSE_SERVER",
                    format!("unix:{}/runtime/pulse/native", root.display()),
                )
                .stdin(Stdio::null())
                .output()
                .ok()?;
            let text = String::from_utf8(output.stdout).ok()?;
            (output.status.success() && endpoints.iter().all(|endpoint| text.contains(endpoint)))
                .then_some(())
        });
    }

    for name in ["alsa_input.M4.pro-input-0", "alsa_output.M4.pro-output-0"] {
        let id = names
            .iter()
            .find_map(|(id, value)| (*value == name).then_some(id))
            .unwrap();
        let result = client(root, "pw-cli")
            .args(["destroy", &id.to_string()])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for name in ["test.other-output", "test.other-microphone"] {
        let id = names
            .iter()
            .find_map(|(id, value)| (*value == name).then_some(id))
            .unwrap();
        let output = client(root, "wpctl")
            .args(["set-default", &id.to_string()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    ready(root, "hardware removal", || {
        let bytes = host.execute(DesktopAction::Dump).ok()?;
        let objects: Vec<Value> = serde_json::from_slice(&bytes).ok()?;
        let links = route_links(&objects);
        let graph = PipeWireGraph::parse(&bytes).ok()?;
        (links == 0
            && DesktopEndpoint::ALL
                .into_iter()
                .all(|endpoint| graph.virtual_node(&card, endpoint).is_err()))
        .then_some(())
    });
    drop(_pulse);
    drop(_policy);
    drop(_main);
    drop(_pipewire);
    let _pipewire = Daemon::spawn(root, "pipewire", &[], "pipewire.log");
    ready(root, "restarted socket", || {
        root.join("runtime/pipewire-0").exists().then_some(())
    });
    let _main = Daemon::spawn(
        root,
        "dbus-run-session",
        &["--", "wireplumber", "-c", "main.conf"],
        "main.log",
    );
    let _policy = Daemon::spawn(
        root,
        "dbus-run-session",
        &["--", "wireplumber", "-c", "policy.conf"],
        "policy.log",
    );
    ready(root, "restarted routes", || {
        let bytes = host.execute(DesktopAction::Dump).ok()?;
        let objects: Vec<Value> = serde_json::from_slice(&bytes).ok()?;
        let graph = PipeWireGraph::parse(&bytes).ok()?;
        (route_links(&objects) == 10
            && DesktopEndpoint::ALL
                .into_iter()
                .all(|endpoint| graph.virtual_node(&card, endpoint).is_ok()))
        .then_some(())
    });
}
