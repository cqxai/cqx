use std::process::Command;

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/c-cpp")
}
#[test]
fn cli_scores_c_and_cpp_skips_bad_files_and_reads_root_calibration() {
    let root = fixture();
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
    assert_eq!(report["scores"]["containment"], 92); // (93 * 4 + 89 * 2) / 6.
    assert_eq!(report["languages"]["c"]["lines"], 4);
    assert_eq!(report["languages"]["cpp"]["lines"], 2);
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
}

#[test]
fn cli_extracts_exact_c_and_cpp_rule_counts_and_roles() {
    let root = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", root.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(output.stdout).unwrap());
    let mut rules = std::collections::BTreeMap::new();
    for edge in &stream.edges {
        for language in ["c", "cpp"] {
            if let Some(rule) = edge
                .attrs
                .get(&format!("{language}:rule"))
                .and_then(serde_json::Value::as_str)
            {
                let role = edge.attrs["role"].as_str().unwrap();
                *rules
                    .entry((language.to_string(), rule.to_string(), role.to_string()))
                    .or_insert(0) += 1;
            }
        }
    }
    assert_eq!(
        rules,
        std::collections::BTreeMap::from([
            (("c".into(), "exit-in-library".into(), "product".into()), 1),
            (("c".into(), "exit-in-library".into(), "test".into()), 1),
            (
                (
                    "c".into(),
                    "shell-argument-unchecked".into(),
                    "product".into()
                ),
                1
            ),
            (
                ("cpp".into(), "exit-in-library".into(), "product".into()),
                1
            ),
        ])
    );
}
