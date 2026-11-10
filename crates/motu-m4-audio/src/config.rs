use crate::desktop::MARKER;
use crate::{AudioError, M4Card, RoutingPlan};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn destination(config: &Path, card: &M4Card) -> PathBuf {
    destination_named(config, card.id())
}

fn destination_named(config: &Path, id: &str) -> PathBuf {
    config
        .join("pipewire/pipewire.conf.d")
        .join(format!("60-motu-m4-{id}.conf"))
}

fn existing(path: &Path) -> Result<Option<String>, AudioError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(AudioError::Invalid(
                "routing configuration must be a regular file",
            ))
        }
        Ok(_) => {}
    }
    let mut contents = String::new();
    File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_string(&mut contents)?;
    if contents.len() > 128 * 1024 {
        return Err(AudioError::Invalid(
            "existing routing configuration is too large",
        ));
    }
    Ok(Some(contents))
}

pub fn install_routing(config: &Path, routing: &RoutingPlan) -> Result<PathBuf, AudioError> {
    let path = destination(config, routing.card());
    let contents = routing.render()?;
    if let Some(previous) = existing(&path)? {
        return if previous == contents {
            Ok(path)
        } else {
            Err(AudioError::Invalid("routing configuration already exists with different contents; preserve or remove it before installing"))
        };
    }
    let directory = path.parent().ok_or(AudioError::Invalid(
        "routing configuration has no parent directory",
    ))?;
    fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(
        ".motu-m4-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::hard_link(&temporary, &path)?;
        Ok(path)
    })();
    let cleanup = fs::remove_file(temporary);
    match (result, cleanup) {
        (result, Ok(())) => result,
        (result, Err(error)) => Err(AudioError::Cleanup {
            primary: result.err().map(Box::new),
            errors: vec![AudioError::Io(error)],
        }),
    }
}

pub fn remove_routing(config: &Path, card: &M4Card) -> Result<bool, AudioError> {
    remove_named_routing(config, card.id())
}

pub fn remove_named_routing(config: &Path, id: &str) -> Result<bool, AudioError> {
    if id.is_empty()
        || id.len() > 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AudioError::Invalid("invalid M4 card ID"));
    }
    let path = destination_named(config, id);
    let Some(contents) = existing(&path)? else {
        return Ok(false);
    };
    if !contents.starts_with(MARKER) {
        return Err(AudioError::Invalid(
            "routing configuration isn't owned by motu-m4",
        ));
    }
    fs::remove_file(path)?;
    Ok(true)
}
