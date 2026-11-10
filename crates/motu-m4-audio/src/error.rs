use crate::PcmFault;
use motu_m4_core::AudioFormatError;
use std::fmt;

#[derive(Debug)]
pub enum AudioError {
    Io(std::io::Error),
    Wave(hound::Error),
    Format(AudioFormatError),
    Invalid(&'static str),
    Pcm {
        operation: &'static str,
        fault: PcmFault,
    },
    Desktop(String),
    Cleanup {
        primary: Option<Box<AudioError>>,
        errors: Vec<AudioError>,
    },
    #[cfg(target_os = "linux")]
    Alsa(alsa::Error),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "audio file or device discovery: {error}"),
            Self::Wave(error) => write!(f, "WAV file: {error}"),
            Self::Format(error) => write!(f, "unsupported audio format: {error:?}"),
            Self::Invalid(message) => f.write_str(message),
            Self::Pcm { operation, fault } => write!(f, "{operation}: {fault}"),
            Self::Desktop(message) => f.write_str(message),
            Self::Cleanup { primary, errors } => {
                if let Some(primary) = primary {
                    write!(f, "{primary}; ")?;
                }
                write!(f, "cleanup failed")?;
                for error in errors {
                    write!(f, "; {error}")?;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Self::Alsa(error) => write!(f, "ALSA setup: {error}"),
        }
    }
}

impl std::error::Error for AudioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Wave(error) => Some(error),
            Self::Cleanup {
                primary: Some(error),
                ..
            } => Some(error.as_ref()),
            #[cfg(target_os = "linux")]
            Self::Alsa(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AudioError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<hound::Error> for AudioError {
    fn from(error: hound::Error) -> Self {
        Self::Wave(error)
    }
}
impl From<AudioFormatError> for AudioError {
    fn from(error: AudioFormatError) -> Self {
        Self::Format(error)
    }
}
#[cfg(target_os = "linux")]
impl From<alsa::Error> for AudioError {
    fn from(error: alsa::Error) -> Self {
        Self::Alsa(error)
    }
}
