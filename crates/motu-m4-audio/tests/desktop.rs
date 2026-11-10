use motu_m4_audio::{
    discover_cards, install_routing, remove_routing, DesktopAction, DesktopEndpoint, PipeWireGraph,
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const SNAPSHOT: &str = include_str!("fixtures/pipewire-m4.json");
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "motu-desktop-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("asound/card1")).unwrap();
        fs::create_dir_all(path.join("sound/card1/device")).unwrap();
        fs::write(path.join("asound/card1/usbid"), "07fd:000b").unwrap();
        fs::write(path.join("asound/card1/id"), "M4").unwrap();
        fs::write(path.join("sound/card1/product"), "M4").unwrap();
        Self(path)
    }
    fn card(&self) -> motu_m4_audio::M4Card {
        discover_cards(&self.0.join("asound"), &self.0.join("sound"))
            .unwrap()
            .remove(0)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn routing_selects_actual_channel_positions_and_never_falls_back_to_other_devices() {
    let scratch = Scratch::new();
    let card = scratch.card();
    let graph = PipeWireGraph::parse(SNAPSHOT.as_bytes()).unwrap();
    let routing = graph.routing(&card).unwrap();
    let text = routing.render().unwrap();
    let config: Value =
        serde_json::from_str(text.lines().skip(1).collect::<Vec<_>>().join("\n").as_str()).unwrap();
    let modules = config["context.modules"].as_array().unwrap();
    assert_eq!(modules.len(), 6);
    assert_eq!(
        modules[0]["args"]["capture.props"]["media.class"],
        "Audio/Sink"
    );
    assert_eq!(
        modules[0]["args"]["playback.props"]["audio.position"],
        json!(["AUX0", "AUX1"])
    );
    assert_eq!(
        modules[1]["args"]["playback.props"]["audio.position"],
        json!(["AUX2", "AUX3"])
    );
    assert_eq!(
        modules[2]["args"]["capture.props"]["audio.position"],
        json!(["AUX0"])
    );
    assert_eq!(
        modules[2]["args"]["playback.props"]["audio.position"],
        json!(["MONO"])
    );
    for (index, module) in modules.iter().enumerate() {
        assert_eq!(module["name"], "libpipewire-module-loopback");
        let target = if index < 2 {
            &module["args"]["playback.props"]
        } else {
            &module["args"]["capture.props"]
        };
        assert_eq!(target["node.dont-fallback"], true);
        assert_eq!(target["node.dont-reconnect"], true);
        assert_eq!(target["stream.dont-remix"], true);
        assert_eq!(target["node.passive"], true);
    }
    let selection = graph.pro_audio_profile(&card).unwrap();
    assert_eq!(
        DesktopAction::Profile(selection).arguments(),
        ["set-profile", "42", "17"]
    );
    let node = graph.virtual_node(&card, DesktopEndpoint::Main).unwrap();
    assert_eq!(
        DesktopAction::Default(node).arguments(),
        ["set-default", "71"]
    );
}

#[test]
fn installation_is_private_idempotent_and_preserves_existing_configuration() {
    let scratch = Scratch::new();
    let card = scratch.card();
    let routing = PipeWireGraph::parse(SNAPSHOT.as_bytes())
        .unwrap()
        .routing(&card)
        .unwrap();
    let directory = scratch.0.join("config");
    let path = install_routing(&directory, &routing).unwrap();
    let contents = fs::read(&path).unwrap();
    assert_eq!(install_routing(&directory, &routing).unwrap(), path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::write(&path, b"user configuration\n").unwrap();
    assert!(install_routing(&directory, &routing).is_err());
    assert!(remove_routing(&directory, &card).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"user configuration\n");
    fs::write(&path, contents).unwrap();
    assert!(remove_routing(&directory, &card).unwrap());
    assert!(!remove_routing(&directory, &card).unwrap());
}

#[test]
fn malformed_or_ambiguous_graphs_cannot_select_a_hardware_endpoint() {
    let scratch = Scratch::new();
    let card = scratch.card();
    assert!(PipeWireGraph::parse(b"{}").is_err());
    assert!(PipeWireGraph::parse(b"[{\"id\":-1,\"type\":\"PipeWire:Interface:Node\"}]").is_err());
    for replacement in [json!(2), json!("two"), json!(null)] {
        let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
        snapshot[1]["info"]["props"]["audio.channels"] = replacement;
        let graph =
            PipeWireGraph::parse(serde_json::to_string(&snapshot).unwrap().as_bytes()).unwrap();
        assert!(graph.routing(&card).is_err());
    }
    let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
    snapshot[1]["info"]["props"]["audio.position"] = json!(["AUX0", "AUX0", "AUX2", "AUX3"]);
    assert!(
        PipeWireGraph::parse(serde_json::to_string(&snapshot).unwrap().as_bytes())
            .unwrap()
            .routing(&card)
            .is_err()
    );
    let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
    let duplicate = snapshot[1].clone();
    snapshot.as_array_mut().unwrap().push(duplicate);
    assert!(PipeWireGraph::parse(serde_json::to_string(&snapshot).unwrap().as_bytes()).is_err());
}

#[test]
fn native_json_encoded_channel_positions_preserve_the_layout() {
    let scratch = Scratch::new();
    let card = scratch.card();
    let mut snapshot: Value = serde_json::from_str(SNAPSHOT).unwrap();
    snapshot[1]["info"]["props"]["audio.position"] =
        json!("[\n \"AUX0\", \"AUX1\", \"AUX2\", \"AUX3\"\n]");
    let graph = PipeWireGraph::parse(serde_json::to_string(&snapshot).unwrap().as_bytes()).unwrap();
    assert!(graph.routing(&card).is_ok());
}
