use crate::{AudioError, M4Card, SampleRate};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

pub(crate) const MARKER: &str = "# Managed by motu-m4 desktop.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileSelection {
    device: u32,
    index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEndpoint {
    Main,
    Line,
    Input1,
    Input2,
    Inputs12,
    Inputs34,
}

impl DesktopEndpoint {
    pub const ALL: [Self; 6] = [
        Self::Main,
        Self::Line,
        Self::Input1,
        Self::Input2,
        Self::Inputs12,
        Self::Inputs34,
    ];
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Line => "line",
            Self::Input1 => "input_1",
            Self::Input2 => "input_2",
            Self::Inputs12 => "inputs_1_2",
            Self::Inputs34 => "inputs_3_4",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Main => "Main outputs 1-2",
            Self::Line => "Line outputs 3-4",
            Self::Input1 => "Microphone input 1",
            Self::Input2 => "Microphone input 2",
            Self::Inputs12 => "Inputs 1-2",
            Self::Inputs34 => "Line inputs 3-4",
        }
    }
    pub fn is_output(self) -> bool {
        matches!(self, Self::Main | Self::Line)
    }
    pub fn node_name(self, card: &M4Card) -> String {
        format!("motu_m4.{}.{}", card.id(), self.suffix())
    }
}

#[derive(Deserialize)]
struct Object {
    id: u32,
    #[serde(rename = "type")]
    kind: String,
    info: Option<Info>,
}

#[derive(Deserialize)]
struct Info {
    #[serde(default)]
    props: Map<String, Value>,
    #[serde(default)]
    params: Map<String, Value>,
}

pub struct PipeWireGraph {
    objects: Vec<Object>,
}

fn number(value: Option<&Value>) -> Option<u32> {
    match value? {
        Value::Number(value) => u32::try_from(value.as_u64()?).ok(),
        Value::String(value) => {
            if let Some(hex) = value.strip_prefix("0x") {
                u32::from_str_radix(hex, 16).ok()
            } else {
                value.parse().ok()
            }
        }
        _ => None,
    }
}

impl PipeWireGraph {
    pub fn parse(bytes: &[u8]) -> Result<Self, AudioError> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(AudioError::Invalid("PipeWire snapshot exceeds 8 MiB"));
        }
        let objects: Vec<Object> = serde_json::from_slice(bytes)
            .map_err(|error| AudioError::Desktop(format!("invalid pw-dump response: {error}")))?;
        if objects.len() > 50_000 {
            return Err(AudioError::Invalid(
                "PipeWire snapshot has too many objects",
            ));
        }
        let mut ids = HashSet::new();
        for object in &objects {
            if !ids.insert(object.id) {
                return Err(AudioError::Invalid(
                    "PipeWire snapshot repeats an object ID",
                ));
            }
        }
        Ok(Self { objects })
    }

    fn device(&self, card: &M4Card) -> Result<&Object, AudioError> {
        let mut matches = self.objects.iter().filter(|object| {
            object.kind == "PipeWire:Interface:Device"
                && object.info.as_ref().is_some_and(|info| {
                    info.props.get("device.api").and_then(Value::as_str) == Some("alsa")
                        && number(info.props.get("api.alsa.card")) == Some(card.index())
                })
        });
        let device = matches.next().ok_or(AudioError::Invalid(
            "M4 isn't present in PipeWire; check the user audio session",
        ))?;
        if device.id == 0 || matches.next().is_some() {
            return Err(AudioError::Invalid(
                "PipeWire M4 device identity is ambiguous",
            ));
        }
        let info = device
            .info
            .as_ref()
            .ok_or(AudioError::Invalid("PipeWire device has no properties"))?;
        for (key, expected) in [
            ("device.vendor.id", 0x07fd),
            ("device.product.id", u32::from(card.usb_product())),
        ] {
            if let Some(value) = info.props.get(key) {
                if number(Some(value)) != Some(expected) {
                    return Err(AudioError::Invalid(
                        "PipeWire device identity differs from the M4 card",
                    ));
                }
            }
        }
        Ok(device)
    }

    pub fn pro_audio_profile(&self, card: &M4Card) -> Result<ProfileSelection, AudioError> {
        let device = self.device(card)?;
        let profiles = device
            .info
            .as_ref()
            .and_then(|info| info.params.get("EnumProfile"))
            .and_then(Value::as_array)
            .ok_or(AudioError::Invalid(
                "PipeWire didn't publish device profiles",
            ))?;
        let mut profiles = profiles.iter().filter(|profile| {
            profile.get("name").and_then(Value::as_str) == Some("pro-audio")
                && profile.get("available").and_then(Value::as_str) != Some("no")
        });
        let profile = profiles.next().ok_or(AudioError::Invalid(
            "the M4 pro-audio profile isn't available",
        ))?;
        let index = number(profile.get("index"))
            .ok_or(AudioError::Invalid("PipeWire profile has no valid index"))?;
        if profiles.next().is_some() {
            return Err(AudioError::Invalid(
                "PipeWire repeats the pro-audio profile",
            ));
        }
        Ok(ProfileSelection {
            device: device.id,
            index,
        })
    }

    pub fn routing(&self, card: &M4Card) -> Result<RoutingPlan, AudioError> {
        let device = self.device(card)?;
        Ok(RoutingPlan {
            card: card.clone(),
            sink: self.endpoint(device.id, "Audio/Sink")?,
            source: self.endpoint(device.id, "Audio/Source")?,
            processing_rate: None,
        })
    }

    fn endpoint(&self, device: u32, class: &str) -> Result<Endpoint, AudioError> {
        let mut matches = self.objects.iter().filter(|object| {
            object.kind == "PipeWire:Interface:Node"
                && object.info.as_ref().is_some_and(|info| {
                    number(info.props.get("device.id")) == Some(device)
                        && info.props.get("media.class").and_then(Value::as_str) == Some(class)
                })
        });
        let object = matches.next().ok_or(AudioError::Invalid(
            "M4 source or sink is missing; select the pro-audio profile",
        ))?;
        if object.id == 0 || matches.next().is_some() {
            return Err(AudioError::Invalid(
                "M4 has ambiguous PipeWire source or sink nodes",
            ));
        }
        let info = object
            .info
            .as_ref()
            .ok_or(AudioError::Invalid("audio node has no properties"))?;
        if number(info.props.get("audio.channels")) != Some(4) {
            return Err(AudioError::Invalid(
                "M4 needs four channels; run desktop profile before installing routing",
            ));
        }
        let name = info
            .props
            .get("node.name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 1024)
            .ok_or(AudioError::Invalid("M4 audio node has no usable name"))?;
        let positions = if let Some(value) = info.props.get("audio.position") {
            positions(value)?
        } else {
            let value = ["Format", "EnumFormat"]
                .into_iter()
                .filter_map(|key| info.params.get(key).and_then(Value::as_array))
                .flatten()
                .find_map(|format| format.get("position"))
                .ok_or(AudioError::Invalid(
                    "M4 audio node didn't publish channel positions",
                ))?;
            positions(value)?
        };
        Ok(Endpoint {
            name: name.to_owned(),
            positions,
        })
    }

    pub fn virtual_node(
        &self,
        card: &M4Card,
        endpoint: DesktopEndpoint,
    ) -> Result<NodeId, AudioError> {
        let name = endpoint.node_name(card);
        let class = if endpoint.is_output() {
            "Audio/Sink"
        } else {
            "Audio/Source"
        };
        let mut nodes = self.objects.iter().filter(|object| {
            object.kind == "PipeWire:Interface:Node"
                && object.info.as_ref().is_some_and(|info| {
                    info.props.get("node.name").and_then(Value::as_str) == Some(name.as_str())
                        && info.props.get("media.class").and_then(Value::as_str) == Some(class)
                })
        });
        let node = nodes.next().ok_or(AudioError::Invalid(
            "M4 desktop endpoint is missing; install routing and restart the user audio session",
        ))?;
        if node.id == 0 || nodes.next().is_some() {
            return Err(AudioError::Invalid(
                "M4 desktop endpoint identity is ambiguous",
            ));
        }
        Ok(NodeId(node.id))
    }
}

fn positions(value: &Value) -> Result<[String; 4], AudioError> {
    let parts = match value {
        Value::Array(values) => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>(),
        Value::String(value) if value.contains('"') => {
            serde_json::from_str::<Vec<String>>(value).ok()
        }
        Value::String(value) => Some(
            value
                .trim()
                .trim_matches(['[', ']'])
                .split([',', ' ', '\t', '\n'])
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
        _ => None,
    }
    .ok_or(AudioError::Invalid("invalid PipeWire channel positions"))?;
    let positions: [String; 4] = parts
        .try_into()
        .map_err(|_| AudioError::Invalid("M4 channel positions must contain four entries"))?;
    let mut unique = HashSet::new();
    for position in &positions {
        let valid = matches!(
            position.as_str(),
            "FL" | "FR" | "FC" | "LFE" | "RL" | "RR" | "SL" | "SR" | "RC"
        ) || position.strip_prefix("AUX").is_some_and(|suffix| {
            suffix
                .parse::<u8>()
                .is_ok_and(|index| index <= 63 && suffix == index.to_string())
        });
        if !valid || !unique.insert(position) {
            return Err(AudioError::Invalid(
                "unsupported or repeated PipeWire channel position",
            ));
        }
    }
    Ok(positions)
}

struct Endpoint {
    name: String,
    positions: [String; 4],
}

pub struct RoutingPlan {
    card: M4Card,
    sink: Endpoint,
    source: Endpoint,
    processing_rate: Option<SampleRate>,
}

impl RoutingPlan {
    pub fn with_processing_rate(mut self, rate: SampleRate) -> Self {
        self.processing_rate = Some(rate);
        self
    }
    pub fn card(&self) -> &M4Card {
        &self.card
    }
    pub fn render(&self) -> Result<String, AudioError> {
        let mut modules = Vec::new();
        for endpoint in DesktopEndpoint::ALL {
            let name = endpoint.node_name(&self.card);
            let physical = if endpoint.is_output() {
                &self.sink
            } else {
                &self.source
            };
            let indexes: &[usize] = match endpoint {
                DesktopEndpoint::Main | DesktopEndpoint::Inputs12 => &[0, 1],
                DesktopEndpoint::Line | DesktopEndpoint::Inputs34 => &[2, 3],
                DesktopEndpoint::Input1 => &[0],
                DesktopEndpoint::Input2 => &[1],
            };
            let channels: Vec<&str> = indexes
                .iter()
                .map(|index| physical.positions[*index].as_str())
                .collect();
            let public_channels: &[&str] = if channels.len() == 1 {
                &["MONO"]
            } else {
                &["FL", "FR"]
            };
            let mut exposed = json!({
                "node.name": name,
                "node.description": format!("MOTU M4 ({}) {}", self.card.id(), endpoint.description()),
                "media.class": if endpoint.is_output() { "Audio/Sink" } else { "Audio/Source" },
                "audio.position": public_channels,
                "audio.channels": public_channels.len(),
                "node.virtual": true,
            });
            let mut target = json!({
                "node.name": format!("{}.{}", if endpoint.is_output() { "playback" } else { "capture" }, name),
                "target.object": physical.name,
                "audio.position": channels,
                "audio.channels": channels.len(),
                "stream.dont-remix": true,
                "node.passive": true,
                "node.dont-fallback": true,
                "node.dont-reconnect": true,
                "node.dont-move": true,
            });
            if let Some(rate) = self.processing_rate {
                exposed["node.force-rate"] = json!(rate.hz());
                exposed["resample.quality"] = json!(14);
                target["resample.quality"] = json!(14);
            }
            let (capture, playback) = if endpoint.is_output() {
                (exposed, target)
            } else {
                (target, exposed)
            };
            modules.push(json!({ "name": "libpipewire-module-loopback", "args": { "capture.props": capture, "playback.props": playback } }));
        }
        let config = serde_json::to_string_pretty(&json!({"context.modules": modules})).map_err(
            |error| AudioError::Desktop(format!("can't encode PipeWire configuration: {error}")),
        )?;
        Ok(format!("{MARKER}{config}\n"))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VolumePercent(u8);
impl VolumePercent {
    pub fn new(percent: u8) -> Result<Self, AudioError> {
        if percent > 100 {
            return Err(AudioError::Invalid(
                "volume must be between 0 and 100 percent",
            ));
        }
        Ok(Self(percent))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DesktopAction {
    Dump,
    Profile(ProfileSelection),
    Default(NodeId),
    GetVolume(NodeId),
    Volume {
        node: NodeId,
        percent: VolumePercent,
    },
    Mute {
        node: NodeId,
        muted: bool,
    },
    Restart,
}

impl DesktopAction {
    pub fn program(self) -> &'static str {
        match self {
            Self::Dump => "pw-dump",
            Self::Restart => "systemctl",
            _ => "wpctl",
        }
    }
    pub fn arguments(self) -> Vec<String> {
        match self {
            Self::Dump => vec!["--no-colors".into()],
            Self::Profile(selection) => vec![
                "set-profile".into(),
                selection.device.to_string(),
                selection.index.to_string(),
            ],
            Self::Default(node) => vec!["set-default".into(), node.0.to_string()],
            Self::GetVolume(node) => vec!["get-volume".into(), node.0.to_string()],
            Self::Volume { node, percent } => vec![
                "set-volume".into(),
                node.0.to_string(),
                format!("{}%", percent.0),
            ],
            Self::Mute { node, muted } => vec![
                "set-mute".into(),
                node.0.to_string(),
                u8::from(muted).to_string(),
            ],
            Self::Restart => [
                "--user",
                "restart",
                "pipewire.service",
                "pipewire-pulse.service",
                "wireplumber.service",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}
