#![forbid(unsafe_code)]

use motu_m4_core::{FirmwareImage, Layout};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[derive(Clone, Copy, Debug)]
pub struct ArchiveIdentity<'a> {
    pub bytes: usize,
    pub sha256: &'a str,
}

pub const MAC_ARCHIVE: ArchiveIdentity<'static> = ArchiveIdentity {
    bytes: 287_586,
    sha256: "24a0f2413760ec937b6bac882b40265dc09de1e63ac238d64f37e9206600e33c",
};
pub const WINDOWS_ARCHIVE: ArchiveIdentity<'static> = ArchiveIdentity {
    bytes: 296_862,
    sha256: "5a3fd4a8579de022ef4eb533ad073f4197d17103440efdee88099b1c66805c6e",
};
const APP: &str = "MOTU M-Series Updater (February 2026).app/Contents/Resources";
const EXE: &str = "MOTU M-Series Updater (February 2026).exe";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorpusError {
    Size,
    Digest,
    Zip,
    Entry,
    Embedded,
    Header,
    Version,
    Descriptor,
    Clock,
}

impl fmt::Display for CorpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Size => "unexpected artifact size",
            Self::Digest => "SHA-256 mismatch",
            Self::Zip => "invalid ZIP archive",
            Self::Entry => "missing, oversized, or incomplete ZIP entry",
            Self::Embedded => {
                "expected exactly one identical firmware image in the Windows updater"
            }
            Self::Header => "invalid observed firmware container",
            Self::Version => "firmware version mismatch",
            Self::Descriptor => "firmware USB descriptor mismatch",
            Self::Clock => "firmware clock range table mismatch",
        })
    }
}

impl std::error::Error for CorpusError {}

pub fn check_archive(bytes: &[u8], identity: ArchiveIdentity<'_>) -> Result<(), CorpusError> {
    if bytes.len() != identity.bytes {
        return Err(CorpusError::Size);
    }
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != identity.sha256 {
        return Err(CorpusError::Digest);
    }
    Ok(())
}

pub fn check_embedded(executable: &[u8], image: &[u8]) -> Result<usize, CorpusError> {
    if image.is_empty() {
        return Err(CorpusError::Embedded);
    }
    let mut matches = executable
        .windows(image.len())
        .enumerate()
        .filter(|(_, candidate)| *candidate == image);
    let (offset, _) = matches.next().ok_or(CorpusError::Embedded)?;
    if matches.next().is_some() {
        return Err(CorpusError::Embedded);
    }
    Ok(offset)
}

fn entry(archive: &[u8], name: &str, expected_size: usize) -> Result<Vec<u8>, CorpusError> {
    let mut archive = ZipArchive::new(Cursor::new(archive)).map_err(|_| CorpusError::Zip)?;
    if archive.len() > 128 {
        return Err(CorpusError::Zip);
    }
    let file = archive.by_name(name).map_err(|_| CorpusError::Entry)?;
    if file.size() != expected_size as u64 {
        return Err(CorpusError::Entry);
    }
    let mut bytes = Vec::new();
    file.take(expected_size as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CorpusError::Entry)?;
    if bytes.len() != expected_size {
        return Err(CorpusError::Entry);
    }
    Ok(bytes)
}

struct ImageIdentity<'a> {
    name: &'static str,
    version: &'static str,
    bytes: usize,
    sha256: &'a str,
    device_offset: usize,
    configuration_offset: usize,
    capture: u8,
    bcd_device: u16,
    clock_offset: usize,
}

const IMAGES: [ImageIdentity<'static>; 3] = [
    ImageIdentity {
        name: "atmel",
        version: "1.2.6+943",
        bytes: 353_356,
        sha256: "25d7faf930ca291aed83c0dffad917ef672050f6aca6711f666b37996e1ec86b",
        device_offset: 91_628,
        configuration_offset: 91_240,
        capture: 0x83,
        bcd_device: 0x0206,
        clock_offset: 82_868,
    },
    ImageIdentity {
        name: "nxp",
        version: "2.2.9+641",
        bytes: 168_596,
        sha256: "80a7e479f15c5f2df8b5d66e9f355d93841d38f92ddd7e9e6e05fcc66e0f5748",
        device_offset: 83_508,
        configuration_offset: 83_120,
        capture: 0x82,
        bcd_device: 0x0209,
        clock_offset: 67_580,
    },
    ImageIdentity {
        name: "m6",
        version: "1.2.3+2026",
        bytes: 270_060,
        sha256: "323e4d4e4f37a7b2b087e7e1661041bba597a86e3fbe6060cea02937e3ac7051",
        device_offset: 166_088,
        configuration_offset: 165_700,
        capture: 0x83,
        bcd_device: 0x0203,
        clock_offset: 157_920,
    },
];

#[derive(Debug)]
pub struct VerifiedImage {
    pub name: &'static str,
    pub version: &'static str,
    pub image_bytes: usize,
    pub windows_offset: usize,
    pub descriptors: Vec<u8>,
    pub clock_ranges: Vec<u8>,
    pub firmware: Vec<u8>,
}

fn descriptors(bytes: &[u8], identity: &ImageIdentity<'_>) -> Result<Vec<u8>, CorpusError> {
    let firmware = FirmwareImage::parse(bytes).map_err(|_| CorpusError::Header)?;
    if firmware.version() != identity.version {
        return Err(CorpusError::Version);
    }
    let mut devices = firmware.usb_devices();
    let device = devices.next().ok_or(CorpusError::Descriptor)?;
    let mut configurations = firmware.usb_configurations();
    let configuration = configurations.next().ok_or(CorpusError::Descriptor)?;
    if devices.next().is_some()
        || configurations.next().is_some()
        || device.offset() != identity.device_offset
        || configuration.offset() != identity.configuration_offset
        || device.bytes().get(12..14) != Some(identity.bcd_device.to_le_bytes().as_slice())
    {
        return Err(CorpusError::Descriptor);
    }
    let bytes = [device.bytes(), configuration.bytes()].concat();
    let layout = Layout::parse(&bytes, "M4").map_err(|_| CorpusError::Descriptor)?;
    if layout.capture().address() != identity.capture
        || layout.playback().address() != 2
        || layout.capture().max_packet() != 405
        || layout.playback().max_packet() != 400
        || layout.clock_id() != 2
    {
        return Err(CorpusError::Descriptor);
    }
    Ok(bytes)
}

fn clock_ranges(bytes: &[u8], offset: usize) -> Result<Vec<u8>, CorpusError> {
    let end = offset.checked_add(74).ok_or(CorpusError::Clock)?;
    let table = bytes.get(offset..end).ok_or(CorpusError::Clock)?;
    if table[..2] != [6, 0] {
        return Err(CorpusError::Clock);
    }
    for (entry, hz) in table[2..]
        .as_chunks::<12>()
        .0
        .iter()
        .zip([44_100_u32, 48_000, 88_200, 96_000, 176_400, 192_000])
    {
        if entry[..4] != hz.to_le_bytes()
            || entry[4..8] != hz.to_le_bytes()
            || entry[8..12] != [0; 4]
        {
            return Err(CorpusError::Clock);
        }
    }
    Ok(table.to_vec())
}

fn verify_archives(
    mac: &[u8],
    windows: &[u8],
    mac_identity: ArchiveIdentity<'_>,
    windows_identity: ArchiveIdentity<'_>,
    executable_size: usize,
    identities: &[ImageIdentity<'_>],
) -> Result<Vec<VerifiedImage>, CorpusError> {
    check_archive(mac, mac_identity)?;
    check_archive(windows, windows_identity)?;
    let executable = entry(windows, EXE, executable_size)?;
    if identities.is_empty() {
        return Err(CorpusError::Clock);
    }
    let mut images = Vec::new();
    for identity in identities {
        let bytes = entry(
            mac,
            &format!("{APP}/{}.image", identity.name),
            identity.bytes,
        )?;
        check_archive(
            &bytes,
            ArchiveIdentity {
                bytes: identity.bytes,
                sha256: identity.sha256,
            },
        )?;
        images.push(VerifiedImage {
            name: identity.name,
            version: identity.version,
            image_bytes: bytes.len(),
            windows_offset: check_embedded(&executable, &bytes)?,
            descriptors: descriptors(&bytes, identity)?,
            clock_ranges: clock_ranges(&bytes, identity.clock_offset)?,
            firmware: bytes,
        });
    }
    Ok(images)
}

pub fn verify_updaters(mac: &[u8], windows: &[u8]) -> Result<Vec<VerifiedImage>, CorpusError> {
    verify_archives(
        mac,
        windows,
        MAC_ARCHIVE,
        WINDOWS_ARCHIVE,
        1_102_800,
        &IMAGES,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        clock_ranges, descriptors, entry, verify_archives, verify_updaters, ArchiveIdentity,
        CorpusError, ImageIdentity, APP, EXE,
    };
    use sha2::{Digest, Sha256};
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    #[test]
    fn zip_entry_reads_exact_names_and_refuses_wrong_sizes_or_corrupt_data() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            writer
                .start_file(
                    "firmware.image",
                    SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(b"firmware").unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            assert_eq!(entry(&bytes, "firmware.image", 8).unwrap(), b"firmware");
            assert_eq!(entry(&bytes, "firmware.image", 7), Err(CorpusError::Entry));
            assert_eq!(entry(&bytes, "firmware.image", 9), Err(CorpusError::Entry));
            assert_eq!(entry(&bytes, "other.image", 8), Err(CorpusError::Entry));
            assert_eq!(
                entry(&bytes[..30], "firmware.image", 8),
                Err(CorpusError::Zip)
            );
        }
    }

    #[test]
    fn zip_entry_limit_accepts_128_entries_and_refuses_129() {
        for count in [128, 129] {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            for index in 0..count {
                writer
                    .start_file(format!("image{index}"), SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(b"firmware").unwrap();
            }
            let archive = writer.finish().unwrap().into_inner();
            if count == 128 {
                assert_eq!(entry(&archive, "image0", 8).unwrap(), b"firmware");
            } else {
                assert_eq!(entry(&archive, "image0", 8), Err(CorpusError::Zip));
            }
        }
    }

    #[test]
    fn zip_reader_refuses_more_output_than_the_declared_uncompressed_size() {
        let mut bytes = archive("firmware.image", b"firmware");
        let local = bytes
            .windows(4)
            .position(|window| window == b"PK\x03\x04")
            .unwrap();
        let central = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        bytes[local + 22..local + 26].copy_from_slice(&7_u32.to_le_bytes());
        bytes[central + 24..central + 28].copy_from_slice(&7_u32.to_le_bytes());
        assert_eq!(entry(&bytes, "firmware.image", 7), Err(CorpusError::Entry));
    }

    fn image() -> Vec<u8> {
        let mut bytes = vec![0; 72];
        bytes[4..13].copy_from_slice(b"2.2.9+641");
        bytes.extend_from_slice(include_bytes!(
            "../../crates/motu-m4-core/tests/fixtures/m4-nxp-firmware-descriptors.bin"
        ));
        let length = u32::try_from(bytes.len() - 64).unwrap();
        bytes[..4].copy_from_slice(&length.to_le_bytes());
        bytes
    }

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn archive(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn archive_pipeline_checks_both_digests_extracts_the_full_image_and_validates_protocol_data() {
        let mut firmware = image();
        let mut table = vec![6, 0];
        for frequency in [44_100_u32, 48_000, 88_200, 96_000, 176_400, 192_000] {
            table.extend(frequency.to_le_bytes());
            table.extend(frequency.to_le_bytes());
            table.extend([0; 4]);
        }
        firmware.extend(&table);
        let length = u32::try_from(firmware.len() - 64).unwrap();
        firmware[..4].copy_from_slice(&length.to_le_bytes());
        let executable = [&b"MZ"[..], &firmware, b"TAIL"].concat();
        let mac = archive(&format!("{APP}/nxp.image"), &firmware);
        let windows = archive(EXE, &executable);
        let mac_hash = digest(&mac);
        let windows_hash = digest(&windows);
        let firmware_hash = digest(&firmware);
        let mac_identity = ArchiveIdentity {
            bytes: mac.len(),
            sha256: &mac_hash,
        };
        let windows_identity = ArchiveIdentity {
            bytes: windows.len(),
            sha256: &windows_hash,
        };
        let image_identity = ImageIdentity {
            bytes: firmware.len(),
            sha256: &firmware_hash,
            ..identity()
        };
        let identities = [image_identity];
        let output = verify_archives(
            &mac,
            &windows,
            mac_identity,
            windows_identity,
            executable.len(),
            &identities,
        )
        .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].name, "nxp");
        assert_eq!(output[0].version, "2.2.9+641");
        assert_eq!(output[0].image_bytes, firmware.len());
        assert_eq!(output[0].windows_offset, 2);
        assert_eq!(output[0].descriptors, &firmware[72..477]);
        assert_eq!(output[0].clock_ranges, table);
        let mut changed_mac = mac.clone();
        changed_mac[0] ^= 1;
        assert_eq!(
            verify_archives(
                &changed_mac,
                &windows,
                mac_identity,
                windows_identity,
                executable.len(),
                &identities
            )
            .unwrap_err(),
            CorpusError::Digest
        );
        let mut changed_windows = windows.clone();
        changed_windows[0] ^= 1;
        assert_eq!(
            verify_archives(
                &mac,
                &changed_windows,
                mac_identity,
                windows_identity,
                executable.len(),
                &identities
            )
            .unwrap_err(),
            CorpusError::Digest
        );
        assert_eq!(
            verify_archives(
                &mac,
                &windows,
                mac_identity,
                windows_identity,
                executable.len() - 1,
                &identities
            )
            .unwrap_err(),
            CorpusError::Entry
        );
        assert_eq!(
            verify_archives(
                &mac,
                &windows,
                mac_identity,
                windows_identity,
                executable.len(),
                &[]
            )
            .unwrap_err(),
            CorpusError::Clock
        );
    }

    fn identity() -> ImageIdentity<'static> {
        ImageIdentity {
            name: "nxp",
            version: "2.2.9+641",
            bytes: 477,
            sha256: "unused",
            device_offset: 72,
            configuration_offset: 90,
            capture: 0x82,
            bcd_device: 0x0209,
            clock_offset: 477,
        }
    }

    #[test]
    fn firmware_clock_table_requires_all_six_discrete_ranges_and_complete_bytes() {
        let mut expected = vec![6, 0];
        for frequency in [44_100_u32, 48_000, 88_200, 96_000, 176_400, 192_000] {
            expected.extend(frequency.to_le_bytes());
            expected.extend(frequency.to_le_bytes());
            expected.extend([0; 4]);
        }
        let mut image = vec![0; 16];
        image.extend(&expected);
        assert_eq!(clock_ranges(&image, 16).unwrap(), expected);
        for offset in [0, 15, 17, image.len(), usize::MAX] {
            assert!(clock_ranges(&image, offset).is_err());
        }
        assert!(clock_ranges(&image[..image.len() - 1], 16).is_err());
        for offset in [16, 18, 22, 26] {
            let mut changed = image.clone();
            changed[offset] ^= 1;
            assert!(clock_ranges(&changed, 16).is_err());
        }
    }

    #[test]
    fn descriptor_extraction_refuses_mismatched_versions_offsets_endpoints_and_duplicates() {
        let bytes = image();
        let expected = include_bytes!(
            "../../crates/motu-m4-core/tests/fixtures/m4-nxp-firmware-descriptors.bin"
        );
        assert_eq!(descriptors(&bytes, &identity()).unwrap(), expected);
        for changed in [
            ImageIdentity {
                device_offset: 73,
                ..identity()
            },
            ImageIdentity {
                configuration_offset: 91,
                ..identity()
            },
            ImageIdentity {
                capture: 0x83,
                ..identity()
            },
            ImageIdentity {
                bcd_device: 0x0208,
                ..identity()
            },
        ] {
            assert_eq!(descriptors(&bytes, &changed), Err(CorpusError::Descriptor));
        }
        assert_eq!(
            descriptors(
                &bytes,
                &ImageIdentity {
                    version: "other",
                    ..identity()
                }
            ),
            Err(CorpusError::Version)
        );
        assert_eq!(
            descriptors(&bytes[..63], &identity()),
            Err(CorpusError::Header)
        );
        for tail in [&expected[..18], &expected[18..]] {
            let mut duplicate = bytes.clone();
            duplicate.extend_from_slice(tail);
            let length = u32::try_from(duplicate.len() - 64).unwrap();
            duplicate[..4].copy_from_slice(&length.to_le_bytes());
            assert_eq!(
                descriptors(&duplicate, &identity()),
                Err(CorpusError::Descriptor)
            );
        }
        assert_eq!(verify_updaters(&[], &[]).unwrap_err(), CorpusError::Size);
    }

    #[test]
    fn descriptor_extraction_checks_packet_limits_and_clock_source_in_the_actual_template() {
        let bytes = image();
        for signature in [&[7, 5, 2, 5, 0x90, 1][..], &[7, 5, 0x82, 0x25, 0x95, 1][..]] {
            let offset = bytes
                .windows(signature.len())
                .position(|window| window == signature)
                .unwrap();
            let mut changed = bytes.clone();
            changed[offset + 4] -= 1;
            assert_eq!(
                descriptors(&changed, &identity()),
                Err(CorpusError::Descriptor)
            );
        }
        let mut no_device = bytes.clone();
        no_device[72] = 17;
        assert_eq!(
            descriptors(&no_device, &identity()),
            Err(CorpusError::Descriptor)
        );
        let mut no_configuration = bytes.clone();
        no_configuration[90] = 8;
        assert_eq!(
            descriptors(&no_configuration, &identity()),
            Err(CorpusError::Descriptor)
        );
        let mut changed_clock = bytes.clone();
        for (signature, field) in [
            (&[8, 0x24, 0x0a, 2][..], 3),
            (&[8, 0x24, 0x0b, 1, 1, 2][..], 5),
        ] {
            let offset = bytes
                .windows(signature.len())
                .position(|window| window == signature)
                .unwrap();
            changed_clock[offset + field] = 9;
        }
        assert_eq!(
            motu_m4_core::Layout::parse(&changed_clock[72..], "M4")
                .unwrap()
                .clock_id(),
            9
        );
        assert_eq!(
            descriptors(&changed_clock, &identity()),
            Err(CorpusError::Descriptor)
        );
    }
}
