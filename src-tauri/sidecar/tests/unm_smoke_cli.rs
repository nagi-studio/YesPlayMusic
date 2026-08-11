use std::process::Command;

#[test]
fn unm_smoke_cli_runs_production_executor_behavior() {
    let output = Command::new(env!("CARGO_BIN_EXE_yesplaymusic-sidecar"))
        .arg("--unm-smoke-test")
        .output()
        .expect("UNM smoke CLI should start");

    assert!(
        output.status.success(),
        "UNM smoke CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UNM smoke output should be UTF-8");
    assert!(
        stdout.contains("production executor search->retrieve passed"),
        "UNM smoke did not execute production search/retrieve: {stdout}"
    );
}
