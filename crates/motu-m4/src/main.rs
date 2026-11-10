#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod native;

use clap::{Args, Parser, Subcommand, ValueEnum};
use motu_m4::{
    run_stream, DeviceProfile, FirmwareM4, FrameCount, SampleRate, Scenario, SimulatedM4,
    StreamOutput, StreamRequest,
};
use motu_m4_core::{FirmwareImage, Layout, MAX_FIRMWARE_BYTES, MAX_FRAMES};
use motu_m4_firmware::{Budget, Machine, MemoryLayout, Peripheral, Stop};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(name = "motu-m4", version, color = clap::ColorChoice::Never, about = "Use an M4 with Linux audio or run isolated USB protocol tests")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[cfg(target_os = "linux")]
    #[command(about = "Record and play real audio through ALSA")]
    Audio {
        #[command(subcommand)]
        command: native::AudioCommand,
    },
    #[cfg(target_os = "linux")]
    #[command(about = "Configure M4 channels for desktop applications through PipeWire")]
    Desktop {
        #[command(subcommand)]
        command: native::DesktopCommand,
    },
    #[command(about = "Execute Arm Thumb firmware in isolated emulated memory")]
    ExecuteFirmware {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_parser = parse_address)]
        ram_base: u32,
        #[arg(long, value_parser = parse_address)]
        ram_bytes: u32,
        #[arg(long, default_value_t = 100_000, value_parser = clap::value_parser!(u32).range(1..=10_000_000))]
        instructions: u32,
        #[arg(long, value_parser = parse_address)]
        entry: Option<u32>,
        #[arg(long, value_parser = parse_address, num_args = 4, requires = "entry")]
        args: Vec<u32>,
        #[arg(long, value_parser = parse_address)]
        read: Option<u32>,
        #[arg(long, default_value_t = 64, requires = "read", value_parser = clap::value_parser!(u32).range(1..=1024))]
        read_bytes: u32,
        #[arg(long, value_enum)]
        peripheral: Vec<PeripheralArg>,
    },
    #[command(about = "Inspect an observed firmware container without executing vendor code")]
    InspectFirmware {
        #[arg(long)]
        input: PathBuf,
    },
    #[command(about = "Read descriptors without taking device ownership")]
    Probe {
        #[arg(long, help = "Read a descriptor file instead of Linux sysfs")]
        descriptors: Option<PathBuf>,
        #[arg(
            long,
            default_value = "M4",
            requires = "descriptors",
            help = "Expected USB product string"
        )]
        product: String,
        #[arg(
            long,
            conflicts_with = "descriptors",
            help = "Search a sysfs metadata directory"
        )]
        sysfs_root: Option<PathBuf>,
    },
    #[command(about = "Stream with a simulated M4 without physical USB writes")]
    Simulate {
        #[command(subcommand)]
        command: Simulation,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum PeripheralArg {
    NxpUsbSram,
    SamBackupRegisters,
    SamUsbControl,
}

#[derive(Subcommand)]
enum Simulation {
    #[command(about = "Capture four-channel simulated audio into a raw file")]
    Capture {
        #[arg(long, default_value_t = 48_000, value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_FRAMES)))]
        frames: u32,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        options: SimulationOptions,
    },
    #[command(about = "Play raw audio into the simulated M4")]
    Play {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, help = "Save the packets received by the simulated device")]
        received: Option<PathBuf>,
        #[command(flatten)]
        options: SimulationOptions,
    },
    #[command(about = "Capture and play four-channel audio in one simulated session")]
    Duplex {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, help = "Save the packets received by the simulated device")]
        received: Option<PathBuf>,
        #[command(flatten)]
        options: SimulationOptions,
    },
}

#[derive(Args)]
struct SimulationOptions {
    #[arg(long, conflicts_with_all = ["profile", "scenario"], help = "Execute verified firmware controls with simulated audio")]
    firmware: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value = "recorded",
        help = "USB layout and timing profile"
    )]
    profile: ProfileArg,
    #[arg(long, default_value = "48000", value_parser = parse_rate, help = "Sample rate in Hz: 44100, 48000, 88200, 96000, 176400, or 192000")]
    rate: SampleRate,
    #[arg(long, value_enum, default_value = "steady")]
    scenario: ScenarioArg,
    #[arg(long, help = "Cancel after N completed USB capture intervals")]
    cancel_after_packets: Option<u32>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProfileArg {
    Recorded,
    AtmelTemplate,
    NxpTemplate,
    OriginalSynthetic,
}

impl From<ProfileArg> for DeviceProfile {
    fn from(value: ProfileArg) -> Self {
        match value {
            ProfileArg::Recorded => Self::Recorded,
            ProfileArg::AtmelTemplate => Self::AtmelTemplate,
            ProfileArg::NxpTemplate => Self::NxpTemplate,
            ProfileArg::OriginalSynthetic => Self::OriginalSynthetic,
        }
    }
}

fn parse_rate(input: &str) -> Result<SampleRate, String> {
    let value = input.parse::<u32>().map_err(|error| error.to_string())?;
    SampleRate::new(value).map_err(|_| {
        "expected an M4 sample rate: 44100, 48000, 88200, 96000, 176400, or 192000".to_owned()
    })
}

fn parse_address(input: &str) -> Result<u32, String> {
    if let Some(hex) = input.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).map_err(|error| error.to_string())
    } else {
        input
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ScenarioArg {
    Steady,
    Drift,
    NegativeDrift,
    Intermittent,
    Disconnect,
    Malformed,
    Stalled,
    Timeout,
    ShortControl,
    UnsupportedRate,
    MalformedRanges,
    InvalidClock,
    ClockMismatch,
}

impl From<ScenarioArg> for Scenario {
    fn from(value: ScenarioArg) -> Self {
        match value {
            ScenarioArg::Steady => Self::Steady,
            ScenarioArg::Drift => Self::Drift,
            ScenarioArg::NegativeDrift => Self::NegativeDrift,
            ScenarioArg::Intermittent => Self::Intermittent,
            ScenarioArg::Disconnect => Self::Disconnect,
            ScenarioArg::Malformed => Self::Malformed,
            ScenarioArg::Stalled => Self::Stalled,
            ScenarioArg::Timeout => Self::Timeout,
            ScenarioArg::ShortControl => Self::ShortControl,
            ScenarioArg::UnsupportedRate => Self::UnsupportedRate,
            ScenarioArg::MalformedRanges => Self::MalformedRanges,
            ScenarioArg::InvalidClock => Self::InvalidClock,
            ScenarioArg::ClockMismatch => Self::ClockMismatch,
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> AppResult<()> {
    match cli.command {
        #[cfg(target_os = "linux")]
        Command::Audio { command } => native::audio(command),
        #[cfg(target_os = "linux")]
        Command::Desktop { command } => native::desktop(command),
        Command::ExecuteFirmware {
            input,
            ram_base,
            ram_bytes,
            instructions,
            entry,
            args,
            read,
            read_bytes,
            peripheral,
        } => {
            let bytes = read_bounded(&input, MAX_FIRMWARE_BYTES)?;
            let image = FirmwareImage::parse(&bytes)
                .map_err(|error| format!("invalid firmware: {error:?}"))?;
            let layout = MemoryLayout::new(ram_base, ram_bytes)
                .map_err(|error| format!("invalid memory layout: {error:?}"))?;
            let mut machine = Machine::new(image, layout)
                .map_err(|error| format!("cannot load firmware: {error:?}"))?;
            for peripheral in peripheral {
                machine
                    .attach(match peripheral {
                        PeripheralArg::NxpUsbSram => Peripheral::NxpUsbSram,
                        PeripheralArg::SamBackupRegisters => Peripheral::SamBackupRegisters,
                        PeripheralArg::SamUsbControl => Peripheral::SamUsbControl,
                    })
                    .map_err(|error| format!("cannot attach peripheral: {error:?}"))?;
            }
            if let Some(entry) = entry {
                let args = if args.is_empty() {
                    [0; 4]
                } else {
                    args.try_into()
                        .map_err(|_| "expected four register arguments")?
                };
                machine
                    .prepare_call(entry, args)
                    .map_err(|error| format!("invalid function call: {error:?}"))?;
            }
            let result = machine
                .run(
                    Budget::new(instructions)
                        .map_err(|error| format!("invalid budget: {error:?}"))?,
                )
                .map_err(|error| format!("cannot execute firmware: {error:?}"))?;
            println!("stop={:?}, instructions={}, pc={:#010x}, r0={:#010x}, sp={:#010x}, physical-usb-writes=0", result.stop, result.instructions, result.registers[15], result.registers[0], result.registers[13]);
            print!("trace:");
            for address in result.trace {
                print!(" {address:#010x}");
            }
            println!();
            if let Some(address) = read {
                let data = machine
                    .read(address, read_bytes as usize)
                    .map_err(|error| format!("invalid memory read: {error:?}"))?;
                print!("memory[{address:#010x}]:");
                for byte in data {
                    print!(" {byte:02x}");
                }
                println!();
            }
            if matches!(result.stop, Stop::Fault(_) | Stop::ProcessorPanic) {
                return Err("firmware stopped at an unsupported operation".into());
            }
            Ok(())
        }
        Command::InspectFirmware { input } => inspect_firmware(&input),
        Command::Probe {
            descriptors,
            product,
            sysfs_root,
        } => probe(descriptors.as_deref(), &product, sysfs_root.as_deref()),
        Command::Simulate {
            command:
                Simulation::Capture {
                    frames,
                    output,
                    options,
                },
        } => {
            let frames = FrameCount::new(frames)
                .map_err(|error| format!("invalid frame count: {error:?}"))?;
            let (captured, device) = simulate(StreamRequest::Capture(frames), options)?;
            write_new(&output, &captured.audio)?;
            println!(
                "captured={} frames, channels=4, rate={}, bytes={}, transport={}, control-instructions={}",
                captured.report.frames,
                captured.report.rate.hz(),
                captured.audio.len(), device.transport_name(), device.instructions()
            );
            Ok(())
        }
        Command::Simulate {
            command:
                Simulation::Play {
                    input,
                    received,
                    options,
                },
        } => {
            let audio = read_bounded(&input, MAX_FRAMES as usize * 16)?;
            let (result, device) = simulate(StreamRequest::Playback(&audio), options)?;
            let report = result.report;
            if let Some(path) = received {
                write_new(&path, device.played())?;
            }
            println!(
                "played={} frames, packets={}, channels=4, rate={}, transport={}, control-instructions={}",
                report.frames,
                report.packet_lengths.len(),
                report.rate.hz(),
                device.transport_name(), device.instructions(),
            );
            Ok(())
        }
        Command::Simulate {
            command:
                Simulation::Duplex {
                    input,
                    output,
                    received,
                    options,
                },
        } => {
            let audio = read_bounded(&input, MAX_FRAMES as usize * 16)?;
            let (result, device) = simulate(StreamRequest::Duplex(&audio), options)?;
            let mut outputs = vec![(output.as_path(), result.audio.as_slice())];
            if let Some(path) = received.as_deref() {
                outputs.push((path, device.played()));
            }
            write_outputs(&outputs)?;
            println!(
                "duplex={} frames, packets={}, channels=4, rate={}, bytes={}, transport={}, control-instructions={}",
                result.report.frames,
                result.report.packet_lengths.len(),
                result.report.rate.hz(),
                result.audio.len(), device.transport_name(), device.instructions()
            );
            Ok(())
        }
    }
}

fn simulate(
    request: StreamRequest<'_>,
    options: SimulationOptions,
) -> AppResult<(StreamOutput, SimulationDevice)> {
    let mut device = if let Some(path) = options.firmware.as_deref() {
        SimulationDevice::Firmware(Box::new(FirmwareM4::new(&read_bounded(
            path,
            MAX_FIRMWARE_BYTES,
        )?)?))
    } else {
        SimulationDevice::Simulated(SimulatedM4::with_profile(
            options.profile.into(),
            options.scenario.into(),
        ))
    };
    let mut cancel = |packets| {
        options
            .cancel_after_packets
            .is_some_and(|limit| packets >= limit)
    };
    let result = match &mut device {
        SimulationDevice::Simulated(device) => {
            run_stream(device, options.rate, request, &mut cancel)
        }
        SimulationDevice::Firmware(device) => {
            run_stream(device.as_mut(), options.rate, request, &mut cancel)
        }
    }
    .map_err(|error| {
        format!(
            "session failed: {error}; attached={}, clock={}",
            device.simulated().is_attached(),
            device.simulated().clock()
        )
    })?;
    Ok((result, device))
}

enum SimulationDevice {
    Simulated(SimulatedM4),
    Firmware(Box<FirmwareM4>),
}

impl SimulationDevice {
    fn simulated(&self) -> &SimulatedM4 {
        match self {
            Self::Simulated(device) => device,
            Self::Firmware(device) => device.simulated(),
        }
    }
    fn played(&self) -> &[u8] {
        self.simulated().played()
    }
    fn instructions(&self) -> u64 {
        match self {
            Self::Simulated(_) => 0,
            Self::Firmware(device) => device.control_instructions(),
        }
    }
    fn transport_name(&self) -> &'static str {
        match self {
            Self::Simulated(_) => "simulated",
            Self::Firmware(_) => "firmware-control+simulated-audio",
        }
    }
}

fn inspect_firmware(path: &Path) -> AppResult<()> {
    let bytes = read_bounded(path, MAX_FIRMWARE_BYTES)?;
    let image = FirmwareImage::parse(&bytes)
        .map_err(|error| format!("invalid observed firmware container: {error:?}"))?;
    println!(
        "version={}, payload={}, initial-stack={:#010x}, reset-handler={:#010x}, access=read-only",
        image.version(),
        image.payload().len(),
        image.initial_stack_pointer(),
        image.reset_handler()
    );
    for candidate in image.usb_devices() {
        let bytes = candidate.bytes();
        let product = u16::from_le_bytes([bytes[10], bytes[11]]);
        let release = u16::from_le_bytes([bytes[12], bytes[13]]);
        println!("usb-device-candidate: offset={:#x}, vendor=0x07fd, product={product:#06x}, bcdDevice={release:#06x}", candidate.offset());
    }
    for candidate in image.usb_configurations() {
        println!(
            "usb-configuration-candidate: offset={:#x}, bytes={}",
            candidate.offset(),
            candidate.bytes().len()
        );
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> AppResult<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(u64::try_from(limit)? + 1)
        .read_to_end(&mut data)?;
    if data.len() > limit {
        return Err(format!("input exceeds {limit} bytes").into());
    }
    Ok(data)
}

fn write_new(path: &Path, data: &[u8]) -> AppResult<()> {
    write_outputs(&[(path, data)])
}

fn write_outputs(outputs: &[(&Path, &[u8])]) -> AppResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut files = Vec::new();
    let result: std::io::Result<()> = (|| {
        for &(path, _) in outputs {
            files.push((path, options.open(path)?));
        }
        for ((_, file), (_, data)) in files.iter_mut().zip(outputs) {
            file.write_all(data)?;
            file.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut message = format!("output failed: {error}");
        for (path, file) in files {
            drop(file);
            if let Err(cleanup) = fs::remove_file(path) {
                message.push_str(&format!(
                    "; remove incomplete output {} failed: {cleanup}",
                    path.display()
                ));
            }
        }
        return Err(message.into());
    }
    Ok(())
}

fn probe(descriptors: Option<&Path>, product: &str, sysfs_root: Option<&Path>) -> AppResult<()> {
    if let Some(path) = descriptors {
        let bytes = read_bounded(path, 65_553)?;
        let layout = Layout::parse(&bytes, product)
            .map_err(|error| format!("unsupported descriptors: {error:?}"))?;
        print_layout(layout);
        return Ok(());
    }
    let root = sysfs_root.unwrap_or_else(|| Path::new("/sys/bus/usb/devices"));
    let mut paths: Vec<PathBuf> = fs::read_dir(root)?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    let mut found = false;
    for path in paths {
        let vendor = match fs::read_to_string(path.join("idVendor")) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if vendor.trim() != "07fd" {
            continue;
        }
        let name = fs::read_to_string(path.join("product"))?;
        if name.trim_end_matches(['\n', '\r']) != "M4" {
            continue;
        }
        let bytes = read_bounded(&path.join("descriptors"), 65_553)?;
        let layout = Layout::parse(&bytes, "M4")
            .map_err(|error| format!("unsupported M4 descriptors: {error:?}"))?;
        print_layout(layout);
        found = true;
    }
    if !found {
        return Err("no supported M4 found in Linux sysfs".into());
    }
    Ok(())
}

fn print_layout(layout: Layout) {
    println!("M4: channels=4, sample_slot=32 bits, valid_bits=24, playback=0x{:02x}, capture=0x{:02x}, clock={}, access=read-only", layout.playback().address(), layout.capture().address(), layout.clock_id());
}
