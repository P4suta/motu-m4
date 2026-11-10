#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use xtask::{verify_updaters, VerifiedImage, MAC_ARCHIVE, WINDOWS_ARCHIVE};

type TaskResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(about = "Verify official firmware and run isolated control handlers")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Firmware {
        #[arg(long)]
        mac_archive: PathBuf,
        #[arg(long)]
        windows_archive: PathBuf,
        #[arg(
            long,
            alias = "export-descriptors",
            help = "Write USB protocol fixtures into a fresh directory"
        )]
        export_fixtures: Option<PathBuf>,
        #[arg(long, help = "Execute the verified M4 audio control handlers")]
        execute: bool,
        #[arg(
            long,
            conflicts_with = "export_fixtures",
            help = "Export verified M4 images into a fresh private test directory"
        )]
        export_corpus: Option<PathBuf>,
    },
}

fn read(path: &Path, limit: usize) -> TaskResult<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(format!("artifact exceeds {limit} bytes").into());
    }
    Ok(bytes)
}

fn export(directory: &Path, images: &[VerifiedImage]) -> TaskResult<()> {
    let ranges = &images
        .first()
        .ok_or("no verified firmware images")?
        .clock_ranges;
    fs::create_dir(directory)?;
    let result = (|| {
        for image in images.iter().filter(|image| image.name != "m6") {
            let path = directory.join(format!("m4-{}-firmware-descriptors.bin", image.name));
            let mut file = File::options().write(true).create_new(true).open(path)?;
            file.write_all(&image.descriptors)?;
        }
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(directory.join("m-series-clock-ranges.bin"))?;
        file.write_all(ranges)?;
        Ok(())
    })();
    if let Err(primary) = result {
        if let Err(cleanup) = fs::remove_dir_all(directory) {
            return Err(format!("{primary}; export cleanup failed: {cleanup}").into());
        }
        return Err(primary);
    }
    Ok(())
}

fn run(cli: Cli) -> TaskResult<()> {
    match cli.command {
        Command::Firmware {
            mac_archive,
            windows_archive,
            export_fixtures,
            execute,
            export_corpus,
        } => {
            let mac = read(&mac_archive, MAC_ARCHIVE.bytes)?;
            let windows = read(&windows_archive, WINDOWS_ARCHIVE.bytes)?;
            let images = verify_updaters(&mac, &windows)?;
            if execute {
                verify_execution(&images)?;
            }
            if let Some(directory) = export_corpus {
                export_images(&directory, &images)?;
            } else if let Some(directory) = export_fixtures {
                export(&directory, &images)?;
            } else {
                let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../crates/motu-m4-core/tests/fixtures");
                check_fixtures(&fixtures, &images)?;
            }
            for image in images {
                println!("{}: version {}, {} bytes, identical Windows image at {:#x}, {} descriptor bytes",
                    image.name, image.version, image.image_bytes, image.windows_offset, image.descriptors.len());
            }
            println!("Verified both archive digests, all three firmware images, and their clock range tables; physical USB writes: 0.");
        }
    }
    Ok(())
}

fn check_fixtures(directory: &Path, images: &[VerifiedImage]) -> TaskResult<()> {
    for image in images.iter().filter(|image| image.name != "m6") {
        let path = directory.join(format!("m4-{}-firmware-descriptors.bin", image.name));
        if read(&path, 65_553)? != image.descriptors {
            return Err(format!(
                "{} descriptor fixture differs from the official image",
                image.name
            )
            .into());
        }
    }
    let ranges = &images
        .first()
        .ok_or("no verified firmware images")?
        .clock_ranges;
    if read(&directory.join("m-series-clock-ranges.bin"), 74)? != *ranges {
        return Err("clock range fixture differs from the official images".into());
    }
    Ok(())
}

fn verify_execution(images: &[VerifiedImage]) -> TaskResult<()> {
    for image in images.iter().filter(|image| image.name != "m6") {
        verify_audio_execution(image)?;
    }
    Ok(())
}

fn export_images(directory: &Path, images: &[VerifiedImage]) -> TaskResult<()> {
    fs::create_dir(directory)?;
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        for image in images.iter().filter(|image| image.name != "m6") {
            let mut options = File::options();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(directory.join(format!("{}.image", image.name)))?;
            file.write_all(&image.firmware)?;
            file.sync_all()?;
        }
        Ok(())
    })();
    if let Err(primary) = result {
        if let Err(cleanup) = fs::remove_dir_all(directory) {
            return Err(format!("{primary}; corpus cleanup failed: {cleanup}").into());
        }
        return Err(primary);
    }
    Ok(())
}

fn verify_audio_execution(image: &VerifiedImage) -> TaskResult<()> {
    let mut control = motu_m4_firmware::FirmwareAudioControl::new(&image.firmware)?;
    let mut data = vec![0; 74];
    let count = control.control([0xa1, 2, 0, 1, 0, 2, 74, 0], &mut data)?;
    if count != 74 || data != image.clock_ranges {
        return Err(format!("firmware range response mismatch: {data:?}").into());
    }
    for rate in [44_100_u32, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let mut data = rate.to_le_bytes();
        if control.control([0x21, 1, 0, 1, 0, 2, 4, 0], &mut data)? != 4 {
            return Err("firmware SET_CUR size mismatch".into());
        }
        data.fill(0);
        if control.control([0xa1, 1, 0, 1, 0, 2, 4, 0], &mut data)? != 4
            || data != rate.to_le_bytes()
        {
            return Err(format!("firmware GET_CUR mismatch at {rate}: {data:?}").into());
        }
        let mut valid = [0];
        if control.control([0xa1, 1, 0, 2, 0, 2, 1, 0], &mut valid)? != 1 || valid != [1] {
            return Err("firmware validity mismatch".into());
        }
        let mut device = motu_m4::FirmwareM4::new(&image.firmware)?;
        let audio = vec![0x42; 19 * 16];
        let rate = motu_m4::SampleRate::new(rate).map_err(|error| format!("{error:?}"))?;
        let result = motu_m4::run_stream(
            &mut device,
            rate,
            motu_m4::StreamRequest::Duplex(&audio),
            |_| false,
        )?;
        if result.audio.len() != audio.len()
            || !device.simulated().is_attached()
            || device.simulated().clock() != 48_000
        {
            return Err("firmware-backed duplex or recovery mismatch".into());
        }
        println!(
            "Firmware-backed duplex: rate={}, frames={}, instructions={}",
            rate.hz(),
            result.report.frames,
            device.control_instructions()
        );
    }
    println!(
        "Control execution: {} instructions, packet lengths {:?}",
        control.instructions(),
        control.packets().iter().map(Vec::len).collect::<Vec<_>>()
    );
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{check_fixtures, export, export_images, read};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use xtask::VerifiedImage;

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "motu-firmware-export-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn image(name: &'static str) -> VerifiedImage {
        VerifiedImage {
            name,
            version: "test",
            image_bytes: 3,
            windows_offset: 0,
            descriptors: b"USB".to_vec(),
            clock_ranges: b"ranges".to_vec(),
            firmware: vec![0; 3],
        }
    }

    #[test]
    fn export_writes_only_m4_descriptors_and_never_overwrites_a_directory() {
        let scratch = Scratch::new();
        let destination = scratch.0.join("descriptors");
        let images = [image("atmel"), image("nxp"), image("m6")];
        export(&destination, &images).unwrap();
        check_fixtures(&destination, &images).unwrap();
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 3);
        for name in ["atmel", "nxp"] {
            assert_eq!(
                fs::read(destination.join(format!("m4-{name}-firmware-descriptors.bin"))).unwrap(),
                b"USB"
            );
        }
        assert!(export(&destination, &images).is_err());
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 3);
        assert_eq!(
            fs::read(destination.join("m-series-clock-ranges.bin")).unwrap(),
            b"ranges"
        );
        fs::write(destination.join("m4-nxp-firmware-descriptors.bin"), b"bad").unwrap();
        assert!(check_fixtures(&destination, &images).is_err());
        fs::write(destination.join("m4-nxp-firmware-descriptors.bin"), b"USB").unwrap();
        fs::write(destination.join("m-series-clock-ranges.bin"), b"bad").unwrap();
        assert!(check_fixtures(&destination, &images).is_err());
        let failed = scratch.0.join("failed");
        let error = export(&failed, &[image("atmel"), image("atmel")]).unwrap_err();
        assert!(error.to_string().contains("exist"));
        assert!(!failed.exists());
    }

    #[test]
    fn artifact_reads_accept_the_limit_and_refuse_one_extra_byte() {
        let scratch = Scratch::new();
        let path = scratch.0.join("archive.zip");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(read(&path, 3).unwrap(), b"abc");
        assert!(read(&path, 2).unwrap_err().to_string().contains("exceeds"));
        assert!(read(&path, 0).is_err());
        assert_eq!(read(&path, 4).unwrap(), b"abc");
    }

    #[test]
    fn corpus_export_is_private_excludes_m6_and_cleans_failed_exports() {
        let scratch = Scratch::new();
        let destination = scratch.0.join("corpus");
        let mut images = [image("atmel"), image("nxp"), image("m6")];
        images[0].firmware = b"atmel".to_vec();
        images[1].firmware = b"nxp".to_vec();
        export_images(&destination, &images).unwrap();
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 2);
        for image in &images[..2] {
            let path = destination.join(format!("{}.image", image.name));
            assert_eq!(fs::read(&path).unwrap(), image.firmware);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(export_images(&destination, &images).is_err());
        assert_eq!(fs::read(destination.join("atmel.image")).unwrap(), b"atmel");
        let failed = scratch.0.join("failed");
        assert!(export_images(&failed, &[image("nxp"), image("nxp")]).is_err());
        assert!(!failed.exists());
    }
}
