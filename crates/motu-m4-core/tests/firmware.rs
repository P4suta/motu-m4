use motu_m4_core::{FirmwareError, FirmwareImage, Hardware, Layout};

fn image() -> Vec<u8> {
    let mut bytes = vec![0; 72];
    bytes[..4].copy_from_slice(&8_u32.to_le_bytes());
    bytes[4..13].copy_from_slice(b"2.2.9+641");
    bytes[64..68].copy_from_slice(&0x2003_0000_u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&0x2000_0201_u32.to_le_bytes());
    bytes
}

#[test]
fn observed_container_separates_version_header_and_payload() {
    let bytes = image();
    let firmware = FirmwareImage::parse(&bytes).unwrap();
    assert_eq!(firmware.version(), "2.2.9+641");
    assert_eq!(firmware.payload(), &bytes[64..]);
    assert_eq!(firmware.initial_stack_pointer(), 0x2003_0000);
    assert_eq!(firmware.reset_handler(), 0x2000_0201);
}

#[test]
fn firmware_header_refuses_truncation_trailing_bytes_and_inconsistent_lengths() {
    let bytes = image();
    for length in 0..72 {
        assert_eq!(
            FirmwareImage::parse(&bytes[..length]),
            Err(FirmwareError::Length)
        );
    }
    for length in [0, 1, 7, 9, u32::MAX] {
        let mut changed = bytes.clone();
        changed[..4].copy_from_slice(&length.to_le_bytes());
        assert_eq!(FirmwareImage::parse(&changed), Err(FirmwareError::Length));
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(FirmwareImage::parse(&trailing), Err(FirmwareError::Length));
    let mut oversized = vec![0; 1_048_577];
    oversized[..4].copy_from_slice(&1_048_513_u32.to_le_bytes());
    oversized[4..13].copy_from_slice(b"2.2.9+641");
    assert_eq!(FirmwareImage::parse(&oversized), Err(FirmwareError::Length));
}

#[test]
fn firmware_version_requires_printable_ascii_a_terminator_and_zero_padding() {
    let mut empty = image();
    empty[4..64].fill(0);
    assert_eq!(FirmwareImage::parse(&empty), Err(FirmwareError::Version));
    for (offset, value) in [(4, 0), (4, 31), (4, 127), (4, 128), (63, 1)] {
        let mut bytes = image();
        bytes[offset] = value;
        assert_eq!(FirmwareImage::parse(&bytes), Err(FirmwareError::Version));
    }
    let mut bytes = image();
    bytes[4..64].fill(b'A');
    assert_eq!(FirmwareImage::parse(&bytes), Err(FirmwareError::Version));
    bytes[63] = 0;
    assert_eq!(FirmwareImage::parse(&bytes).unwrap().version().len(), 59);
    for character in [32, 126] {
        bytes[4..63].fill(character);
        assert_eq!(
            FirmwareImage::parse(&bytes).unwrap().version().as_bytes(),
            &[character; 59]
        );
    }
}

#[test]
fn usb_candidates_preserve_file_offsets_and_ignore_truncated_records() {
    let descriptors = include_bytes!("fixtures/m4-usb-descriptors.bin");
    let mut bytes = image();
    bytes.extend_from_slice(descriptors);
    bytes.extend_from_slice(&[18, 1, 0, 2, 0xef, 2, 1, 64, 0xfd, 7]);
    let length = u32::try_from(bytes.len() - 64).unwrap();
    bytes[..4].copy_from_slice(&length.to_le_bytes());
    let firmware = FirmwareImage::parse(&bytes).unwrap();
    let devices: Vec<_> = firmware.usb_devices().collect();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].offset(), 72);
    assert_eq!(devices[0].bytes(), &descriptors[..18]);
    let configurations: Vec<_> = firmware.usb_configurations().collect();
    assert_eq!(configurations.len(), 1);
    assert_eq!(configurations[0].offset(), 90);
    assert_eq!(configurations[0].bytes(), &descriptors[18..]);
    bytes[90 + 9] = 1;
    assert_eq!(
        FirmwareImage::parse(&bytes)
            .unwrap()
            .usb_configurations()
            .count(),
        0
    );
}

#[test]
fn usb_candidates_refuse_other_identifiers_and_malformed_configuration_headers() {
    let descriptor_bytes = include_bytes!("fixtures/m4-nxp-firmware-descriptors.bin");
    let mut bytes = image();
    bytes.extend_from_slice(descriptor_bytes);
    let length = u32::try_from(bytes.len() - 64).unwrap();
    bytes[..4].copy_from_slice(&length.to_le_bytes());
    for (offset, value) in [
        (72, 17),
        (73, 2),
        (74, 1),
        (75, 1),
        (79, 8),
        (80, 0),
        (81, 0),
        (89, 2),
    ] {
        let mut changed = bytes.clone();
        changed[offset] = value;
        assert_eq!(
            FirmwareImage::parse(&changed)
                .unwrap()
                .usb_devices()
                .count(),
            0
        );
    }
    for (offset, value) in [
        (90, 8),
        (91, 3),
        (92, 8),
        (93, 255),
        (94, 0),
        (95, 2),
        (99, 0),
        (99, 255),
    ] {
        let mut changed = bytes.clone();
        changed[offset] = value;
        assert_eq!(
            FirmwareImage::parse(&changed)
                .unwrap()
                .usb_configurations()
                .count(),
            0
        );
    }
}

#[test]
fn configuration_candidates_distinguish_an_empty_header_from_a_zero_length_record() {
    let mut bytes = image();
    bytes.extend_from_slice(&[9, 2, 9, 0, 1, 1, 0, 0x80, 250]);
    bytes[..4].copy_from_slice(&17_u32.to_le_bytes());
    let candidate = FirmwareImage::parse(&bytes)
        .unwrap()
        .usb_configurations()
        .next()
        .unwrap();
    assert_eq!(candidate.bytes(), &[9, 2, 9, 0, 1, 1, 0, 0x80, 250]);
    for length in 0..9_u8 {
        bytes[74] = length;
        assert!(FirmwareImage::parse(&bytes)
            .unwrap()
            .usb_configurations()
            .next()
            .is_none());
    }
}

#[test]
fn hardware_timing_survives_descriptor_parsing() {
    let mut bytes = include_bytes!("fixtures/m4-usb-descriptors.bin").to_vec();
    let revised = Layout::parse(&bytes, "M4").unwrap();
    assert_eq!(revised.hardware(), Hardware::Revised);
    assert_eq!(revised.hardware().boot_delay_us(), 0);
    assert_eq!(revised.read_clock().post_delay_us(), 1000);
    assert_eq!(revised.set_clock().post_delay_us(), 1000);
    assert_eq!(revised.clock_ranges(74).post_delay_us(), 1000);
    assert_eq!(revised.clock_validity().unwrap().post_delay_us(), 1000);
    bytes[10] = 8;
    let original = Layout::parse(&bytes, "M4").unwrap();
    assert_eq!(original.hardware(), Hardware::Original);
    assert_eq!(original.hardware().boot_delay_us(), 4_000_000);
    assert_eq!(original.read_clock().post_delay_us(), 0);
    assert_eq!(original.set_clock().post_delay_us(), 0);
    assert_eq!(original.clock_ranges(2).post_delay_us(), 0);
    assert_eq!(original.clock_validity().unwrap().post_delay_us(), 0);
}
