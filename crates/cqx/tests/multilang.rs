use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mixed")
}

fn report(vfs: &cqx_vfs::Vfs, text: &str) -> serde_json::Value {
    let mut facts = Vec::new();
    cqx_analysis::run(vfs, &mut facts).unwrap();
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(Some(text)).unwrap();
    cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    )
}

#[test]
fn mixed_fixture_weights_independent_category_scores_by_product_lines() {
    let vfs = cqx_vfs::from_dir(&fixture()).unwrap();
    let got = report(&vfs, "{}");
    assert_eq!(got["lines"], 1800);
    assert_eq!(
        got["scores"],
        serde_json::json!({
            "containment": 70, "security": 73, "quality": 100, "legibility": 100, "modularity": 100
        })
    );
    assert_eq!(got["languages"]["rust"]["lines"], 600);
    assert_eq!(got["languages"]["typescript"]["lines"], 1200);
    assert_eq!(got["languages"]["rust"]["scores"]["security"], 80);
    assert_eq!(got["languages"]["typescript"]["scores"]["security"], 70);
    for language in ["rust", "typescript"] {
        let mut isolated = vfs.clone();
        isolated.files.retain(|path, _| {
            if language == "rust" {
                path.starts_with("src-tauri/")
            } else {
                !path.starts_with("src-tauri/")
            }
        });
        assert_eq!(
            got["languages"][language]["scores"],
            report(&isolated, "{}")["scores"]
        );
        assert_eq!(
            got["languages"][language]["scores"]
                .as_object()
                .unwrap()
                .len(),
            5
        );
        assert!(got["languages"][language]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["language"] == language));
    }
}

#[test]
fn weights_exclude_tests_exclusions_and_unparsed_files_and_clamp_before_averaging() {
    let mut vfs = cqx_vfs::from_dir(&fixture()).unwrap();
    vfs.insert("tests/test.ts", "eval(input);\n".repeat(2000));
    vfs.insert("ignored/product.ts", "eval(input);\n".repeat(3000));
    vfs.insert("broken.ts", "const = ;\n".repeat(4000));
    let got = report(
        &vfs,
        r#"{"exclude":["ignored/"],"rules":{"shell-invocation":{"weight":140}}}"#,
    );
    assert_eq!(got["lines"], 1800);
    assert_eq!(got["languages"]["rust"]["scores"]["security"], 0);
    assert_eq!(got["scores"]["security"], 47); // (0 * 600 + 70 * 1200) / 1800.
    assert_eq!(got["skipped_files"][0]["file"], "broken.ts");
    let only_ts = report(
        &vfs,
        r#"{"exclude":["ignored/","src-tauri/"],"rules":{"shell-invocation":{"weight":140}}}"#,
    );
    assert_eq!(only_ts["lines"], 1200);
    assert!(only_ts.get("languages").is_none());
    assert_eq!(only_ts["scores"]["security"], 70);
}

#[test]
fn tiny_language_keeps_density_floor_but_uses_actual_line_weight() {
    let mut vfs = cqx_vfs::Vfs::new("tiny mixed");
    vfs.insert("Cargo.toml", "[package]\nname='tiny'\nversion='0.1.0'\n");
    vfs.insert("src/lib.rs", "pub fn stop() { std::process::exit(1); }\n");
    vfs.insert("src/lib.ts", "eval(input);\n// product\n// product\n");
    let got = report(&vfs, "{}");
    assert_eq!(got["languages"]["rust"]["lines"], 1);
    assert_eq!(got["languages"]["typescript"]["lines"], 3);
    assert_eq!(got["languages"]["typescript"]["scores"]["security"], 70);
    assert_eq!(got["scores"]["containment"], 93); // 92.5 rounds to 93.
    assert_eq!(got["scores"]["security"], 78); // 77.5 rounds to 78.
    vfs.insert("src/lib.ts", "// no findings\n");
    vfs.insert("tests/test.ts", "eval(input);\n");
    let got = report(&vfs, r#"{"exclude":["src/lib.ts"]}"#);
    assert!(got.get("languages").is_none());
    assert_eq!(got["scores"]["containment"], 70);
}

#[test]
fn scan_and_score_enforce_language_floors_and_numeric_headline_override() {
    let scratch = fixture()
        .join("../../.tmp")
        .join(format!("multilang-gate-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let config = scratch.join("cqx.json");
    let facts = scratch.join("facts.ndjson");
    let extract = Command::new(env!("CARGO_BIN_EXE_cqx"))
        .args(["extract", fixture().to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(
        extract.status.success(),
        "{}",
        String::from_utf8_lossy(&extract.stderr)
    );
    std::fs::write(&facts, extract.stdout).unwrap();
    for (floor, expected, security_only) in [
        (r#"{"rust":70,"typescript":70}"#, true, false),
        (r#"{"rust":81,"typescript":70}"#, false, false),
        (r#"{"rust":70,"typescript":71}"#, false, false),
        (r#"{"java":100}"#, true, false), // No Java product code, so no Java gate.
        ("70", true, false),
        ("71", false, false),
        // A Rust floor above the headline can pass, while a TS floor below
        // the headline can fail. These distinguish named gates from headline gates.
        (r#"{"rust":80,"typescript":70}"#, true, true),
        (r#"{"typescript":71}"#, false, true),
        ("71", true, true),
        ("74", false, true),
    ] {
        let mut document = serde_json::json!({"min_score": serde_json::from_str::<serde_json::Value>(floor).unwrap()});
        if security_only {
            document["rules"] = serde_json::json!({
                "exit-in-library": {"enabled": false},
                "typescript/exit-in-library": {"enabled": false},
            });
        }
        std::fs::write(&config, document.to_string()).unwrap();
        for command in ["scan", "score"] {
            let mut args = vec![command.to_string()];
            if command == "scan" {
                args.push(fixture().to_str().unwrap().into());
            } else {
                args.extend(["--facts".into(), facts.to_str().unwrap().into()]);
            }
            args.extend([
                "--config".into(),
                config.to_str().unwrap().into(),
                "--json".into(),
            ]);
            let out = Command::new(env!("CARGO_BIN_EXE_cqx"))
                .args(&args)
                .output()
                .unwrap();
            assert_eq!(
                out.status.success(),
                expected,
                "{command} {floor}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(
                data["config"]["min_score"],
                serde_json::from_str::<serde_json::Value>(floor).unwrap()
            );
            if !expected && floor.starts_with('{') {
                assert!(String::from_utf8_lossy(&out.stderr).contains("/security scored"));
            }
            let override_out = Command::new(env!("CARGO_BIN_EXE_cqx"))
                .args(&args)
                .args(["--min-score", "70"])
                .output()
                .unwrap();
            assert!(
                override_out.status.success(),
                "{}",
                String::from_utf8_lossy(&override_out.stderr)
            );
        }
    }
    // Single-language JSON has no breakdown block, but its named floor still gates.
    for (floor, expected) in [(70, true), (71, false)] {
        std::fs::write(&config, format!("{{\"min_score\":{{\"rust\":{floor}}}}}")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_cqx"))
            .args([
                "scan",
                fixture().join("src-tauri").to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
                "--json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            out.status.success(),
            expected,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(data.get("languages").is_none());
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
