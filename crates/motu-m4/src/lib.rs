#![forbid(unsafe_code)]

pub use motu_m4_core::{FrameCount, SampleRate};

mod engine;
mod firmware_device;
mod simulated;

pub use engine::StreamOutput as Capture;
pub use engine::{
    capture, capture_with_cancel, play, run_stream, AudioTransport, Failure, FailureCause,
    Operation, RunError, SessionReport, StreamOutput, StreamRequest, TransportError,
};
pub use firmware_device::FirmwareM4;
pub use simulated::{DeviceProfile, Scenario, SimulatedM4};
