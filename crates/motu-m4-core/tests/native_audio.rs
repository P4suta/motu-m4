use motu_m4_core::{
    native_recording_frames, pcm_from_float, pcm_from_signed, AudioChannels, AudioInputs,
    AudioOutputs, RecordingFrames, SampleRate,
};

#[test]
fn wav_samples_use_the_m4_signed_24_bit_scale() {
    for (sample, bits, expected) in [
        (0, 16, 0),
        (1, 16, 65_536),
        (-1, 16, -65_536),
        (32_767, 16, 0x7fff0000),
        (-32_768, 16, i32::MIN),
        (8_388_607, 24, 0x7fffff00),
        (-8_388_608, 24, i32::MIN),
        (i32::MAX, 32, 0x7fffff00),
        (i32::MIN, 32, i32::MIN),
        (-1, 32, -256),
    ] {
        assert_eq!(pcm_from_signed(sample, bits).unwrap(), expected);
    }
    for (sample, bits) in [(32_768, 16), (-32_769, 16), (8_388_608, 24), (0, 8), (0, 0)] {
        assert!(pcm_from_signed(sample, bits).is_err());
    }
    for (sample, expected) in [
        (0.0, 0),
        (0.5, 1_073_741_824),
        (-0.5, -1_073_741_824),
        (1.0, 0x7fffff00),
        (2.0, 0x7fffff00),
        (-1.0, i32::MIN),
        (-2.0, i32::MIN),
    ] {
        assert_eq!(pcm_from_float(sample).unwrap(), expected);
    }
    for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(pcm_from_float(sample).is_err());
    }
}

#[test]
fn channel_routing_preserves_independent_inputs_and_output_pairs() {
    let frame = [256, 512, 768, 1024];
    for (selection, count, expected) in [
        (AudioInputs::Input1, 1, [256, 0, 0, 0]),
        (AudioInputs::Input2, 1, [512, 0, 0, 0]),
        (AudioInputs::Input3, 1, [768, 0, 0, 0]),
        (AudioInputs::Input4, 1, [1024, 0, 0, 0]),
        (AudioInputs::Pair12, 2, [256, 512, 0, 0]),
        (AudioInputs::Pair34, 2, [768, 1024, 0, 0]),
        (AudioInputs::All, 4, frame),
    ] {
        assert_eq!(selection.channels().get(), count);
        assert_eq!(selection.select(frame), expected);
    }
    assert_eq!(
        AudioOutputs::Main
            .route(frame, AudioChannels::Mono)
            .unwrap(),
        [256, 256, 0, 0]
    );
    assert_eq!(
        AudioOutputs::Line
            .route(frame, AudioChannels::Stereo)
            .unwrap(),
        [0, 0, 256, 512]
    );
    assert_eq!(
        AudioOutputs::All.route(frame, AudioChannels::Four).unwrap(),
        frame
    );
    assert!(AudioOutputs::Main
        .route(frame, AudioChannels::Four)
        .is_err());
    assert!(AudioOutputs::Line
        .route(frame, AudioChannels::Four)
        .is_err());
    for channels in [0, 3, 5, u16::MAX] {
        assert!(AudioChannels::new(channels).is_err());
    }
}

#[test]
fn recording_lengths_reserve_space_for_a_complete_wave_header() {
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        let rate = SampleRate::new(hz).unwrap();
        for channels in [
            AudioChannels::Mono,
            AudioChannels::Stereo,
            AudioChannels::Four,
        ] {
            assert_eq!(
                native_recording_frames(rate, 1, channels).unwrap(),
                u64::from(hz)
            );
            assert!(native_recording_frames(rate, 0, channels).is_err());
            assert!(native_recording_frames(rate, u32::MAX, channels).is_err());
        }
    }
    let rate = SampleRate::new(192_000).unwrap();
    assert!(native_recording_frames(rate, 1864, AudioChannels::Four).is_ok());
    assert!(native_recording_frames(rate, 1865, AudioChannels::Four).is_err());
}

#[test]
fn arbitrary_recording_frame_counts_share_the_exact_wave_size_boundary() {
    for (number, channels) in [
        (1, AudioChannels::Mono),
        (2, AudioChannels::Stereo),
        (4, AudioChannels::Four),
    ] {
        assert_eq!(AudioChannels::new(number).unwrap(), channels);
        let maximum = (u64::from(u32::MAX) - 256) / (u64::from(number) * 3);
        assert_eq!(
            RecordingFrames::new(maximum, channels).unwrap().get(),
            maximum
        );
        assert!(RecordingFrames::new(maximum + 1, channels).is_err());
        assert!(RecordingFrames::new(0, channels).is_err());
        assert!(RecordingFrames::new(u64::MAX, channels).is_err());
    }
}
