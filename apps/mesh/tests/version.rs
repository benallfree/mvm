//! `mesh --version` prints the package version (implementation plan 0.3a).

use std::process::Command;

#[test]
fn prints_its_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_mesh"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("mesh {}\n", env!("CARGO_PKG_VERSION"))
    );
}
