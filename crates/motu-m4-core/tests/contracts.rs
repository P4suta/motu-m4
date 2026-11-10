use motu_m4_core::{Error, Hardware, PacketLength};

#[test]
fn frame_and_descriptor_lengths_reject_invalid_bounds() {
    use motu_m4_core::{DescriptorLength, FrameCount};
    for frames in [0, 480_001, u32::MAX] {
        assert_eq!(FrameCount::new(frames), Err(Error::FrameLimit));
    }
    for frames in [1, 48_000, 480_000] {
        let value = FrameCount::new(frames).unwrap();
        assert_eq!(value.get(), frames);
        assert_eq!(value.bytes(), frames * 16);
    }
    for (length, remaining) in [(0, 100), (1, 100), (3, 2), (255, 254)] {
        assert_eq!(
            DescriptorLength::new(length, remaining),
            Err(Error::Descriptor)
        );
    }
    for (length, remaining) in [(2, 2), (2, 3), (255, 255), (255, usize::MAX)] {
        assert_eq!(
            DescriptorLength::new(length, remaining).unwrap().get(),
            length
        );
    }
}

#[test]
fn recovery_releases_resources_in_reverse_order() {
    use motu_m4_core::{Recovery, RecoveryAction};
    let expected = [
        RecoveryAction::StopPlayback,
        RecoveryAction::StopCapture,
        RecoveryAction::RestoreClock(96_000),
        RecoveryAction::Release,
        RecoveryAction::Attach,
    ];
    let states = [
        Recovery::Duplex(96_000),
        Recovery::Capture(96_000),
        Recovery::Clock(96_000),
        Recovery::Claim,
        Recovery::Attachment,
        Recovery::Complete,
    ];
    for (index, state) in states.into_iter().enumerate() {
        let mut current = state;
        let mut actions = Vec::new();
        for _ in 0..6 {
            if let Some((action, remaining)) = current.next() {
                actions.push(action);
                current = remaining;
            }
        }
        assert_eq!(actions, &expected[index..]);
        assert_eq!(current, Recovery::Complete);
    }
}

#[test]
fn identifies_m4_without_accepting_the_m2_that_shares_its_ids() {
    assert_eq!(
        Hardware::identify(0x07fd, 0x000b, "M4"),
        Ok(Hardware::Revised)
    );
    assert_eq!(
        Hardware::identify(0x07fd, 0x0008, "M4"),
        Ok(Hardware::Original)
    );
    assert_eq!(
        Hardware::identify(0x07fd, 0x000b, "M2"),
        Err(Error::WrongDevice)
    );
    assert_eq!(
        Hardware::identify(0x07fd, 0x000b, "M4\0"),
        Err(Error::WrongDevice)
    );
    assert_eq!(
        Hardware::identify(0x1234, 0x000b, "M4"),
        Err(Error::WrongDevice)
    );
    assert_eq!(
        Hardware::identify(0x07fd, 0x1234, "M4"),
        Err(Error::WrongDevice)
    );
}

#[test]
fn feedback_uses_whole_four_channel_frames_within_both_endpoint_limits() {
    assert_eq!(
        PacketLength::from_feedback(96, 96, 400).map(PacketLength::bytes),
        Ok(96)
    );
    assert_eq!(
        PacketLength::from_feedback(0, 0, 0).map(PacketLength::bytes),
        Ok(0)
    );
    assert_eq!(
        PacketLength::from_feedback(96, 405, 400).map(PacketLength::bytes),
        Ok(96)
    );
    assert_eq!(
        PacketLength::from_feedback(400, 405, 400).map(PacketLength::bytes),
        Ok(400)
    );
    assert_eq!(
        PacketLength::from_feedback(0, 405, 400).map(PacketLength::bytes),
        Ok(0)
    );
    assert_eq!(
        PacketLength::from_feedback(405, 405, 400),
        Err(Error::Packet)
    );
    assert_eq!(
        PacketLength::from_feedback(416, 512, 400),
        Err(Error::Packet)
    );
    assert_eq!(PacketLength::from_feedback(96, 95, 400), Err(Error::Packet));
    assert_eq!(
        PacketLength::from_feedback(u32::MAX, u16::MAX, u16::MAX),
        Err(Error::Packet)
    );
}

#[test]
fn sample_encoding_uses_little_endian_with_zero_padding() {
    use motu_m4_core::PcmSample;
    for (word, expected) in [
        (0x1234_5678, [0, 0x56, 0x34, 0x12]),
        (u32::MAX, [0, 255, 255, 255]),
        (0x8000_0000, [0, 0, 0, 128]),
        (255, [0; 4]),
    ] {
        assert_eq!(PcmSample::from_bits(word).to_le_bytes(), expected);
    }
}
