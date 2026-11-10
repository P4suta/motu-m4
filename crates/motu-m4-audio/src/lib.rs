#![forbid(unsafe_code)]

pub use motu_m4_core::{AudioChannels, AudioInputs, AudioOutputs, SampleRate};

mod config;
mod desktop;
mod discovery;
mod error;
#[cfg(target_os = "linux")]
mod native;
mod process;
mod transfer;
mod wave;

pub use config::{install_routing, remove_named_routing, remove_routing};
pub use desktop::{
    DesktopAction, DesktopEndpoint, NodeId, PipeWireGraph, ProfileSelection, RoutingPlan,
    VolumePercent,
};
pub use discovery::{discover_cards, select_card, M4Card};
pub use error::AudioError;
#[cfg(target_os = "linux")]
pub use native::NativePcm;
pub use process::{DesktopHost, SystemDesktop};
pub use transfer::{
    capture_wave, duplex_wave, play_wave, CaptureRequest, NativeReport, PcmFault, PcmStream,
};
pub use wave::WaveInput;
