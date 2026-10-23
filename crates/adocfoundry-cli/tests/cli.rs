//! Drives the `adocfoundry-cli` binary.

use std::process::Command;

#[test]
fn prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("adocfoundry-cli {}", env!("CARGO_PKG_VERSION"))
    );
}
