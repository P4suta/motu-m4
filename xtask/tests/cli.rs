use std::fs;
use std::process::Command;

#[test]
fn firmware_command_requires_explicit_archives_and_refuses_changed_bytes() {
    let executable = env!("CARGO_BIN_EXE_xtask");
    assert!(Command::new(executable)
        .arg("--help")
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        Command::new(executable)
            .arg("firmware")
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
    let directory = std::env::temp_dir().join(format!("motu-firmware-task-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let archive = directory.join("changed.zip");
    fs::write(&archive, b"changed archive").unwrap();
    let destination = directory.join("descriptors");
    let result = Command::new(executable)
        .args(["firmware", "--mac-archive"])
        .arg(&archive)
        .arg("--windows-archive")
        .arg(&archive)
        .arg("--export-descriptors")
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("unexpected artifact size"));
    assert!(!destination.exists());
    fs::remove_dir_all(directory).unwrap();
}
