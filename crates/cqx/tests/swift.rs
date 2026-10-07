use std::process::Command;
#[test]
fn cli_scores_swift_skips_invalid_files_and_reads_root_calibration() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/swift");
    let out = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["scan", root.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let p: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(p["scores"]["containment"], 93);
    assert_eq!(p["scores"]["security"], 85);
    assert_eq!(p["skipped_files"][0]["file"], "src/Broken.swift");
    assert_eq!(p["lines"], 2);
    let out = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", root.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("swift:rule"));
}
