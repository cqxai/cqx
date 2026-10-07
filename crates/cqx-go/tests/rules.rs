use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;

fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let got = report_allowing_skips(files, config);
    assert!(
        got.get("skipped_files").is_none(),
        "unexpected skipped source: {got}"
    );
    got
}

fn report_allowing_skips(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    vfs.insert("go.mod", "module example.com/library\ngo 1.23\n");
    for (path, source) in files {
        vfs.insert(*path, *source);
    }
    let mut out = Vec::new();
    cqx_go::run(&vfs, &mut out).expect("valid Go source");
    let stream = Stream::from_ndjson(&String::from_utf8(out).unwrap());
    let config = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&config, &Metrics::compute(&stream, &config))
}
fn findings(report: &serde_json::Value, id: &str) -> u64 {
    report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "go" && r["rule"] == id)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
fn count(source: &str, id: &str) -> u64 {
    findings(&report(&[("internal/lib.go", source)], "{}"), id)
}

#[test]
fn exit_in_library_fires_but_main_and_shadowed_imports_stay_quiet() {
    let bad = "package library\nimport (o \"os\"; l \"log\")\nfunc Stop() { o.Exit(1); l.Fatal(\"failed\") }";
    let got = report(&[("internal/lib.go", bad)], "{}");
    assert_eq!(findings(&got, "exit-in-library"), 2);
    assert!(got["scores"]["containment"].as_u64().unwrap() < 100);
    assert_eq!(
        count(
            &bad.replace("package library", "package main"),
            "exit-in-library"
        ),
        0
    );
    assert_eq!(
        count(
            "package library\nimport \"os\"\nfunc f(os interface{Exit(int)}) { os.Exit(1) }",
            "exit-in-library"
        ),
        0
    );
    assert_eq!(
        count(
            "package library\nimport \"example.com/os\"\nfunc f() { os.Exit(1) }",
            "exit-in-library"
        ),
        0
    );
    // A cmd/ path alone does not turn a library into package main.
    assert_eq!(
        findings(
            &report(&[("cmd/helper/lib.go", bad)], "{}"),
            "exit-in-library"
        ),
        2
    );
}

#[test]
fn shell_invocation_fires_but_direct_exec_and_local_command_stay_quiet() {
    assert_eq!(
        count(
            "package p\nimport e \"os/exec\"\nfunc f() { e.Command(\"sh\", \"-c\", \"echo hi\") }",
            "shell-invocation"
        ),
        1
    );
    assert_eq!(
        count(
            "package p\nimport \"os/exec\"\nfunc f() { exec.Command(\"git\", \"status\"); exec.Command(\"sh\", \"script.sh\") }",
            "shell-invocation"
        ),
        0
    );
    assert_eq!(
        count(
            "package p\nimport \"os/exec\"\nfunc f(exec interface{Command(...string)}) { exec.Command(\"sh\", \"-c\", \"x\") }",
            "shell-invocation"
        ),
        0
    );
}

#[test]
fn shell_argument_unchecked_fires_but_literal_arguments_stay_quiet() {
    assert_eq!(
        count(
            "package p\nimport \"os/exec\"\nfunc f(command string) { exec.CommandContext(ctx, \"/bin/bash\", \"-c\", command) }",
            "shell-argument-unchecked"
        ),
        1
    );
    assert_eq!(
        count(
            "package p\nimport \"os/exec\"\nfunc f(arg string) { const script = \"echo hi\"; exec.Command(\"sh\", \"-c\", script); exec.Command(\"git\", arg) }",
            "shell-argument-unchecked"
        ),
        0
    );
}

#[test]
fn env_controlled_spawn_tracks_values_but_not_child_arguments_or_reassigned_values() {
    assert_eq!(
        count(
            "package p\nimport (\"os\"; \"os/exec\";)\nfunc f() { program := os.Getenv(\"EDITOR\"); alias := program; exec.Command(alias) }",
            "env-controlled-spawn"
        ),
        1
    );
    assert_eq!(
        count(
            "package p\nimport (\"os\"; \"os/exec\";)\nfunc f() { exec.Command(\"echo\", os.Getenv(\"HOME\")); program := os.Getenv(\"EDITOR\"); program = \"vim\"; exec.Command(program) }",
            "env-controlled-spawn"
        ),
        0
    );
}

#[test]
fn discarded_check_fires_only_for_known_error_slots_and_empty_typed_error_branches() {
    let source = "package p\nfunc check() error { return nil }\nfunc pair() (int, error) { return 0, nil }\nfunc f() { _ = check(); pair(); n, _ := pair(); _ = n; if err := check(); err != nil {} }";
    let got = report(&[("lib.go", source)], "{}");
    assert_eq!(findings(&got, "discarded-check"), 4);
    assert!(got["scores"]["security"].as_u64().unwrap() < 100);
}

#[test]
fn discarded_check_allows_discarding_a_non_error_value() {
    assert_eq!(
        count(
            "package p\nfunc value() int { return 1 }\nfunc f() { _ = value() }",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn discarded_check_allows_handled_error_and_discarded_non_error_slot() {
    assert_eq!(
        count(
            "package p\nfunc pair() (int, error) { return 0, nil }\nfunc f() error { _, err := pair(); if err != nil { return err }; return nil }",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn discarded_check_allows_explained_empty_error_branch() {
    assert_eq!(
        count(
            "package p\nfunc check() error { return nil }\nfunc f() { if err := check(); err != nil { /* best effort; unavailable is expected */ } }",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn discarded_check_allows_deferred_close() {
    assert_eq!(
        count(
            "package p\nimport \"os\"\nfunc cleanup(f *os.File) {\n defer f.Close()\n}",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn discarded_check_allows_deferred_known_error_function() {
    assert_eq!(
        count(
            "package p\nfunc check() error { return nil }\nfunc f() {\n defer check()\n}",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn discarded_check_does_not_guess_unknown_function_results() {
    assert_eq!(
        count("package p\nfunc f() { unknown() }", "discarded-check"),
        0
    );
}

#[test]
fn discarded_check_respects_shadowed_function_results() {
    assert_eq!(
        count(
            "package p\nfunc check() error { return nil }\nfunc f(check func() int) { _ = check() }",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn discarded_check_does_not_treat_non_error_nil_checks_as_errors() {
    assert_eq!(
        count(
            "package p\nfunc f(err *int) { if err != nil {} }",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn log_fatal_in_a_library_fires_directly() {
    assert_eq!(
        count(
            "package library\nimport \"log\"\nfunc Stop() { log.Fatal(\"failed\") }",
            "exit-in-library"
        ),
        1
    );
}

#[test]
fn package_main_allows_log_fatal_and_os_exit_at_any_path() {
    let source = "package main\nimport (\"log\"; \"os\";)\nfunc main() { log.Fatal(\"failed\"); os.Exit(1) }";
    for path in ["main.go", "cmd/app/main.go", "internal/entry/main.go"] {
        assert_eq!(
            findings(&report(&[(path, source)], "{}"), "exit-in-library"),
            0,
            "{path}"
        );
    }
}

#[test]
fn non_main_package_under_cmd_is_still_a_library() {
    let source = "package library\nimport (\"log\"; \"os\";)\nfunc Stop() { log.Fatal(\"failed\"); os.Exit(1) }";
    assert_eq!(
        findings(
            &report(&[("cmd/helper/lib.go", source)], "{}"),
            "exit-in-library"
        ),
        2
    );
}

#[test]
fn undocumented_suppressions_fire_but_reasons_and_strings_stay_quiet() {
    assert_eq!(
        count(
            "package p\n//nolint:errcheck\nfunc f() {}\n//nolint\nvar x = 1",
            "undocumented-suppressions"
        ),
        2
    );
    assert_eq!(
        count(
            "package p\n//nolint:errcheck // best effort cleanup\nfunc f() {}\nvar x = \"//nolint\"",
            "undocumented-suppressions"
        ),
        0
    );
}

#[test]
fn hand_built_json_fires_but_serialization_and_ordinary_formatting_stay_quiet() {
    assert_eq!(
        count(
            "package p\nimport \"fmt\"\nfunc f(name string) { fmt.Sprintf(`{\"name\":\"%s\"}`, name) }",
            "hand-built-json"
        ),
        1
    );
    assert_eq!(
        count(
            "package p\nimport (\"fmt\"; \"encoding/json\";)\nfunc f(name string) { json.Marshal(name); fmt.Sprintf(\"hello %s\", name); fmt.Sprintf(`{\"fixed\":true}`); fmt.Sprintf(`{ echo \"%s\"; local x=1; }`, name); fmt.Println(name) }",
            "hand-built-json"
        ),
        0
    );
}

#[test]
fn oversized_rules_use_repository_thresholds_and_stay_quiet_below_them() {
    let config = r#"{"rules":{"go/oversized-files":{"params":{"max_lines":2}},"go/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let bad = report(&[("lib.go", "package p\nvar a = 1\nvar b = 2")], config);
    assert_eq!(findings(&bad, "oversized-files"), 1);
    assert!(bad["scores"]["modularity"].as_u64().unwrap() < 100);
    assert!(
        bad["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["language"] == "go"
                && r["rule"] == "oversized-line-share"
                && r["deducted"].as_f64().unwrap_or(0.0) > 0.0)
    );
    let good = report(&[("lib.go", "package p\nvar a = 1")], config);
    assert_eq!(findings(&good, "oversized-files"), 0);
    assert_eq!(good["scores"]["modularity"], 100);
}

#[test]
fn duplicated_bodies_ignore_comments_spacing_small_wrappers_and_different_literals() {
    let body = "consume(\"a b\");".repeat(20);
    let source = format!(
        "package p\nfunc a() {{ {body} }}\nfunc b() {{ {} }}",
        body.replace(';', "\n// comment\n")
    );
    let got = report(&[("lib.go", &source)], "{}");
    assert_eq!(findings(&got, "duplicated-bodies"), 1);
    assert!(got["scores"]["quality"].as_u64().unwrap() < 100);
    assert_eq!(
        count(
            "package p\nfunc a() { shared() }; func b() { shared() }",
            "duplicated-bodies"
        ),
        0
    );
    assert_eq!(
        count(
            &format!(
                "package p\nfunc a() {{ {body} }}\nfunc b() {{ {} }}",
                body.replace("a b", "ab")
            ),
            "duplicated-bodies"
        ),
        0
    );
}

#[test]
fn generics_and_unicode_locations() {
    let source = "package p\nimport \"os\"\ntype Set[T comparable] map[T]struct{}\nfunc Map[T ~int | ~int64, U any](items []T, fn func(T) U) []U { var out []U; for _, item := range items { out = append(out, fn(item)) }; return out }\nfunc naïve() { _ = Map[int, string](nil, nil); os.Exit(1) }";
    let got = report(&[("lib.go", source)], "{}");
    assert_eq!(findings(&got, "exit-in-library"), 1);
    let finding = got["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "go" && r["rule"] == "exit-in-library")
        .unwrap();
    assert_eq!(finding["findings"][0]["line"], 5);
}

#[test]
fn tests_build_tags_testdata_and_config_exclusions_remove_findings_and_denominators() {
    let source = "package p\nimport \"os\"\nfunc f() { os.Exit(1) }";
    let got = report(
        &[
            ("internal/lib.go", "package p"),
            ("lib_test.go", source),
            ("testdata/tool.go", source),
            (
                "generate.go",
                "//go:build ignore\n\npackage p\nimport \"os\"\nfunc f() { os.Exit(1) }",
            ),
            ("vendor/lib.go", source),
        ],
        r#"{"exclude":["vendor/"]}"#,
    );
    assert_eq!(got["lines"], 1);
    assert_eq!(findings(&got, "exit-in-library"), 0);
}

#[test]
fn generated_protobuf_header_removes_findings_and_denominator() {
    let source = include_str!("../../../fixtures/go/generated/message.pb.go");
    let got = report(
        &[("lib.go", "package p"), ("generated/message.pb.go", source)],
        "{}",
    );
    assert_eq!(got["lines"], 1);
    assert_eq!(findings(&got, "exit-in-library"), 0);
    let crlf = source.replace('\n', "\r\n");
    assert_eq!(count(&crlf, "exit-in-library"), 0);
    // The header, rather than the extension, decides whether code is generated.
    let handwritten = source.replace("// Code generated by protoc-gen-go. DO NOT EDIT.\n", "");
    assert_eq!(
        findings(
            &report(&[("message.pb.go", &handwritten)], "{}"),
            "exit-in-library"
        ),
        1
    );
    assert_eq!(
        findings(
            &report(&[("generated.go", source)], "{}"),
            "exit-in-library"
        ),
        0
    );
}

#[test]
fn excluded_go_files_retain_their_graph_and_no_neutral_score_effects() {
    let mut vfs = Vfs::new("generated-graph");
    vfs.insert("lib.go", "package p");
    let source = include_str!("../../../fixtures/go/generated/message.pb.go");
    vfs.insert("message.pb.go", source);
    vfs.insert(
        "vendor/dependency/lib.go",
        source.replace("// Code generated by protoc-gen-go. DO NOT EDIT.\n", ""),
    );
    let mut out = Vec::new();
    let stats = cqx_go::run(&vfs, &mut out).unwrap();
    assert_eq!(stats.files, 3);
    let stream = Stream::from_ndjson(&String::from_utf8(out).unwrap());
    for path in ["message.pb.go", "vendor/dependency/lib.go"] {
        assert!(
            stream
                .nodes
                .iter()
                .any(|n| n.id == cqx_schema::Id::file(path) && n.attrs["role"] == "test")
        );
        assert!(
            stream
                .edges
                .iter()
                .any(|e| e.from == cqx_schema::Id::file(path)
                    && e.kind == cqx_schema::EdgeKind::Contains)
        );
    }
    let config = Config::from_text(Some(r#"{"rules":{"go/oversized-files":{"params":{"max_lines":2}},"go/oversized-line-share":{"params":{"max_lines":2}}}}"#)).unwrap();
    let got = cqx_score::report_json(&config, &Metrics::compute(&stream, &config));
    assert_eq!(got["lines"], 1);
    assert_eq!(findings(&got, "oversized-files"), 0);
    assert_eq!(findings(&got, "duplicated-bodies"), 0);
    assert!(
        got["scores"]
            .as_object()
            .unwrap()
            .values()
            .all(|s| s == 100)
    );
}

#[test]
fn progress_counts_skipped_files_and_output_errors_still_fail() {
    let mut vfs = Vfs::new("progress");
    vfs.insert("bad.go", "package p\nfunc = ;");
    vfs.insert("good.go", "package p\nfunc f() {}");
    let ticks = std::cell::Cell::new(0);
    let prepared =
        cqx_go::prepare(&vfs, cqx_go::modules(&vfs), &|| ticks.set(ticks.get() + 1)).unwrap();
    assert_eq!(ticks.get(), 2);
    let stats = prepared.emit(&vfs, Vec::new()).unwrap();
    assert_eq!(stats.files, 1);
    assert_eq!(stats.unparsed.len(), 1);
    struct FailingWriter;
    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        prepared.emit(&vfs, FailingWriter).err().unwrap().kind(),
        std::io::ErrorKind::BrokenPipe
    );
}

#[test]
fn generated_marker_must_be_an_exact_header_line_before_package() {
    for marker in [
        "// Code generated x DO NOT EDIT.",
        "// Code generated  DO NOT EDIT.",
    ] {
        let source =
            format!("{marker}\npackage p\nimport \"log\"\nfunc f() {{ log.Fatal(\"x\") }}");
        assert_eq!(count(&source, "exit-in-library"), 0, "{marker}");
    }
    for source in [
        " // Code generated x DO NOT EDIT.\npackage p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }",
        "// Code generated x DO NOT EDIT\npackage p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }",
        "// Code generated x DO NOT EDIT. extra\npackage p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }",
        "/*\n// Code generated x DO NOT EDIT.\n*/\npackage p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }",
        "package p\n// Code generated x DO NOT EDIT.\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }",
        "package\n// Code generated x DO NOT EDIT.\np\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }",
    ] {
        assert_eq!(count(source, "exit-in-library"), 1, "{source}");
    }
}

#[test]
fn vendor_is_excluded_by_default_including_nested_modules() {
    let source = "package p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }";
    let got = report(
        &[
            ("lib.go", "package p"),
            ("vendor/thirdparty/lib.go", source),
            ("nested/vendor/thirdparty/lib.go", source),
        ],
        "{}",
    );
    assert_eq!(got["lines"], 1);
    assert_eq!(findings(&got, "exit-in-library"), 0);
    assert_eq!(
        findings(
            &report(&[("vendored/lib.go", source)], "{}"),
            "exit-in-library"
        ),
        1
    );
}

#[test]
fn tool_and_tools_packages_are_product_code_without_exclusive_build_tags() {
    let source = "package tool\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }";
    for path in ["tool/lib.go", "tools/lib.go", "internal/tool/lib.go"] {
        let got = report(&[(path, source)], "{}");
        assert_eq!(got["lines"], 3, "{path}");
        assert_eq!(findings(&got, "exit-in-library"), 1, "{path}");
    }
}

#[test]
fn exclusive_tools_and_ignore_build_tags_exclude_files_at_any_path() {
    for tag in [
        "//go:build tools",
        "//go:build ignore",
        "// +build tools",
        "// +build ignore",
    ] {
        let source =
            format!("{tag}\n\npackage tool\nimport \"log\"\nfunc f() {{ log.Fatal(\"x\") }}");
        for path in ["tool/lib.go", "ordinary/lib.go"] {
            let got = report(&[("lib.go", "package p"), (path, &source)], "{}");
            assert_eq!(got["lines"], 1, "{tag}: {path}");
            assert_eq!(findings(&got, "exit-in-library"), 0, "{tag}: {path}");
        }
    }
    let source =
        "//go:build linux || tools\n\npackage p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }";
    assert_eq!(count(source, "exit-in-library"), 1);
}

#[test]
fn skipped_syntax_and_token_errors_do_not_abort_or_dilute_good_go_files() {
    let source = "package p\nimport \"log\"\nfunc f() { log.Fatal(\"x\") }";
    let clean = report(&[("lib.go", source)], "{}");
    let got = report_allowing_skips(
        &[
            ("lib.go", source),
            ("bad.go", "package p\nfunc = ;"),
            ("bad-token.go", "package p\nvar s = \"unterminated"),
        ],
        "{}",
    );
    assert_eq!(got["scores"], clean["scores"]);
    assert_eq!(got["rules"], clean["rules"]);
    assert_eq!(got["lines"], clean["lines"]);
    let skipped = got["skipped_files"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0]["file"], "bad-token.go");
    assert_eq!(skipped[1]["file"], "bad.go");
    assert!(
        skipped
            .iter()
            .all(|s| !s["reason"].as_str().unwrap().is_empty())
    );
}

#[test]
fn signatures_and_module_identity_are_real_graph_facts() {
    let mut vfs = Vfs::new("types");
    vfs.insert("go.mod", "module example.com/lib");
    vfs.insert(
        "internal/lib.go",
        "package lib\nfunc F(x string) error { return nil }",
    );
    vfs.insert("generic.go", "package lib\ntype Box[T any] struct { value T }\nfunc (b *Box[T]) Get() T { return b.value }\nfunc Transform[T any](x map[string][]*T) map[string][]*T { return x }");
    let mut out = Vec::new();
    cqx_go::run(&vfs, &mut out).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(out).unwrap());
    assert!(
        stream
            .edges
            .iter()
            .any(|e| e.kind == cqx_schema::EdgeKind::Param && e.to.0 == "type:go:string")
    );
    assert!(
        stream
            .edges
            .iter()
            .any(|e| e.kind == cqx_schema::EdgeKind::Returns && e.to.0 == "type:go:error")
    );
    assert!(
        stream
            .nodes
            .iter()
            .any(|n| n.id.0 == "pkg:go:example.com/lib:internal:lib")
    );
    assert!(
        stream
            .nodes
            .iter()
            .any(|n| n.id.0 == "sym:go:generic.go::*Box[T].Get")
    );
    assert!(
        stream
            .edges
            .iter()
            .any(|e| e.kind == cqx_schema::EdgeKind::Param && e.to.0 == "type:go:map[string][]*T")
    );
}

#[test]
fn custom_error_types_are_not_builtin_errors() {
    assert_eq!(
        count(
            "package p\ntype error int\nfunc result() error { return 0 }\nfunc f(err *error) { _ = result(); if err != nil {} }",
            "discarded-check"
        ),
        0
    );
}

#[test]
fn generic_error_results_and_variable_declarations_are_checked() {
    assert_eq!(
        count(
            "package p\nfunc check[T any]() error { return nil }\nfunc f() { var _ = check[int](); var err = check[int](); if err != nil {} }",
            "discarded-check"
        ),
        2
    );
    assert_eq!(
        count(
            "package p\nimport \"os/exec\"\nfunc f() { exec.Command(\"sh\", \"-c\", \"echo \" + \"hi\") }",
            "shell-argument-unchecked"
        ),
        0
    );
}
