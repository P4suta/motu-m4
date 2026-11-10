use motu_m4_core::{Error, Layout};

const M4: &[u8] = include_bytes!("fixtures/m4-usb-descriptors.bin");

#[test]
fn reads_the_attached_m4_layout() {
    let layout = Layout::parse(M4, "M4").expect("device fixture");
    let out = layout.playback();
    let input = layout.capture();
    assert_eq!(
        (out.interface(), out.address(), out.max_packet()),
        (1, 0x02, 400)
    );
    assert_eq!(
        (input.interface(), input.address(), input.max_packet()),
        (2, 0x83, 405)
    );
    assert_eq!(layout.control_interface(), 0);
    assert_eq!(layout.clock_id(), 2);
    assert_eq!(layout.read_clock().bytes(), [0xa1, 1, 0, 1, 0, 2, 4, 0]);
    assert_eq!(layout.set_clock().bytes(), [0x21, 1, 0, 1, 0, 2, 4, 0]);
}

#[test]
fn refuses_every_truncated_prefix() {
    for length in 0..M4.len() {
        assert!(
            Layout::parse(&M4[..length], "M4").is_err(),
            "prefix {length}"
        );
    }
}

#[test]
fn refuses_a_different_product_with_the_same_usb_identifiers() {
    assert_eq!(Layout::parse(M4, "M2"), Err(Error::Layout));
}

#[test]
fn refuses_zero_descriptor_lengths_instead_of_looping() {
    let mut invalid = M4.to_vec();
    invalid[18] = 0;
    assert!(Layout::parse(&invalid, "M4").is_err());
}

fn offsets(predicate: impl Fn(&[u8]) -> bool) -> Vec<usize> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < M4.len() {
        let length = usize::from(M4[offset]);
        if predicate(&M4[offset..offset + length]) {
            result.push(offset);
        }
        offset += length;
    }
    result
}

fn reject_field(offset: usize, field: usize, value: u8) {
    let mut invalid = M4.to_vec();
    invalid[offset + field] = value;
    assert!(
        Layout::parse(&invalid, "M4").is_err(),
        "offset={offset}, field={field}, value={value}"
    );
}

#[test]
fn unsupported_formats_clocks_and_endpoint_layouts_fail_validation() {
    reject_field(0, 1, 2);
    for value in [0, 2, 255] {
        reject_field(0, 17, value);
    }
    reject_field(18, 1, 3);
    reject_field(18, 2, 0);
    for offset in offsets(|d| d.len() == 16 && d[1] == 0x24 && d[2] == 1) {
        for value in [0, 2, 8] {
            reject_field(offset, 10, value);
        }
        reject_field(offset, 5, 2);
        for field in 6..10 {
            reject_field(offset, field, 2);
        }
    }
    for offset in offsets(|d| d.len() == 6 && d[1] == 0x24 && d[2] == 2 && d[4] == 4) {
        for field in 3..6 {
            reject_field(offset, field, 0);
        }
    }
    for offset in offsets(|d| d.len() == 8 && d[1] == 0x24 && d[2] == 10) {
        reject_field(offset, 3, 0);
        for value in [0, 1, 2] {
            reject_field(offset, 5, value);
        }
    }
    for offset in offsets(|d| d.len() == 8 && d[1] == 0x24 && d[2] == 11) {
        reject_field(offset, 4, 0);
        reject_field(offset, 5, 99);
    }
    for offset in offsets(|d| d.len() == 7 && d[1] == 5 && [5, 37].contains(&d[3])) {
        for value in [0, 2] {
            reject_field(offset, 6, value);
        }
        for value in [0, 0x70] {
            reject_field(offset, 2, value);
        }
        reject_field(offset, 3, 1);
        for maximum in [0u16, 15, 1025, u16::MAX] {
            let mut invalid = M4.to_vec();
            invalid[offset + 4..offset + 6].copy_from_slice(&maximum.to_le_bytes());
            assert!(Layout::parse(&invalid, "M4").is_err());
        }
    }
}

#[test]
fn descriptor_lengths_are_checked_without_relying_on_truncation() {
    for offset in offsets(|d| {
        d[1] == 1
            || d[1] == 2
            || d[1] == 4
            || (d.len() == 7 && d[1] == 5 && [5, 37].contains(&d[3]))
            || (d.len() == 8 && d[1] == 0x24 && [10, 11].contains(&d[2]))
            || (d.len() == 16 && d[1] == 0x24 && d[2] == 1)
            || (d.len() == 6 && d[1] == 0x24 && d[2] == 2 && d[4] == 4)
            || (d.len() == 17 && d[1..3] == [0x24, 2])
            || (d.len() == 12 && d[1..3] == [0x24, 3])
            || (d.len() == 9 && d[1..5] == [0x24, 1, 0, 2])
    }) {
        let old = usize::from(M4[offset]);
        let mut invalid = M4.to_vec();
        invalid.insert(offset + old, 0);
        invalid[offset] += 1;
        if offset > 0 {
            invalid[20..22].copy_from_slice(&376u16.to_le_bytes());
        }
        if (44..127).contains(&offset) {
            invalid[50..52].copy_from_slice(&84u16.to_le_bytes());
        }
        assert!(
            Layout::parse(&invalid, "M4").is_err(),
            "descriptor at {offset}"
        );
    }
}

#[test]
fn control_requests_preserve_the_descriptor_entity_and_interface() {
    let mut input = M4.to_vec();
    for offset in offsets(|d| d.len() == 9 && d[1] == 4 && d[5..8] == [1, 1, 0x20]) {
        input[offset + 2] = 7;
    }
    for offset in offsets(|d| d.len() == 8 && d[1] == 0x24 && d[2] == 10) {
        input[offset + 3] = 8;
    }
    for offset in offsets(|d| d.len() == 8 && d[1] == 0x24 && d[2] == 11) {
        input[offset + 5] = 8;
    }
    let layout = Layout::parse(&input, "M4").unwrap();
    assert_eq!(layout.control_interface(), 7);
    assert_eq!(layout.clock_id(), 8);
    assert_eq!(layout.read_clock().bytes(), [0xa1, 1, 0, 1, 7, 8, 4, 0]);
    let mut invalid = M4.to_vec();
    for offset in offsets(|d| d.len() == 9 && d[1] == 4 && d[2] == 2 && d[5..8] == [1, 2, 0x20]) {
        invalid[offset + 2] = 1;
    }
    assert!(Layout::parse(&invalid, "M4").is_err());
}

#[test]
fn duplicate_protocol_descriptors_fail_validation() {
    for offset in offsets(|d| {
        (d.len() == 7 && d[1] == 5 && [5, 37].contains(&d[3]))
            || (d.len() == 8 && d[1] == 0x24 && [10, 11].contains(&d[2]))
            || (d.len() == 16 && d[1] == 0x24 && d[2] == 1)
            || (d.len() == 6 && d[1] == 0x24 && d[2] == 2 && d[4] == 4)
    }) {
        let length = usize::from(M4[offset]);
        let mut invalid = M4.to_vec();
        invalid.splice(offset..offset, M4[offset..offset + length].iter().copied());
        let total = u16::try_from(invalid.len() - 18).unwrap();
        invalid[20..22].copy_from_slice(&total.to_le_bytes());
        assert!(
            Layout::parse(&invalid, "M4").is_err(),
            "duplicate at {offset}"
        );
    }
}

#[test]
fn byte_corruption_never_panics_or_loops() {
    for offset in 0..M4.len() {
        for value in [0, 1, 2, 3, 16, 127, 255] {
            let mut input = M4.to_vec();
            input[offset] = value;
            let _ = Layout::parse(&input, "M4");
        }
    }
}

#[test]
fn stream_endpoints_require_both_channel_and_format_descriptors() {
    for offset in offsets(|d| {
        (d.len() == 16 && d[1] == 0x24 && d[2] == 1)
            || (d.len() == 6 && d[1] == 0x24 && d[2] == 2 && d[4] == 4)
    }) {
        let length = usize::from(M4[offset]);
        let mut invalid = M4.to_vec();
        invalid.drain(offset..offset + length);
        let total = u16::try_from(invalid.len() - 18).unwrap();
        invalid[20..22].copy_from_slice(&total.to_le_bytes());
        assert!(
            Layout::parse(&invalid, "M4").is_err(),
            "removed descriptor at {offset}"
        );
    }
}

#[test]
fn audio_stream_class_protocol_and_alternate_are_independent_requirements() {
    for offset in offsets(|d| d.len() == 9 && d[1] == 4 && d[3] == 1 && d[5..8] == [1, 2, 0x20]) {
        for (field, value) in [(3, 0), (3, 2), (5, 2), (6, 0), (6, 3), (7, 0)] {
            reject_field(offset, field, value);
        }
    }
}

#[test]
fn streaming_terminals_must_share_the_selected_clock_and_valid_signal_paths() {
    for offset in offsets(|d| d.len() == 16 && d[1..3] == [0x24, 1]) {
        for id in [0, 1, 2, 4, 6, 255] {
            reject_field(offset, 3, id);
        }
    }
    for offset in offsets(|d| d.len() == 17 && d[1..3] == [0x24, 2]) {
        for clock in [0, 2, 3, 255] {
            reject_field(offset, 7, clock);
        }
        reject_field(offset, 8, 2);
    }
    for offset in offsets(|d| d.len() == 12 && d[1..3] == [0x24, 3]) {
        for clock in [0, 2, 3, 255] {
            reject_field(offset, 8, clock);
        }
        for source in [0, 1, 2, 4, 5, 255] {
            reject_field(offset, 7, source);
        }
    }
    for offset in offsets(|d| d.len() == 8 && d[1..3] == [0x24, 11]) {
        for id in [0, 2, 3, 4, 5, 6] {
            reject_field(offset, 3, id);
        }
    }
    for offset in offsets(|d| d.len() == 9 && d[1..5] == [0x24, 1, 0, 2]) {
        reject_field(offset, 3, 1);
        reject_field(offset, 6, 0);
    }
}

#[test]
fn validity_controls_respect_the_descriptor_and_reject_reserved_values() {
    let clock = offsets(|d| d.len() == 8 && d[1..3] == [0x24, 10])[0];
    let mut optional = M4.to_vec();
    optional[clock + 5] = 3;
    assert_eq!(
        Layout::parse(&optional, "M4").unwrap().clock_validity(),
        None
    );
    for value in [11, 15, 19, 255] {
        reject_field(clock, 5, value);
    }
}

fn remap_entities(mapping: &[(u8, u8)]) -> Vec<u8> {
    let mut bytes = M4.to_vec();
    let replace = |value: u8| {
        mapping
            .iter()
            .find_map(|(from, to)| (*from == value).then_some(*to))
            .unwrap_or(value)
    };
    let mut audio_control = false;
    for offset in offsets(|_| true) {
        let length = usize::from(bytes[offset]);
        let d = &mut bytes[offset..offset + length];
        if d[1] == 4 {
            audio_control = d[5..8] == [1, 1, 0x20];
        }
        if d[1] != 0x24 {
            continue;
        }
        if audio_control {
            match d[2] {
                2 => {
                    d[3] = replace(d[3]);
                    d[7] = replace(d[7]);
                }
                3 => {
                    d[3] = replace(d[3]);
                    d[7] = replace(d[7]);
                    d[8] = replace(d[8]);
                }
                10 => d[3] = replace(d[3]),
                11 => {
                    d[3] = replace(d[3]);
                    d[5] = replace(d[5]);
                }
                _ => {}
            }
        } else if d.len() == 16 && d[2] == 1 {
            d[3] = replace(d[3]);
        }
    }
    bytes
}

#[test]
fn coherent_entity_renumbering_preserves_paths_but_refuses_reserved_or_aliased_ids() {
    let valid = remap_entities(&[(1, 9), (2, 8), (3, 7), (4, 6), (5, 5), (6, 4)]);
    assert_eq!(Layout::parse(&valid, "M4").unwrap().clock_id(), 8);
    for mapping in [[(3, 0)], [(3, 2)], [(6, 0)], [(6, 3)]] {
        assert!(
            Layout::parse(&remap_entities(&mapping), "M4").is_err(),
            "{mapping:?}"
        );
    }
}
