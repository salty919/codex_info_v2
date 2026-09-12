use std::process::Command;

#[test]
fn binary_version_is_the_recorder_package_version() {
    let executable = std::env::var_os("CARGO_BIN_EXE_codex_info_recorder")
        .expect("cargo must expose the recorder binary to integration tests");
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .expect("recorder --version must start");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("version output is UTF-8")
            .trim(),
        env!("CARGO_PKG_VERSION")
    );
}
