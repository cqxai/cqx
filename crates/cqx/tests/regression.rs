#[test]
fn java_keeps_its_unicode_tables_without_downgrading_rust() {
    let mut vfs = cqx_vfs::Vfs::new("unicode-fixture");
    vfs.insert(
        "Cargo.toml",
        "[package]\nname=\"unicode_fixture\"\nversion=\"0.1.0\"\n",
    );
    // U+0558 became an identifier-start character in Unicode 18. Rust's
    // existing parser accepts it; Java SE 26 still uses Unicode 17.
    vfs.insert("src/lib.rs", "pub fn ՘() { std::process::exit(1); }");
    vfs.insert("Library.java", "class ՘ {}");
    let mut facts = Vec::new();
    let stats = cqx_analysis::run(&vfs, &mut facts).unwrap();
    assert_eq!(stats.unparsed.len(), 1, "{:?}", stats.unparsed);
    assert!(stats.unparsed[0].starts_with("Library.java:"));
    let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let config = cqx_score::config::Config::from_text(None).unwrap();
    let report = cqx_score::report_json(
        &config,
        &cqx_score::metrics::Metrics::compute(&stream, &config),
    );
    assert_eq!(report["scores"]["containment"], 70);
}

#[test]
fn all_language_goldens_are_byte_identical() {
    // Existing main after #62: Rust, TypeScript and Go, verified with its scorer.
    // Six new frontends own their fixtures. No frontend may rewrite a sibling.
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    for language in [
        "rust-compat",
        "typescript",
        "go",
        "java",
        "kotlin",
        "swift",
        "zig",
        "python",
        "php",
    ] {
        let root = fixtures.join(language);
        let vfs = cqx_vfs::from_dir(&root).unwrap();
        let mut facts = Vec::new();
        cqx_analysis::run(&vfs, &mut facts).unwrap();
        let stream = cqx_store::facts::Stream::from_ndjson(&String::from_utf8(facts).unwrap());
        let text = if language == "rust-compat" {
            Some(r#"{"exclude":["src/vendor/"]}"#)
        } else {
            vfs.read("cqx.json")
        };
        let config = cqx_score::config::Config::from_text(text).unwrap();
        let report = cqx_score::report_json(
            &config,
            &cqx_score::metrics::Metrics::compute(&stream, &config),
        );
        let bytes = serde_json::to_string_pretty(&report).unwrap() + "\n";
        assert_eq!(
            bytes,
            std::fs::read_to_string(root.join("report.json")).unwrap(),
            "{language}"
        );
    }
}
