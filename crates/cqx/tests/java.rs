use std::process::Command;
#[test]
fn cli_scores_java_skips_invalid_files_and_reads_root_calibration() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/java");
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
    assert_eq!(p["skipped_files"][0]["file"], "src/main/java/Broken.java");
    assert_eq!(p["lines"], 1);
    let out = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", root.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout).unwrap().contains("java:rule"));
}

#[test]
fn review_cli_scores_build_package_and_testimonial_service() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/java-review");
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
    let rule = p["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "java" && r["rule"] == "exit-in-library")
        .unwrap();
    assert_eq!(rule["total_findings"], 2);
    assert_eq!(p["lines"], 2);
    assert!(rule["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["file"] == "module/src/main/java/com/acme/build/Library.java"));
}
