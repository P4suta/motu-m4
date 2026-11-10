use std::fs;
use std::process::Command;

#[test]
fn instruction_cli_reports_stops_and_refuses_invalid_memory_and_arguments() {
    let directory =
        std::env::temp_dir().join(format!("motu-instruction-cli-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let mut image = vec![0; 64];
    image[4..8].copy_from_slice(b"test");
    image.extend_from_slice(&0x20001000_u32.to_le_bytes());
    image.extend_from_slice(&0x20000009_u32.to_le_bytes());
    image.extend_from_slice(&[0x40, 0x18, 0x70, 0x47]);
    image[..4].copy_from_slice(&12_u32.to_le_bytes());
    fs::write(directory.join("test.image"), &image).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_motu-m4"))
            .current_dir(&directory)
            .args(args)
            .output()
            .unwrap()
    };
    let mut args = vec![
        "execute-firmware",
        "--input",
        "test.image",
        "--ram-base",
        "0x20000000",
        "--ram-bytes",
        "4096",
        "--entry",
        "0x20000009",
        "--args",
        "5",
        "7",
        "0",
        "0",
        "--instructions",
        "2",
        "--read",
        "0x20000008",
        "--read-bytes",
        "4",
    ];
    let result = run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary = String::from_utf8_lossy(&result.stdout);
    assert!(summary.contains("stop=Returned, instructions=2"));
    assert!(summary.contains("r0=0x0000000c"));
    assert!(summary.contains("memory[0x20000008]: 40 18 70 47"));
    assert!(summary.contains("physical-usb-writes=0"));
    args[6] = "8";
    assert_eq!(run(&args).status.code(), Some(1));
    args[6] = "4096";
    args[8] = "0x20000008";
    assert_eq!(run(&args).status.code(), Some(1));
    args[8] = "0x20000009";
    args[16] = "0";
    assert_eq!(run(&args).status.code(), Some(2));
    assert_eq!(fs::read(directory.join("test.image")).unwrap(), image);
    for code in [[0xfe, 0xe7, 0, 0], [0x30, 0xbf, 0, 0], [0, 0x68, 0, 0]] {
        image[72..].copy_from_slice(&code);
        fs::write(directory.join("test.image"), &image).unwrap();
        let result = run(&[
            "execute-firmware",
            "--input",
            "test.image",
            "--ram-base",
            "536870912",
            "--ram-bytes",
            "4096",
            "--instructions",
            "5",
        ]);
        let summary = String::from_utf8_lossy(&result.stdout);
        if code[1] == 0xe7 {
            assert!(result.status.success());
            assert!(summary.contains("stop=Budget, instructions=5"));
        }
        if code[1] == 0xbf {
            assert!(result.status.success());
            assert!(summary.contains("stop=WaitForInterrupt, instructions=1"));
        }
        if code[1] == 0x68 {
            assert_eq!(result.status.code(), Some(1));
            assert!(summary.contains("Fault(MemRead"));
            assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported operation"));
        }
    }
    for options in [vec!["--profile", "recorded"], vec!["--scenario", "steady"]] {
        let mut args = vec![
            "simulate",
            "capture",
            "--firmware",
            "test.image",
            "--output",
            "out.raw",
        ];
        args.extend(options);
        assert_eq!(run(&args).status.code(), Some(2));
    }
    assert_eq!(
        run(&[
            "simulate",
            "capture",
            "--firmware",
            "test.image",
            "--output",
            "out.raw"
        ])
        .status
        .code(),
        Some(1)
    );
    assert!(!directory.join("out.raw").exists());
    fs::remove_dir_all(directory).unwrap();
}
