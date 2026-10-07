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
fn zig_full_report_is_fixed_before_further_frontends() {
    // Captured with this Zig frontend before any further frontends.
    // CLI scan metadata, config path and source quotes are normalized away.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/zig");
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
        include_str!("../../../fixtures/zig/report.json")
    );
}
