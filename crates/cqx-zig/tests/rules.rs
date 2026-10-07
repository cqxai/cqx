use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (p, s) in files {
        vfs.insert(*p, *s);
    }
    let mut facts = Vec::new();
    cqx_zig::run(&vfs, &mut facts, &|| {}).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let c = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&c, &Metrics::compute(&stream, &c))
}
fn count(code: &str, rule: &str) -> u64 {
    let p = report(&[("lib.zig", code)], "{}");
    assert!(p["skipped_files"].is_null(), "{p}");
    findings(&p, rule)
}
fn findings(p: &serde_json::Value, rule: &str) -> u64 {
    p["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "zig" && r["rule"] == rule)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
#[test]
fn exits_fire_but_public_main_build_scripts_tests_and_custom_std_stay_quiet() {
    assert_eq!(
        count(
            r#"const std=@import("std");pub fn stop() void {std.process.exit(1);}"#,
            "exit-in-library"
        ),
        1
    );
    for code in [
        r#"const std=@import("std");pub fn main() void {std.process.exit(1);}"#,
        r#"const std=@import("custom");pub fn stop() void {std.process.exit(1);}"#,
        r#"const std=@import("std");pub fn stop(std:Other) void {std.process.exit(1);}"#,
        r#"const std=@import("std");test "stop" {std.process.exit(1);}"#,
    ] {
        assert_eq!(count(code, "exit-in-library"), 0, "{code}")
    }
    assert_eq!(
        findings(
            &report(
                &[(
                    "build.zig",
                    r#"const std=@import("std");pub fn build(b:*Builder) void {std.process.exit(1);}"#
                )],
                "{}"
            ),
            "exit-in-library"
        ),
        0
    );
}
#[test]
fn catch_discards_fire_but_propagation_handling_and_explained_empty_blocks_stay_quiet() {
    assert_eq!(
        count(
            "pub fn f() void {work() catch unreachable;work() catch {};}",
            "swallowed-errors"
        ),
        2
    );
    for code in [
        "pub fn f() !void {try work();}",
        "pub fn f() void {work() catch |err| {log(err);};}",
        "pub fn f() void {work() catch { // Optional file may be absent.\n};}",
    ] {
        assert_eq!(count(code, "swallowed-errors"), 0, "{code}")
    }
}
#[test]
fn dynamic_child_argv_fires_but_literal_executable_with_dynamic_arguments_stays_quiet() {
    assert_eq!(
        count(
            r#"const std=@import("std");pub fn f(input:[]const u8) void {var p=std.process.Child.init(&.{input},allocator);}"#,
            "nonliteral-process"
        ),
        1
    );
    for code in [
        r#"const std=@import("std");pub fn f(input:[]const u8) void {var p=std.process.Child.init(&.{"git",input},allocator);}"#,
        r#"const std=@import("custom");pub fn f(input:[]const u8) void {var p=std.process.Child.init(&.{input},allocator);}"#,
    ] {
        assert_eq!(count(code, "nonliteral-process"), 0, "{code}")
    }
}
#[test]
fn exclusions_parse_failures_and_embedded_tests_preserve_product_lines() {
    for path in [
        "zig-cache/lib.zig",
        ".zig-cache/lib.zig",
        "zig-out/lib.zig",
        "tests/lib.zig",
        "lib_test.zig",
    ] {
        let p = report(
            &[
                (
                    path,
                    r#"const std=@import("std");pub fn f() void {std.process.exit(1);}"#,
                ),
                ("quiet.zig", "pub fn quiet() void {}"),
            ],
            "{}",
        );
        assert_eq!(findings(&p, "exit-in-library"), 0);
        assert_eq!(p["lines"], 1);
    }
    let p = report(
        &[
            ("broken.zig", "pub fn broken( {"),
            (
                "lib.zig",
                r#"const std=@import("std");pub fn f() void {std.process.exit(1);}"#,
            ),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "exit-in-library"), 1);
    assert_eq!(p["skipped_files"][0]["file"], "broken.zig");
    let p = report(
        &[(
            "lib.zig",
            "pub fn f() void {}\ntest \"work\" {\nwork() catch unreachable;\n}\n",
        )],
        "{}",
    );
    assert_eq!(p["lines"], 1);
    assert_eq!(findings(&p, "swallowed-errors"), 0);
}
#[test]
fn duplicate_and_size_rules_fire_with_config_and_quiet_counterexamples() {
    let body = "consume(1);".repeat(30);
    let code = format!("pub fn a() void {{{body}}}\npub fn b() void {{{body}}}");
    assert_eq!(count(&code, "duplicated-bodies"), 1);
    assert_eq!(
        count(
            &code.replacen("consume(1)", "consume(2)", 1),
            "duplicated-bodies"
        ),
        0
    );
    assert_eq!(
        count(
            "pub fn a() void {wrap();}\npub fn b() void {wrap();}",
            "duplicated-bodies"
        ),
        0
    );
    let cfg = r#"{"rules":{"zig/oversized-files":{"params":{"max_lines":2}},"zig/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let p = report(&[("lib.zig", "pub fn f() void {\nwork();\n}")], cfg);
    assert_eq!(findings(&p, "oversized-files"), 1);
    assert!(p["scores"]["modularity"].as_u64().unwrap() < 100);
    assert_eq!(
        findings(
            &report(&[("lib.zig", "pub fn f() void {\n}")], cfg),
            "oversized-files"
        ),
        0
    );
}
