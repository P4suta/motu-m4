use crate::{AudioError, DesktopAction, PipeWireGraph};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub trait DesktopHost {
    fn execute(&mut self, action: DesktopAction) -> Result<Vec<u8>, AudioError>;
}

#[derive(Default)]
pub struct SystemDesktop {
    runtime: Option<PathBuf>,
}

impl SystemDesktop {
    pub fn in_runtime(directory: &Path) -> Self {
        Self {
            runtime: Some(directory.to_owned()),
        }
    }
    pub fn snapshot(&mut self) -> Result<PipeWireGraph, AudioError> {
        PipeWireGraph::parse(&self.execute(DesktopAction::Dump)?)
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn read_limit(reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::other(
            "desktop tool output exceeded its size limit",
        ));
    }
    Ok(bytes)
}

impl DesktopHost for SystemDesktop {
    fn execute(&mut self, action: DesktopAction) -> Result<Vec<u8>, AudioError> {
        let mut command = Command::new(action.program());
        command
            .args(action.arguments())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(runtime) = &self.runtime {
            command.env("XDG_RUNTIME_DIR", runtime);
            command.env_remove("PIPEWIRE_RUNTIME_DIR");
            command.env_remove("PIPEWIRE_REMOTE");
        }
        let mut child = OwnedChild(command.spawn().map_err(|error| {
            AudioError::Desktop(format!("can't start {}: {error}", action.program()))
        })?);
        let stdout = child
            .0
            .stdout
            .take()
            .ok_or(AudioError::Invalid("desktop tool has no output pipe"))?;
        let stderr = child
            .0
            .stderr
            .take()
            .ok_or(AudioError::Invalid("desktop tool has no error pipe"))?;
        let stdout = thread::spawn(move || read_limit(stdout, 8 * 1024 * 1024));
        let stderr = thread::spawn(move || read_limit(stderr, 64 * 1024));
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            if started.elapsed() >= Duration::from_secs(10) {
                child.0.kill()?;
                child.0.wait()?;
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(AudioError::Desktop(format!(
                    "{} didn't finish within 10 seconds",
                    action.program()
                )));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let output = stdout
            .join()
            .map_err(|_| AudioError::Invalid("desktop output reader failed"))??;
        let errors = stderr
            .join()
            .map_err(|_| AudioError::Invalid("desktop error reader failed"))??;
        if !status.success() {
            return Err(AudioError::Desktop(format!(
                "{} failed ({status}): {}",
                action.program(),
                String::from_utf8_lossy(&errors).trim()
            )));
        }
        Ok(output)
    }
}
