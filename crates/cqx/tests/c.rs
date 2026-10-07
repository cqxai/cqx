use std::process::Command;
#[test]
fn cli_scores_c_and_cpp_skips_bad_files_and_reads_root_calibration() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/c-cpp");
    let output = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["scan", root.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scores"]["containment"], 82);
    assert_eq!(report["skipped_files"][0]["file"], "src/broken.cpp");
    for language in ["c", "cpp"] {
        assert_eq!(
            report["rules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["language"] == language && r["rule"] == "exit-in-library")
                .unwrap()["total_findings"],
            1
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", root.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let facts = String::from_utf8(output.stdout).unwrap();
    assert!(facts.contains("c:rule"));
    assert!(facts.contains("cpp:rule"));
}
