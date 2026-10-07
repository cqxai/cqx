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

#[test]
fn go_parser_skips_preserve_all_three_languages_scores_and_report_each_file() {
    let mut vfs = cqx_vfs::Vfs::new("mixed");
    vfs.insert("Cargo.toml", "[package]\nname='mixed'\nversion='0.1.0'\n");
    vfs.insert("src/lib.rs", "pub fn shutdown() { std::process::exit(1); }");
    vfs.insert("library.ts", "eval(input);");
    vfs.insert(
        "lib.go",
        "package library\nimport \"log\"\nfunc Stop() { log.Fatal(\"failed\") }",
    );
    let report = |vfs: &cqx_vfs::Vfs| {
        let mut facts = Vec::new();
        let stats = cqx_analysis::run(vfs, &mut facts).unwrap();
        let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
        let config = cqx_score::config::Config::from_text(None).unwrap();
        (
            cqx_score::report_json(
                &config,
                &cqx_score::metrics::Metrics::compute(&stream, &config),
            ),
            stats,
        )
    };
    let (clean, _) = report(&vfs);
    vfs.insert("bad.go", "package p\nfunc = ;");
    vfs.insert("bad-token.go", "package p\nvar s = \"unterminated");
    let (got, stats) = report(&vfs);
    assert_eq!(got["scores"], clean["scores"]);
    assert_eq!(got["rules"], clean["rules"]);
    assert_eq!(got["lines"], clean["lines"]);
    assert_eq!(got["skipped_files"][0]["file"], "bad-token.go");
    assert_eq!(got["skipped_files"][1]["file"], "bad.go");
    assert_eq!(stats.unparsed.len(), 2);
    assert!(
        stats
            .unparsed
            .iter()
            .any(|reason| reason.starts_with("bad.go: "))
    );
    for language in ["rust", "typescript", "go"] {
        assert!(
            got["rules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["language"] == language && r["total_findings"].as_u64().unwrap() > 0)
        );
    }
}

#[test]
fn cli_reports_go_parser_skips_in_json_human_output_and_extract_summary() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/go-skips");
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
    assert_eq!(report["skipped_files"][0]["file"], "bad-token.go");
    assert_eq!(report["skipped_files"][1]["file"], "bad.go");
    assert_eq!(report["scores"]["containment"], 70);
    assert_eq!(report["scores"]["security"], 70);
    let human = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["scan", fixture.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("bad.go:"));
    assert!(human.contains("bad-token.go:"));
    let extract = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", fixture.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(extract.status.success());
    let summary = String::from_utf8(extract.stderr).unwrap();
    assert!(summary.contains("bad.go:"));
    assert!(summary.contains("bad-token.go:"));
    assert!(
        String::from_utf8(extract.stdout)
            .unwrap()
            .contains("go:rule")
    );
}
