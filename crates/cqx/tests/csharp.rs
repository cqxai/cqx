use std::process::Command;

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/csharp")
}
#[test]
fn cli_scores_csharp_skips_invalid_files_and_reads_root_calibration() {
    let root = fixture();
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
    assert_eq!(p["skipped_files"][0]["file"], "src/Broken.cs");
    assert_eq!(p["lines"], 1);
}

#[test]
fn cli_extracts_exact_csharp_rule_counts_and_roles() {
    let root = fixture();
    let out = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", root.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(out.stdout).unwrap());
    let mut rules = std::collections::BTreeMap::new();
    for edge in &stream.edges {
        if let Some(rule) = edge
            .attrs
            .get("csharp:rule")
            .and_then(serde_json::Value::as_str)
        {
            let role = edge.attrs["role"].as_str().unwrap();
            *rules
                .entry(("csharp".to_string(), rule.to_string(), role.to_string()))
                .or_insert(0) += 1;
        }
    }
    assert_eq!(
        rules,
        std::collections::BTreeMap::from([
            (
                ("csharp".into(), "exit-in-library".into(), "product".into()),
                1
            ),
            (
                ("csharp".into(), "exit-in-library".into(), "test".into()),
                1
            ),
            (
                (
                    "csharp".into(),
                    "nonliteral-process".into(),
                    "product".into()
                ),
                1
            ),
        ])
    );
}
