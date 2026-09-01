use std::process::Command;

#[test]
fn release_resolver_rollout_guard_executes_missing_linux_authority_case() {
    let output = Command::new("python3")
        .args(["scripts/workflow_quality_gate.py", "--release-self-test"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("python3 must execute the release resolver fixture gate");
    assert!(
        output.status.success(),
        "release resolver fixture gate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("case=missing-linux-authority PASS"),
        "missing Linux authority regression case was not executed: {stdout}"
    );
}
