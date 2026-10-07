#[test]
fn typescript_full_report_matches_reviewed_typescript_branch() {
    // Captured with #59's scorer at 7ed9add. Only CLI scan timing/metadata and
    // the machine-specific config_path were removed, leaving the full report.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/typescript");
    let vfs = cqx_vfs::from_dir(&root).unwrap();
    let mut facts = Vec::new();
    cqx_analysis::run(&vfs, &mut facts).unwrap();
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(vfs.read("cqx.json")).unwrap();
    let report = cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    );
    assert_eq!(
        serde_json::to_string_pretty(&report).unwrap() + "\n",
        include_str!("../../../fixtures/typescript/report.json")
    );
}

#[test]
fn go_full_report_matches_reviewed_go_branch() {
    // Captured with #60's scorer at a2760e9, before any further frontends.
    // CLI scan metadata, config path and source quotes are normalized away.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/go");
    let vfs = cqx_vfs::from_dir(&root).unwrap();
    let mut facts = Vec::new();
    cqx_analysis::run(&vfs, &mut facts).unwrap();
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(vfs.read("cqx.json")).unwrap();
    let report = cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    );
    assert_eq!(
        serde_json::to_string_pretty(&report).unwrap() + "\n",
        include_str!("../../../fixtures/go/report.json")
    );
}

#[test]
fn python_keeps_its_unicode_tables_without_downgrading_rust() {
    let mut vfs = cqx_vfs::Vfs::new("unicode-fixture");
    vfs.insert(
        "Cargo.toml",
        "[package]\nname=\"unicode_fixture\"\nversion=\"0.1.0\"\n",
    );
    // U+0558 became an identifier-start character in Unicode 18. Rust's
    // existing parser accepts it; Python 3.14 still uses Unicode 17.
    vfs.insert("src/lib.rs", "pub fn ՘() { std::process::exit(1); }");
    vfs.insert("lib.py", "def ՘(): pass");
    let mut facts = Vec::new();
    let stats = cqx_analysis::run(&vfs, &mut facts).unwrap();
    assert_eq!(stats.unparsed.len(), 1, "{:?}", stats.unparsed);
    assert!(stats.unparsed[0].starts_with("lib.py:"));
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(None).unwrap();
    let report = cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    );
    assert_eq!(report["scores"]["containment"], 70);
}

#[test]
fn python_full_report_is_fixed_before_further_frontends() {
    // Captured with this Kotlin frontend before any further frontends.
    // CLI scan metadata, config path and source quotes are normalized away.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/python");
    let vfs = cqx_vfs::from_dir(&root).unwrap();
    let mut facts = Vec::new();
    cqx_analysis::run(&vfs, &mut facts).unwrap();
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(vfs.read("cqx.json")).unwrap();
    let report = cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    );
    assert_eq!(
        serde_json::to_string_pretty(&report).unwrap() + "\n",
        include_str!("../../../fixtures/python/report.json")
    );
}
