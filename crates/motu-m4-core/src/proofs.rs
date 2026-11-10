use super::*;

#[kani::proof]
#[kani::unwind(1)]
fn arbitrary_recording_frame_counts_cannot_overflow_the_wave_container() {
    let frames: u64 = kani::any();
    let selector: u8 = kani::any();
    let channels = match selector {
        0 => AudioChannels::Mono,
        1 => AudioChannels::Stereo,
        _ => AudioChannels::Four,
    };
    let result = RecordingFrames::new(frames, channels);
    let maximum = (u64::from(u32::MAX) - 256) / (u64::from(channels.get()) * 3);
    assert_eq!(result.is_ok(), frames != 0 && frames <= maximum);
    if let Ok(count) = result {
        assert_eq!(count.get(), frames);
        assert!(count.get() * u64::from(channels.get()) * 3 + 256 <= u64::from(u32::MAX));
    }
    kani::cover!(frames == maximum && result.is_ok());
    kani::cover!(frames == maximum + 1 && result.is_err());
    kani::cover!(frames == 0 && result.is_err());
}

#[kani::proof]
#[kani::unwind(1)]
fn native_integer_samples_preserve_the_signed_scale_and_zero_padding() {
    let sample: i32 = kani::any();
    let bits: u16 = kani::any();
    let expected = match bits {
        16 if (-32_768..=32_767).contains(&sample) => Some(i64::from(sample) * 65_536),
        24 if (-8_388_608..=8_388_607).contains(&sample) => Some(i64::from(sample) * 256),
        32 => Some(i64::from(sample).div_euclid(256) * 256),
        _ => None,
    };
    let result = pcm_from_signed(sample, bits);
    assert_eq!(result.map(i64::from).ok(), expected);
    if let Ok(encoded) = result {
        assert_eq!(encoded & 255, 0);
    }
    kani::cover!(bits == 16 && sample == -32_768);
    kani::cover!(bits == 24 && sample == 8_388_607);
    kani::cover!(bits == 32 && sample < 0);
    kani::cover!(result.is_err());
}

#[kani::proof]
#[kani::unwind(1)]
fn recording_lengths_keep_every_complete_frame_below_the_wave_size_limit() {
    let rate_index: u8 = kani::any();
    let seconds: u32 = kani::any();
    let channel_index: u8 = kani::any();
    let hz = match rate_index {
        0 => 44_100,
        1 => 48_000,
        2 => 88_200,
        3 => 96_000,
        4 => 176_400,
        _ => 192_000,
    };
    let channels = match channel_index {
        0 => AudioChannels::Mono,
        1 => AudioChannels::Stereo,
        _ => AudioChannels::Four,
    };
    if let Ok(rate) = SampleRate::new(hz) {
        let result = native_recording_frames(rate, seconds, channels);
        let frames = u64::from(hz) * u64::from(seconds);
        let bytes = frames * u64::from(channels.get()) * 3;
        assert_eq!(
            result.is_ok(),
            frames > 0 && bytes + 256 <= u64::from(u32::MAX)
        );
        if let Ok(actual) = result {
            assert_eq!(actual, frames);
        }
        kani::cover!(result.is_ok());
        kani::cover!(seconds == 0 && result.is_err());
        kani::cover!(seconds == u32::MAX && result.is_err());
    }
}

#[kani::proof]
fn pcm_encoding_preserves_the_valid_bits_and_zeroes_padding() {
    let word: u32 = kani::any();
    let encoded = u32::from_le_bytes(PcmSample::from_bits(word).to_le_bytes());
    assert_eq!(encoded & 255, 0);
    assert_eq!(encoded >> 8, word >> 8);
    assert_eq!(encoded >> 31, word >> 31);
    kani::cover!(word >> 31 == 1);
    kani::cover!(word & 255 != 0);
}

#[kani::proof]
fn identity_matches_only_admitted_products() {
    let vendor: u16 = kani::any();
    let product: u16 = kani::any();
    let m4: bool = kani::any();
    let result = Hardware::identify(vendor, product, if m4 { "M4" } else { "M2" });
    assert_eq!(
        result.is_ok(),
        vendor == 0x07fd && (product == 8 || product == 11) && m4
    );
    kani::cover!(result == Ok(Hardware::Original));
    kani::cover!(result == Ok(Hardware::Revised));
    kani::cover!(result.is_err());
}

#[kani::proof]
fn hardware_timing_matches_linux_generation_quirks() {
    let revised: bool = kani::any();
    let hardware = if revised {
        Hardware::Revised
    } else {
        Hardware::Original
    };
    assert_eq!(
        hardware.boot_delay_us(),
        if revised { 0 } else { 4_000_000 }
    );
    assert_eq!(hardware.control_delay_us(), if revised { 1000 } else { 0 });
}

#[kani::proof]
fn firmware_payload_lengths_admit_exact_bounded_container_sizes() {
    let total: usize = kani::any();
    let declared: u32 = kani::any();
    let result = firmware::payload_length(total, declared);
    assert_eq!(
        result.is_ok(),
        (72..=1_048_576).contains(&total) && u64::from(declared) + 64 == total as u64
    );
}

#[kani::proof]
#[kani::unwind(61)]
fn firmware_version_fields_require_printable_prefixes_and_zero_padding() {
    let bytes: [u8; 60] = kani::any();
    let index: usize = kani::any();
    kani::assume(index < 60);
    if let Ok(end) = firmware::version_length(&bytes) {
        assert!((1..60).contains(&end));
        assert_eq!(bytes[end], 0);
        if index < end {
            assert!((32..=126).contains(&bytes[index]));
        } else {
            assert_eq!(bytes[index], 0);
        }
    }
}

#[kani::proof]
#[kani::unwind(33)]
fn firmware_candidate_spans_preserve_file_offsets_and_payload_bounds() {
    let payload: [u8; 32] = kani::any();
    let offset: usize = kani::any();
    let length: usize = kani::any();
    if let Some(candidate) = UsbDescriptorCandidate::from_payload(&payload, offset, length) {
        assert_eq!(candidate.offset(), offset + 64);
        assert!(offset + length <= payload.len());
        assert_eq!(candidate.bytes().len(), length);
        assert_eq!(
            candidate.bytes(),
            &payload[offset..offset + candidate.bytes().len()]
        );
    }
}

#[kani::proof]
fn feedback_preserves_complete_frames_and_endpoint_bounds() {
    let actual: u32 = kani::any();
    let input: u16 = kani::any();
    let output: u16 = kani::any();
    let result = PacketLength::from_feedback(actual, input, output);
    assert_eq!(
        result.is_ok(),
        actual <= u32::from(input) && actual <= u32::from(output) && actual % 16 == 0
    );
    if let Ok(packet) = result {
        assert_eq!(u32::from(packet.bytes()), actual);
        assert_eq!(u32::from(packet.frames()) * 16, actual);
    }
    kani::cover!(result.is_ok() && actual > 0);
    kani::cover!(result.is_err());
}

#[kani::proof]
fn frame_counts_bound_memory_and_reject_empty_audio() {
    let frames: u32 = kani::any();
    let result = FrameCount::new(frames);
    assert_eq!(result.is_ok(), frames > 0 && frames <= 480_000);
    if let Ok(count) = result {
        assert_eq!(count.get(), frames);
        assert_eq!(count.bytes(), frames * 16);
        assert!(count.bytes() <= 7_680_000);
    }
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
fn descriptor_lengths_advance_without_exceeding_the_input() {
    let length: u8 = kani::any();
    let remaining: usize = kani::any();
    let result = DescriptorLength::new(length, remaining);
    assert_eq!(
        result.is_ok(),
        length >= 2 && usize::from(length) <= remaining
    );
    if let Ok(value) = result {
        assert_eq!(value.get(), length);
        assert!(remaining - usize::from(value.get()) < remaining);
    }
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
#[kani::unwind(8)]
fn recovery_finishes_and_preserves_the_previous_clock() {
    let choice: u8 = kani::any();
    kani::assume(choice < 8);
    let rate: u32 = kani::any();
    let mut pending = match choice {
        0 => Recovery::Complete,
        1 => Recovery::Attachment,
        2 => Recovery::Claim,
        3 => Recovery::Clock(rate),
        4 => Recovery::Capture(rate),
        5 => Recovery::Duplex(rate),
        6 => Recovery::CaptureUnchanged,
        _ => Recovery::DuplexUnchanged,
    };
    let mut count = 0;
    while let Some((action, remaining)) = pending.next() {
        if let RecoveryAction::RestoreClock(previous) = action {
            assert!(choice < 6);
            assert_eq!(previous, rate);
        }
        pending = remaining;
        count += 1;
        assert!(count <= 5);
    }
    let expected = match choice {
        6 => 3,
        7 => 4,
        _ => choice,
    };
    assert_eq!(count, expected);
    assert_eq!(pending, Recovery::Complete);
    kani::cover!(choice == 5);
    kani::cover!(choice == 0);
    kani::cover!(choice == 7);
}

#[kani::proof]
#[kani::unwind(8)]
fn incomplete_profiles_are_rejected_before_decoding() {
    let length: usize = kani::any();
    let result = profile_length(length);
    assert_eq!(result.is_ok(), length >= 128);
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
fn subminimum_profile_bytes_never_enter_the_decoder() {
    let bytes: [u8; 127] = kani::any();
    assert_eq!(Layout::parse(&bytes, "M4"), Err(Error::Descriptor));
}

#[kani::proof]
#[kani::unwind(8)]
fn individual_descriptor_slices_preserve_bounds() {
    let bytes: [u8; 32] = kani::any();
    let length: usize = kani::any();
    kani::assume(length <= bytes.len());
    let result = descriptor(&bytes[..length]);
    if let Ok((head, tail)) = result {
        assert!(head.len() >= 2);
        assert!(head.len() <= length);
        assert_eq!(head.len() + tail.len(), length);
    }
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
#[kani::unwind(512)]
fn the_recorded_fixture_builds_the_expected_clock_requests() {
    let result = Layout::parse(
        include_bytes!("../tests/fixtures/m4-usb-descriptors.bin"),
        "M4",
    );
    assert!(result.is_ok());
    if let Ok(layout) = result {
        assert_eq!(layout.read_clock().bytes(), [0xa1, 1, 0, 1, 0, 2, 4, 0]);
        assert_eq!(layout.set_clock().bytes(), [0x21, 1, 0, 1, 0, 2, 4, 0]);
        assert_eq!(layout.capture().address(), 0x83);
        assert_eq!(layout.playback().address(), 2);
    }
}

#[kani::proof]
fn sample_rates_admit_exact_frequencies_and_bound_packet_memory() {
    let hz: u32 = kani::any();
    let result = SampleRate::new(hz);
    assert_eq!(
        result.is_ok(),
        matches!(hz, 44_100 | 48_000 | 88_200 | 96_000 | 176_400 | 192_000)
    );
    if let Ok(rate) = result {
        assert_eq!(rate.hz(), hz);
        assert!(rate.max_packet_bytes() <= 400);
        assert!(rate.max_packet_bytes() >= 112);
        assert_eq!(rate.max_packet_bytes() % 16, 0);
        assert!(rate.max_packet_bytes() / 16 > hz / 8000);
    }
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
#[kani::unwind(4)]
fn clock_range_counts_bound_control_memory() {
    let bytes: [u8; 2] = kani::any();
    let count = u16::from_le_bytes(bytes);
    let result = ClockRangeCount::parse(&bytes);
    assert_eq!(result.is_ok(), count >= 1 && count <= 32);
    if let Ok(count) = result {
        assert!(count.bytes() >= 14 && count.bytes() <= 386);
        assert_eq!(count.bytes(), 2 + u16::from_le_bytes(bytes) * 12);
    }
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
fn rate_range_offsets_preserve_bounds_without_arithmetic_overflow() {
    let min: u32 = kani::any();
    let max: u32 = kani::any();
    let step: u32 = kani::any();
    let hz: u32 = kani::any();
    let result = RateRange::new(min, max, step);
    assert_eq!(
        result.is_ok(),
        min > 0 && min <= max && ((min == max) == (step == 0))
    );
    if let Ok(range) = result {
        let accepted = range.offset(hz);
        assert_eq!(accepted.is_some(), hz >= min && hz <= max);
        if let Some(offset) = accepted {
            assert!(hz >= min && hz <= max);
            assert_eq!(offset, hz - min);
            assert!(offset <= max - min);
        }
        assert_eq!(range.offset(min), Some(0));
        kani::cover!(accepted.is_some() && step > 0);
        kani::cover!(accepted.is_none());
    }
    kani::cover!(result.is_err());
}

#[kani::proof]
#[kani::unwind(32)]
fn arbitrary_clock_range_responses_preserve_bounds() {
    let bytes: [u8; 26] = kani::any();
    let result = ClockRanges::parse(&bytes);
    if result.is_ok() {
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 2);
    }
    kani::cover!(result.is_ok());
    kani::cover!(result.is_err());
}

#[kani::proof]
#[kani::unwind(32)]
fn decoded_discrete_and_stepped_ranges_select_exact_m4_rates() {
    let response = [
        2, 0, 0x44, 0xac, 0, 0, 0x44, 0xac, 0, 0, 0, 0, 0, 0, 0x80, 0xbb, 0, 0, 0, 0x77, 1, 0,
        0x80, 0xbb, 0, 0,
    ];
    let ranges = ClockRanges::parse(&response);
    assert!(ranges.is_ok());
    let rate = SampleRate::new(kani::any());
    if let (Ok(ranges), Ok(rate)) = (ranges, rate) {
        assert_eq!(
            ranges.supports(rate),
            matches!(rate.hz(), 44_100 | 48_000 | 96_000)
        );
        kani::cover!(ranges.supports(rate));
        kani::cover!(!ranges.supports(rate));
    }
}
