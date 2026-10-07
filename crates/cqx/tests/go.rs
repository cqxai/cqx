use std::process::Command;

#[test]
fn cli_scan_and_extract_score_go_with_root_configuration() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/go");
    let scan = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["scan", fixture.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert_eq!(report["scores"].as_object().unwrap().len(), 5);
    assert_eq!(report["scores"]["security"], 93);
    assert_eq!(report["scores"]["containment"], 70);
    assert_eq!(report["config"]["min_score"], 60);
    let extract = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", fixture.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(extract.status.success());
    let facts = String::from_utf8(extract.stdout).unwrap();
    assert!(facts.contains("go:rule"));
    assert!(facts.contains("example.com/fixture"));
}
