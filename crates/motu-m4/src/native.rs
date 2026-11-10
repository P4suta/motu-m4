use clap::{Args, Subcommand, ValueEnum};
use motu_m4_audio::{
    capture_wave, discover_cards, duplex_wave, install_routing, play_wave, remove_named_routing,
    select_card, AudioError, AudioInputs, AudioOutputs, CaptureRequest, DesktopAction,
    DesktopEndpoint, DesktopHost, M4Card, NativePcm, NativeReport, PipeWireGraph, SystemDesktop,
    VolumePercent, WaveInput,
};
use motu_m4_core::{native_recording_frames, SampleRate};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const DEFAULT_SAMPLE_RATE: &str = "192000";

#[derive(Args)]
pub struct CardOptions {
    #[arg(long, help = "Stable ALSA card ID from audio devices")]
    card: Option<String>,
    #[arg(long, hide = true, default_value = "/proc/asound")]
    asound_root: PathBuf,
    #[arg(long, hide = true, default_value = "/sys/class/sound")]
    sound_root: PathBuf,
}

impl CardOptions {
    fn cards(&self) -> Result<Vec<M4Card>, AudioError> {
        let cards = discover_cards(&self.asound_root, &self.sound_root)?;
        match &self.card {
            Some(id) => Ok(vec![select_card(&cards, Some(id))?.clone()]),
            None => Ok(cards),
        }
    }
    fn selected(&self) -> Result<M4Card, AudioError> {
        Ok(select_card(&self.cards()?, None)?.clone())
    }
}

#[derive(Args)]
pub struct DeviceOptions {
    #[command(flatten)]
    card: CardOptions,
    #[arg(
        long,
        conflicts_with = "card",
        help = "Use an explicit ALSA PCM name, such as default or the software-only null device"
    )]
    pcm: Option<String>,
}

impl DeviceOptions {
    fn pcm(&self) -> Result<String, AudioError> {
        match &self.pcm {
            Some(pcm) => Ok(pcm.clone()),
            None => Ok(self.card.selected()?.pcm_name()),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Inputs {
    #[value(name = "1")]
    Input1,
    #[value(name = "2")]
    Input2,
    #[value(name = "3")]
    Input3,
    #[value(name = "4")]
    Input4,
    #[value(name = "1-2")]
    Pair12,
    #[value(name = "3-4")]
    Pair34,
    All,
}
impl From<Inputs> for AudioInputs {
    fn from(value: Inputs) -> Self {
        match value {
            Inputs::Input1 => Self::Input1,
            Inputs::Input2 => Self::Input2,
            Inputs::Input3 => Self::Input3,
            Inputs::Input4 => Self::Input4,
            Inputs::Pair12 => Self::Pair12,
            Inputs::Pair34 => Self::Pair34,
            Inputs::All => Self::All,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Outputs {
    Main,
    Line,
    All,
}
impl From<Outputs> for AudioOutputs {
    fn from(value: Outputs) -> Self {
        match value {
            Outputs::Main => Self::Main,
            Outputs::Line => Self::Line,
            Outputs::All => Self::All,
        }
    }
}

#[derive(Subcommand)]
pub enum AudioCommand {
    #[command(about = "List M4 cards without opening their audio streams")]
    Devices {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Check kernel discovery and PipeWire without starting audio")]
    Doctor {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Record real audio into a 24-bit WAV file; Ctrl+C finalizes the recording")]
    Capture {
        #[command(flatten)]
        device: DeviceOptions,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = DEFAULT_SAMPLE_RATE, value_parser = super::parse_rate)]
        rate: SampleRate,
        #[arg(long, conflicts_with = "frames", value_parser = clap::value_parser!(u32).range(1..), help = "Recording duration in seconds; defaults to 60 when --frames is absent")]
        seconds: Option<u32>,
        #[arg(long, conflicts_with = "seconds", value_parser = clap::value_parser!(u64).range(1..), help = "Record an exact frame count instead of a duration")]
        frames: Option<u64>,
        #[arg(long, value_enum, default_value = "all")]
        inputs: Inputs,
    },
    #[command(about = "Play a WAV file through native Linux audio")]
    Play {
        #[command(flatten)]
        device: DeviceOptions,
        #[arg(long)]
        input: PathBuf,
        #[arg(
            long,
            value_enum,
            default_value = "all",
            help = "main: outputs 1-2; line: outputs 3-4; all: preserve four-channel files"
        )]
        outputs: Outputs,
    },
    #[command(about = "Play a WAV file while recording a separate WAV file")]
    Duplex {
        #[command(flatten)]
        device: DeviceOptions,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value = "all")]
        inputs: Inputs,
        #[arg(long, value_enum, default_value = "all")]
        outputs: Outputs,
    },
}

fn cancellation() -> super::AppResult<Arc<AtomicBool>> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    Ok(cancelled)
}

fn output_file(path: &Path) -> Result<BufWriter<File>, AudioError> {
    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(BufWriter::new(options.open(path)?))
}

fn unused_output(path: &Path) -> Result<(), AudioError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(AudioError::Invalid(
            "output already exists; choose a fresh recording path",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn report(pcm: &str, rate: SampleRate, report: NativeReport) {
    println!("transport=alsa, pcm={pcm}, rate={}, captured-frames={}, played-frames={}, recoveries={}, cancelled={}", rate.hz(), report.captured_frames, report.played_frames, report.recoveries, report.cancelled);
}

pub fn audio(command: AudioCommand) -> super::AppResult<()> {
    match command {
        AudioCommand::Devices { card, json } => {
            let cards = card.cards()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&cards)?);
            } else if cards.is_empty() {
                println!("No MOTU M4 cards found.");
            } else {
                for card in cards {
                    println!(
                        "{}: MOTU M4, USB 07fd:{:04x}, {}",
                        card.id(),
                        card.usb_product(),
                        card.pcm_name()
                    );
                }
            }
        }
        AudioCommand::Doctor { card, json } => {
            let cards = card.cards()?;
            let mut desktop = SystemDesktop::default();
            let pipewire = desktop.snapshot();
            let mut issues = Vec::new();
            if cards.is_empty() {
                issues.push("No MOTU M4 was found in kernel audio metadata.".to_owned());
            }
            if let Err(error) = &pipewire {
                issues.push(error.to_string());
            }
            if let Ok(graph) = &pipewire {
                for card in &cards {
                    if let Err(error) = graph.routing(card) {
                        issues.push(format!("{}: {error}", card.id()));
                    }
                }
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({ "cards": cards, "pipewire_connected": pipewire.is_ok(), "issues": issues })
                    )?
                );
            } else {
                println!(
                    "MOTU M4 cards: {}; PipeWire connected: {}",
                    cards.len(),
                    pipewire.is_ok()
                );
                for card in &cards {
                    println!("{}: {}", card.id(), card.pcm_name());
                }
                for issue in &issues {
                    eprintln!("{issue}");
                }
            }
            if !issues.is_empty() {
                return Err(AudioError::Invalid(
                    "audio setup needs attention; no audio streams were opened",
                )
                .into());
            }
        }
        AudioCommand::Capture {
            device,
            output,
            rate,
            seconds,
            frames,
            inputs,
        } => {
            let inputs: AudioInputs = inputs.into();
            let frames = match frames {
                Some(frames) => frames,
                None => native_recording_frames(rate, seconds.unwrap_or(60), inputs.channels())
                    .map_err(AudioError::from)?,
            };
            let request = CaptureRequest::new(rate, frames, inputs)?;
            unused_output(&output)?;
            let pcm = device.pcm()?;
            let cancelled = cancellation()?;
            let mut capture = NativePcm::capture(&pcm, rate)?;
            let writer = output_file(&output)?;
            eprintln!("Recording to {}; press Ctrl+C to finish.", output.display());
            let result = capture_wave(&mut capture, writer, request, || {
                cancelled.load(Ordering::Relaxed)
            });
            if result.is_err() {
                eprintln!("Recording retained at {}.", output.display());
            }
            report(&pcm, rate, result?);
        }
        AudioCommand::Play {
            device,
            input,
            outputs,
        } => {
            let input = WaveInput::new(BufReader::new(File::open(input)?))?;
            let outputs: AudioOutputs = outputs.into();
            outputs
                .route([0; 4], input.channels())
                .map_err(AudioError::from)?;
            let rate = input.rate();
            let pcm = device.pcm()?;
            let cancelled = cancellation()?;
            let mut playback = NativePcm::playback(&pcm, rate)?;
            let result = play_wave(&mut playback, input, outputs, || {
                cancelled.load(Ordering::Relaxed)
            })?;
            report(&pcm, rate, result);
        }
        AudioCommand::Duplex {
            device,
            input,
            output,
            inputs,
            outputs,
        } => {
            let input = WaveInput::new(BufReader::new(File::open(input)?))?;
            let outputs: AudioOutputs = outputs.into();
            outputs
                .route([0; 4], input.channels())
                .map_err(AudioError::from)?;
            let inputs: AudioInputs = inputs.into();
            CaptureRequest::new(input.rate(), input.frames(), inputs)?;
            unused_output(&output)?;
            let rate = input.rate();
            let pcm = device.pcm()?;
            let cancelled = cancellation()?;
            let mut capture = NativePcm::capture(&pcm, rate)?;
            let mut playback = NativePcm::playback(&pcm, rate)?;
            let writer = output_file(&output)?;
            eprintln!("Recording to {}; press Ctrl+C to finish.", output.display());
            let result = duplex_wave(
                &mut capture,
                &mut playback,
                input,
                writer,
                inputs,
                outputs,
                || cancelled.load(Ordering::Relaxed),
            );
            if result.is_err() {
                eprintln!("Recording retained at {}.", output.display());
            }
            report(&pcm, rate, result?);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Endpoint {
    Main,
    Line,
    #[value(name = "input-1")]
    Input1,
    #[value(name = "input-2")]
    Input2,
    #[value(name = "inputs-1-2")]
    Inputs12,
    #[value(name = "inputs-3-4")]
    Inputs34,
}
impl From<Endpoint> for DesktopEndpoint {
    fn from(value: Endpoint) -> Self {
        match value {
            Endpoint::Main => Self::Main,
            Endpoint::Line => Self::Line,
            Endpoint::Input1 => Self::Input1,
            Endpoint::Input2 => Self::Input2,
            Endpoint::Inputs12 => Self::Inputs12,
            Endpoint::Inputs34 => Self::Inputs34,
        }
    }
}

#[derive(Subcommand)]
pub enum DesktopCommand {
    #[command(about = "Show the M4 desktop endpoints without starting audio")]
    Status {
        #[command(flatten)]
        card: CardOptions,
    },
    #[command(about = "Select the M4 pro-audio profile to expose all four channels")]
    Profile {
        #[command(flatten)]
        card: CardOptions,
    },
    #[command(about = "Print desktop routing for the detected M4 channel layout")]
    Render {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long, help = "Read an offline pw-dump snapshot")]
        snapshot: Option<PathBuf>,
        #[arg(long, default_value = DEFAULT_SAMPLE_RATE, value_parser = super::parse_rate, help = "Graph sample rate while an M4 endpoint is active")]
        rate: SampleRate,
    },
    #[command(
        about = "Install M4 output pairs and microphone inputs into user PipeWire configuration"
    )]
    Install {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long, default_value = DEFAULT_SAMPLE_RATE, value_parser = super::parse_rate, help = "Graph sample rate while an M4 endpoint is active")]
        rate: SampleRate,
        #[arg(
            long,
            help = "Restart user audio services and interrupt active audio sessions"
        )]
        restart: bool,
    },
    #[command(about = "Remove only the routing file managed by motu-m4")]
    Remove {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(
            long,
            help = "Restart user audio services and interrupt active audio sessions"
        )]
        restart: bool,
    },
    #[command(about = "Use an M4 desktop endpoint as the default source or sink")]
    Default {
        #[command(flatten)]
        card: CardOptions,
        #[arg(value_enum)]
        endpoint: Endpoint,
    },
    #[command(about = "Set an M4 desktop endpoint volume without amplification")]
    Volume {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long, value_enum, default_value = "main")]
        endpoint: Endpoint,
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    #[command(about = "Mute or unmute an M4 desktop endpoint")]
    Mute {
        #[command(flatten)]
        card: CardOptions,
        #[arg(long, value_enum, default_value = "main")]
        endpoint: Endpoint,
        #[arg(long)]
        unmute: bool,
    },
}

fn config_directory(directory: Option<PathBuf>) -> Result<PathBuf, AudioError> {
    if let Some(directory) = directory {
        return Ok(directory);
    }
    let directory =
        if let Some(root) = std::env::var_os("XDG_CONFIG_HOME").filter(|root| !root.is_empty()) {
            PathBuf::from(root)
        } else {
            PathBuf::from(std::env::var_os("HOME").ok_or(AudioError::Invalid(
                "set XDG_CONFIG_HOME or pass --config-dir",
            ))?)
            .join(".config")
        };
    if !directory.is_absolute() {
        return Err(AudioError::Invalid(
            "user configuration directory must be absolute",
        ));
    }
    Ok(directory)
}

pub fn desktop(command: DesktopCommand) -> super::AppResult<()> {
    let mut host = SystemDesktop::default();
    match command {
        DesktopCommand::Status { card } => {
            let card = card.selected()?;
            let graph = host.snapshot()?;
            for endpoint in DesktopEndpoint::ALL {
                println!(
                    "{}: {}",
                    endpoint.description(),
                    if graph.virtual_node(&card, endpoint).is_ok() {
                        "available"
                    } else {
                        "not installed"
                    }
                );
            }
        }
        DesktopCommand::Profile { card } => {
            let card = card.selected()?;
            let graph = host.snapshot()?;
            host.execute(DesktopAction::Profile(graph.pro_audio_profile(&card)?))?;
            println!("Selected pro-audio for {}.", card.id());
        }
        DesktopCommand::Render {
            card,
            snapshot,
            rate,
        } => {
            let card = card.selected()?;
            let graph = if let Some(path) = snapshot {
                let mut bytes = Vec::new();
                File::open(path)?
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                PipeWireGraph::parse(&bytes)?
            } else {
                host.snapshot()?
            };
            print!(
                "{}",
                graph.routing(&card)?.with_processing_rate(rate).render()?
            );
        }
        DesktopCommand::Install {
            card,
            config_dir,
            rate,
            restart,
        } => {
            let card = card.selected()?;
            let routing = host.snapshot()?.routing(&card)?.with_processing_rate(rate);
            let path = install_routing(&config_directory(config_dir)?, &routing)?;
            println!("Installed {}.", path.display());
            if restart {
                host.execute(DesktopAction::Restart)?;
                println!("Restarted user audio services.");
            } else {
                println!("Sign out and back in, or restart the user audio services, to load the endpoints.");
            }
        }
        DesktopCommand::Remove {
            card,
            config_dir,
            restart,
        } => {
            let id = match &card.card {
                Some(id) => id.clone(),
                None => card.selected()?.id().to_owned(),
            };
            let removed = remove_named_routing(&config_directory(config_dir)?, &id)?;
            println!("M4 routing removed: {removed}.");
            if restart {
                host.execute(DesktopAction::Restart)?;
                println!("Restarted user audio services.");
            }
        }
        DesktopCommand::Default { card, endpoint } => {
            let card = card.selected()?;
            let endpoint = endpoint.into();
            let node = host.snapshot()?.virtual_node(&card, endpoint)?;
            host.execute(DesktopAction::Default(node))?;
            println!(
                "Default {}: {}.",
                if endpoint.is_output() {
                    "output"
                } else {
                    "input"
                },
                endpoint.description()
            );
        }
        DesktopCommand::Volume {
            card,
            endpoint,
            percent,
        } => {
            let card = card.selected()?;
            let endpoint = endpoint.into();
            let node = host.snapshot()?.virtual_node(&card, endpoint)?;
            host.execute(DesktopAction::Volume {
                node,
                percent: VolumePercent::new(percent)?,
            })?;
            println!("{} volume: {percent}%.", endpoint.description());
        }
        DesktopCommand::Mute {
            card,
            endpoint,
            unmute,
        } => {
            let card = card.selected()?;
            let endpoint = endpoint.into();
            let node = host.snapshot()?.virtual_node(&card, endpoint)?;
            host.execute(DesktopAction::Mute {
                node,
                muted: !unmute,
            })?;
            println!("{} muted: {}.", endpoint.description(), !unmute);
        }
    }
    Ok(())
}
