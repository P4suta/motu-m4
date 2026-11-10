use motu_m4_core::{ClockRangeCount, ClockRanges, Error, Layout, RateRange, SampleRate};

#[test]
fn firmware_clock_table_decodes_exactly_the_six_supported_frequencies() {
    let bytes = include_bytes!("fixtures/m-series-clock-ranges.bin");
    assert_eq!(ClockRangeCount::parse(&bytes[..2]).unwrap().bytes(), 74);
    let ranges = ClockRanges::parse(bytes).unwrap();
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        assert!(ranges.supports(SampleRate::new(hz).unwrap()));
    }
    for length in 0..bytes.len() {
        assert!(ClockRanges::parse(&bytes[..length]).is_err());
    }
}

const M4: &[u8] = include_bytes!("fixtures/m4-usb-descriptors.bin");

#[test]
fn rates_admit_exactly_the_six_m4_frequencies() {
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        assert_eq!(SampleRate::new(hz).unwrap().hz(), hz);
        let required = (hz.div_ceil(8000) + 1) * 16;
        assert_eq!(SampleRate::new(hz).unwrap().max_packet_bytes(), required);
    }
    for hz in [
        0,
        1,
        44_099,
        44_101,
        47_999,
        48_001,
        96_001,
        192_001,
        u32::MAX,
    ] {
        assert_eq!(SampleRate::new(hz), Err(Error::Clock));
    }
}

#[test]
fn control_builders_encode_range_lengths_and_validity_without_writes() {
    let layout = Layout::parse(M4, "M4").unwrap();
    assert_eq!(layout.clock_ranges(2).bytes(), [0xa1, 2, 0, 1, 0, 2, 2, 0]);
    assert_eq!(
        layout.clock_ranges(74).bytes(),
        [0xa1, 2, 0, 1, 0, 2, 74, 0]
    );
    assert_eq!(
        layout.clock_validity().unwrap().bytes(),
        [0xa1, 1, 0, 2, 0, 2, 1, 0]
    );
    assert_eq!(ClockRangeCount::parse(&[6, 0]).unwrap().bytes(), 74);
    for input in [
        &[][..],
        &[0][..],
        &[0, 0][..],
        &[33, 0][..],
        &[255, 255][..],
        &[1, 0, 0][..],
    ] {
        assert!(ClockRangeCount::parse(input).is_err());
    }
    assert_eq!(ClockRangeCount::parse(&[32, 0]).unwrap().bytes(), 386);
}

#[test]
fn range_membership_uses_bounds_and_resolution() {
    let range = RateRange::new(44_100, 48_000, 300).unwrap();
    for hz in [44_100, 44_400, 47_700, 48_000] {
        assert!(range.contains(hz));
    }
    for hz in [0, 44_099, 44_101, 47_999, 48_001, u32::MAX] {
        assert!(!range.contains(hz));
    }
    let discrete = RateRange::new(48_000, 48_000, 0).unwrap();
    assert!(discrete.contains(48_000));
    assert!(!discrete.contains(48_001));
    for (min, max, step) in [
        (0, 0, 0),
        (48_000, 44_100, 1),
        (44_100, 48_000, 0),
        (48_000, 48_000, 1),
    ] {
        assert!(RateRange::new(min, max, step).is_err());
    }
}

fn response(ranges: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut data = u16::try_from(ranges.len()).unwrap().to_le_bytes().to_vec();
    for &(min, max, step) in ranges {
        for value in [min, max, step] {
            data.extend(value.to_le_bytes());
        }
    }
    data
}

#[test]
fn clock_ranges_validate_the_entire_response_before_admitting_a_rate() {
    let data = response(&[(44_100, 44_100, 0), (48_000, 96_000, 48_000)]);
    let ranges = ClockRanges::parse(&data).unwrap();
    for rate in [44_100, 48_000, 96_000] {
        assert!(ranges.supports(SampleRate::new(rate).unwrap()));
    }
    assert!(!ranges.supports(SampleRate::new(88_200).unwrap()));
    for length in 0..data.len() {
        assert!(ClockRanges::parse(&data[..length]).is_err());
    }
    for entries in [
        vec![(48_000, 48_000, 0), (44_100, 44_100, 0)],
        vec![(48_000, 48_000, 0), (48_000, 48_000, 0)],
        vec![(44_100, 48_000, 1), (48_000, 96_000, 1)],
        vec![(48_000, 48_000, 0), (96_000, 44_100, 1)],
    ] {
        assert!(ClockRanges::parse(&response(&entries)).is_err());
    }
    let mut trailing = data;
    trailing.push(0);
    assert!(ClockRanges::parse(&trailing).is_err());
}
