use std::process::Command;

#[test]
fn cli_scan_and_extract_score_typescript_without_a_cargo_manifest() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/typescript");
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
    assert_eq!(report["skipped_files"][0]["file"], "broken.ts");
    assert_eq!(report["scores"].as_object().unwrap().len(), 5);
    assert_eq!(report["scores"]["security"], 83);
    assert_eq!(report["config"]["min_score"], 70);
    assert!(report["scores"]["quality"].as_u64().unwrap() < 100);
    assert_eq!(report["scores"]["containment"], 100);
    let human = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["scan", fixture.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(String::from_utf8(human.stdout)
        .unwrap()
        .contains("broken.ts:"));
    let extract = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", fixture.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(extract.status.success());
    assert!(String::from_utf8(extract.stdout)
        .unwrap()
        .contains("typescript:rule"));
}

fn snapshot_report(vfs: &cqx_vfs::Vfs, config: &str) -> serde_json::Value {
    let mut facts = Vec::new();
    cqx_analysis::run(vfs, &mut facts).unwrap();
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    )
}

#[test]
fn review_invalid_typescript_keeps_mixed_rust_scores_and_reports_each_file() {
    let mut vfs = cqx_vfs::Vfs::new("mixed");
    vfs.insert("Cargo.toml", "[package]\nname='mixed'\nversion='0.1.0'\n");
    vfs.insert("src/lib.rs", "pub fn shutdown() { std::process::exit(1); }");
    vfs.insert("src/good.ts", "eval(input);");
    let clean = snapshot_report(&vfs, "{}");
    vfs.insert("bad.ts", "const = ;");
    vfs.insert("redeclaration.ts", "let value; let value;");
    let got = snapshot_report(&vfs, "{}");
    assert_eq!(got["scores"], clean["scores"]);
    assert_eq!(got["scores"]["containment"], 85);
    assert_eq!(got["languages"]["rust"]["scores"]["containment"], 70);
    assert_eq!(got["scores"]["security"], 85);
    assert_eq!(got["languages"]["typescript"]["scores"]["security"], 70);
    assert_eq!(got["lines"], 2);
    let skipped = got["skipped_files"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0]["file"], "bad.ts");
    assert_eq!(skipped[1]["file"], "redeclaration.ts");
    assert!(skipped
        .iter()
        .all(|s| !s["reason"].as_str().unwrap().is_empty()));
}
