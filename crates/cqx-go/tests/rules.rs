use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;

fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
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
    assert_eq!(count("package p\nimport \"os/exec\"\nfunc f() { exec.Command(\"git\", \"status\"); exec.Command(\"sh\", \"script.sh\") }", "shell-invocation"), 0);
    assert_eq!(count("package p\nimport \"os/exec\"\nfunc f(exec interface{Command(...string)}) { exec.Command(\"sh\", \"-c\", \"x\") }", "shell-invocation"), 0);
}

#[test]
fn shell_argument_unchecked_fires_but_literal_arguments_stay_quiet() {
    assert_eq!(count("package p\nimport \"os/exec\"\nfunc f(command string) { exec.CommandContext(ctx, \"/bin/bash\", \"-c\", command) }", "shell-argument-unchecked"), 1);
    assert_eq!(count("package p\nimport \"os/exec\"\nfunc f(arg string) { const script = \"echo hi\"; exec.Command(\"sh\", \"-c\", script); exec.Command(\"git\", arg) }", "shell-argument-unchecked"), 0);
}

#[test]
fn env_controlled_spawn_tracks_values_but_not_child_arguments_or_reassigned_values() {
    assert_eq!(count("package p\nimport (\"os\"; \"os/exec\";)\nfunc f() { program := os.Getenv(\"EDITOR\"); alias := program; exec.Command(alias) }", "env-controlled-spawn"), 1);
    assert_eq!(count("package p\nimport (\"os\"; \"os/exec\";)\nfunc f() { exec.Command(\"echo\", os.Getenv(\"HOME\")); program := os.Getenv(\"EDITOR\"); program = \"vim\"; exec.Command(program) }", "env-controlled-spawn"), 0);
}

#[test]
fn discarded_check_fires_only_for_known_error_slots_and_empty_typed_error_branches() {
    let source = "package p\nfunc check() error { return nil }\nfunc pair() (int, error) { return 0, nil }\nfunc f() { _ = check(); pair(); n, _ := pair(); _ = n; if err := check(); err != nil {} }";
    let got = report(&[("lib.go", source)], "{}");
    assert_eq!(findings(&got, "discarded-check"), 4);
    assert!(got["scores"]["security"].as_u64().unwrap() < 100);
    assert_eq!(count("package p\nfunc check() error { return nil }\nfunc pair() (int, error) { return 0, nil }\nfunc value() int { return 1 }\nfunc f() error { _ = value(); _, err := pair(); if err != nil { return err }; if err := check(); err != nil { /* best effort; unavailable is expected */ }; unknown(); defer check(); return nil }", "discarded-check"), 0);
    assert_eq!(count("package p\nfunc check() error { return nil }\nfunc f(check func() int, err *int) { _ = check(); if err != nil {} }", "discarded-check"), 0);
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
    assert_eq!(count("package p\n//nolint:errcheck // best effort cleanup\nfunc f() {}\nvar x = \"//nolint\"", "undocumented-suppressions"), 0);
}

#[test]
fn hand_built_json_fires_but_serialization_and_ordinary_formatting_stay_quiet() {
    assert_eq!(count("package p\nimport \"fmt\"\nfunc f(name string) { fmt.Sprintf(`{\"name\":\"%s\"}`, name) }", "hand-built-json"), 1);
    assert_eq!(count("package p\nimport (\"fmt\"; \"encoding/json\";)\nfunc f(name string) { json.Marshal(name); fmt.Sprintf(\"hello %s\", name); fmt.Sprintf(`{\"fixed\":true}`); fmt.Sprintf(`{ echo \"%s\"; local x=1; }`, name); fmt.Println(name) }", "hand-built-json"), 0);
}

#[test]
fn oversized_rules_use_repository_thresholds_and_stay_quiet_below_them() {
    let config = r#"{"rules":{"go/oversized-files":{"params":{"max_lines":2}},"go/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let bad = report(&[("lib.go", "package p\nvar a = 1\nvar b = 2")], config);
    assert_eq!(findings(&bad, "oversized-files"), 1);
    assert!(bad["scores"]["modularity"].as_u64().unwrap() < 100);
    assert!(bad["rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["language"] == "go"
            && r["rule"] == "oversized-line-share"
            && r["deducted"].as_f64().unwrap_or(0.0) > 0.0));
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
fn generics_unicode_and_invalid_syntax() {
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
    let mut vfs = Vfs::new("broken");
    vfs.insert("bad.go", "package p\nfunc = ;");
    assert!(cqx_go::run(&vfs, Vec::new()).is_err());
}

#[test]
fn tests_tools_and_excluded_paths_remove_findings_and_denominators() {
    let source = "package p\nimport \"os\"\nfunc f() { os.Exit(1) }";
    let got = report(
        &[
            ("internal/lib.go", "package p"),
            ("lib_test.go", source),
            ("tools/tool.go", source),
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
    assert!(stream
        .edges
        .iter()
        .any(|e| e.kind == cqx_schema::EdgeKind::Param && e.to.0 == "type:go:string"));
    assert!(stream
        .edges
        .iter()
        .any(|e| e.kind == cqx_schema::EdgeKind::Returns && e.to.0 == "type:go:error"));
    assert!(stream
        .nodes
        .iter()
        .any(|n| n.id.0 == "pkg:go:example.com/lib:internal:lib"));
    assert!(stream
        .nodes
        .iter()
        .any(|n| n.id.0 == "sym:go:generic.go::*Box[T].Get"));
    assert!(stream
        .edges
        .iter()
        .any(|e| e.kind == cqx_schema::EdgeKind::Param && e.to.0 == "type:go:map[string][]*T"));
}

#[test]
fn custom_error_types_are_not_builtin_errors() {
    assert_eq!(count("package p\ntype error int\nfunc result() error { return 0 }\nfunc f(err *error) { _ = result(); if err != nil {} }", "discarded-check"), 0);
}

#[test]
fn generic_error_results_and_variable_declarations_are_checked() {
    assert_eq!(count("package p\nfunc check[T any]() error { return nil }\nfunc f() { var _ = check[int](); var err = check[int](); if err != nil {} }", "discarded-check"), 2);
    assert_eq!(count("package p\nimport \"os/exec\"\nfunc f() { exec.Command(\"sh\", \"-c\", \"echo \" + \"hi\") }", "shell-argument-unchecked"), 0);
}
