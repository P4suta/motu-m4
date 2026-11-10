use crate::{run_stream, AudioTransport, FirmwareM4, FrameCount, SampleRate, StreamRequest};
use motu_m4_firmware::{ControlError, FirmwareAudioControl, FirmwareFamily};
use std::fs;
use std::path::PathBuf;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../.firmware-corpus/{name}.image"))
}

#[test]
fn firmware_identity_refuses_synthetic_images_before_cpu_execution() {
    for bytes in [
        vec![],
        vec![0; 168_596],
        vec![0; 353_356],
        vec![0; 1_048_577],
    ] {
        assert!(matches!(
            FirmwareAudioControl::new(&bytes),
            Err(ControlError::Identity)
        ));
        assert!(FirmwareM4::new(&bytes).is_err());
    }
}

#[test]
#[ignore = "Requires verified images exported with just firmware-corpus"]
fn official_control_handlers_generate_independent_protocol_responses() {
    for (name, family) in [
        ("atmel", FirmwareFamily::Atmel),
        ("nxp", FirmwareFamily::Nxp),
    ] {
        let firmware = fs::read(corpus(name)).expect("run just firmware-corpus MAC WINDOWS");
        let mut control = FirmwareAudioControl::new(&firmware).unwrap();
        assert_eq!(control.family(), family);
        let ranges =
            include_bytes!("../../../motu-m4-core/tests/fixtures/m-series-clock-ranges.bin");
        for length in [0_u16, 1, 2, 3, 63, 64, 65, 73, 74, 75, 255, 256, 1024] {
            let mut bytes = vec![0xff; length as usize];
            let [low, high] = length.to_le_bytes();
            let count = control
                .control([0xa1, 2, 0, 1, 0, 2, low, high], &mut bytes)
                .unwrap();
            assert_eq!(count, usize::from(length).min(74), "{name} {length}");
            assert_eq!(&bytes[..count], &ranges[..count], "{name} {length}");
            assert!(bytes[count..].iter().all(|byte| *byte == 0xff));
            assert!(control.packets().iter().all(|packet| packet.len() <= 64));
            assert_eq!(control.packets().concat(), ranges[..count]);
        }
        for rate in [44_100_u32, 48_000, 88_200, 96_000, 176_400, 192_000] {
            let mut bytes = rate.to_le_bytes();
            assert_eq!(
                control
                    .control([0x21, 1, 0, 1, 0, 2, 4, 0], &mut bytes)
                    .unwrap(),
                4
            );
            bytes.fill(0);
            assert_eq!(
                control
                    .control([0xa1, 1, 0, 1, 0, 2, 4, 0], &mut bytes)
                    .unwrap(),
                4
            );
            assert_eq!(bytes, rate.to_le_bytes());
            let mut valid = [0];
            assert_eq!(
                control
                    .control([0xa1, 1, 0, 2, 0, 2, 1, 0], &mut valid)
                    .unwrap(),
                1
            );
            assert_eq!(valid, [1]);
        }
        let mut selector = [0];
        assert_eq!(
            control
                .control([0xa1, 1, 0, 1, 0, 1, 1, 0], &mut selector)
                .unwrap(),
            1
        );
        assert_eq!(selector, [1]);
        let mut maximum_out = [0; 64];
        maximum_out[..4].copy_from_slice(&48_000_u32.to_le_bytes());
        assert_eq!(
            control
                .control([0x21, 1, 0, 1, 0, 2, 64, 0], &mut maximum_out)
                .unwrap(),
            64
        );
        let before = control.instructions();
        for (setup, mut data) in [
            ([0x21, 1, 0, 1, 0, 2, 65, 0], vec![0; 65]),
            ([0xa1, 2, 0, 1, 0, 2, 2, 0], vec![0; 1]),
            ([0xa1, 2, 0, 1, 0, 2, 1, 4], vec![0; 1025]),
            ([0x80, 1, 0, 0, 0, 0, 1, 0], vec![0; 1]),
        ] {
            assert!(matches!(
                control.control(setup, &mut data),
                Err(ControlError::Request)
            ));
        }
        assert_eq!(control.instructions(), before);
        assert!(before > 100);
        let mut changed = firmware.clone();
        changed[100] ^= 1;
        assert!(matches!(
            FirmwareAudioControl::new(&changed),
            Err(ControlError::Identity)
        ));
    }
}

#[test]
#[ignore = "Requires verified images exported with just firmware-corpus"]
fn official_firmware_controls_support_all_stream_modes_and_recovery() {
    let long_audio = vec![0; 307 * 16];
    let audio: Vec<u8> = (0_u32..76)
        .flat_map(|sample| (sample << 8).to_le_bytes())
        .collect();
    for name in ["atmel", "nxp"] {
        let firmware = fs::read(corpus(name)).expect("run just firmware-corpus MAC WINDOWS");
        for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            for mode in 0..3 {
                let request = match mode {
                    0 => StreamRequest::Capture(FrameCount::new(19).unwrap()),
                    1 => StreamRequest::Playback(&audio),
                    _ => StreamRequest::Duplex(&audio),
                };
                let mut device = FirmwareM4::new(&firmware).unwrap();
                let expected: &[u8] = match name {
                    "atmel" => include_bytes!(
                        "../../../motu-m4-core/tests/fixtures/m4-atmel-firmware-descriptors.bin"
                    ),
                    _ => include_bytes!(
                        "../../../motu-m4-core/tests/fixtures/m4-nxp-firmware-descriptors.bin"
                    ),
                };
                assert_eq!(device.descriptors(), expected);
                for _ in 0..2 {
                    let result =
                        run_stream(&mut device, SampleRate::new(hz).unwrap(), request, |_| {
                            false
                        })
                        .unwrap();
                    if mode != 1 {
                        assert_eq!(result.audio, audio);
                    } else {
                        assert!(result.audio.is_empty());
                    }
                    assert_eq!(result.report.frames, 19);
                    assert!(device.simulated().is_attached());
                    assert_eq!(device.simulated().clock(), 48_000);
                    assert!(device.firmware_failure().is_none());
                    assert!(device.control_instructions() > 100);
                    if mode != 0 {
                        assert_eq!(&device.simulated().played()[..audio.len()], audio);
                        assert!(device.simulated().played()[audio.len()..]
                            .iter()
                            .all(|byte| *byte == 0));
                    }
                }
                for limit in [0, 1] {
                    let request = match mode {
                        0 => StreamRequest::Capture(FrameCount::new(307).unwrap()),
                        1 => StreamRequest::Playback(&long_audio),
                        _ => StreamRequest::Duplex(&long_audio),
                    };
                    let result = run_stream(
                        &mut device,
                        SampleRate::new(hz).unwrap(),
                        request,
                        |packets| packets >= limit,
                    );
                    assert!(result.is_err());
                    assert!(device.simulated().is_attached());
                    assert_eq!(device.simulated().clock(), 48_000);
                }
            }
        }
    }
}
