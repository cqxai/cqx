use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (p, s) in files {
        vfs.insert(*p, *s);
    }
    let mut out = Vec::new();
    cqx_c::run(&vfs, &mut out, &|| {}).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(out).unwrap());
    let config = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&config, &Metrics::compute(&stream, &config))
}
fn findings(report: &serde_json::Value, lang: &str, rule: &str) -> u64 {
    report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == lang && r["rule"] == rule)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
fn count(lang: &str, code: &str, rule: &str) -> u64 {
    let path = if lang == "c" {
        "src/lib.c"
    } else {
        "src/lib.cpp"
    };
    let got = report(&[(path, code)], "{}");
    assert!(got["skipped_files"].is_null(), "{got}");
    findings(&got, lang, rule)
}
#[test]
fn exits_fire_in_libraries_but_main_shadowed_and_member_apis_stay_quiet() {
    for lang in ["c", "cpp"] {
        let bad = report(
            &[(
                if lang == "c" { "lib.c" } else { "lib.cpp" },
                "#include <stdlib.h>\nvoid stop(void) { exit(1); abort(); }",
            )],
            "{}",
        );
        assert_eq!(findings(&bad, lang, "exit-in-library"), 2);
        assert!(bad["scores"]["containment"].as_u64().unwrap() < 100);
        for code in [
            "#include <stdlib.h>\nint main(void) { exit(1); }",
            "void exit(int n) {} void f(void) { exit(1); }",
            "void f(void) { const char *s = \"exit(1)\"; }",
        ] {
            assert_eq!(count(lang, code, "exit-in-library"), 0);
        }
    }
    assert_eq!(
        count(
            "cpp",
            "#include <cstdlib>\nvoid f() { std::exit(1); }",
            "exit-in-library"
        ),
        1
    );
    assert_eq!(
        count(
            "cpp",
            "struct P { void exit(int); }; void f(P p) { p.exit(1); }",
            "exit-in-library"
        ),
        0
    );
}
#[test]
fn nonliteral_shell_commands_fire_but_literal_commands_and_project_apis_stay_quiet() {
    for lang in ["c", "cpp"] {
        assert_eq!(count(lang, "#include <stdlib.h>\n#include <stdio.h>\nvoid f(char *input) { system(input); popen(input, \"r\"); }", "shell-argument-unchecked"), 2);
        for code in ["#include <stdlib.h>\n#include <stdio.h>\nvoid f(void) { system(\"echo ok\"); popen(\"ls\", \"r\"); }", "void system(char *s) {} void f(char *input) { system(input); }", "void f(void) { const char *s = \"system(input)\"; }"] {
            assert_eq!(count(lang, code, "shell-argument-unchecked"), 0);
        }
    }
}
#[test]
fn unsafe_buffer_apis_fire_but_bounded_calls_and_member_methods_stay_quiet() {
    for lang in ["c", "cpp"] {
        assert_eq!(count(lang, "#include <stdio.h>\n#include <string.h>\nvoid f(char *b, char *s) { sprintf(b, \"%s\", s); strcpy(b, s); strcat(b, s); gets(b); }", "unsafe-buffer-calls"), 4);
        assert_eq!(count(lang, "#include <stdio.h>\n#include <string.h>\nvoid f(char *b, char *s) { snprintf(b, 32, \"%s\", s); memcpy(b, s, 4); }", "unsafe-buffer-calls"), 0);
    }
    assert_eq!(
        count(
            "cpp",
            "struct P { void strcpy(); }; void f(P p) { p.strcpy(); }",
            "unsafe-buffer-calls"
        ),
        0
    );
}
#[test]
fn locally_declared_must_check_returns_fire_only_when_unused() {
    for lang in ["c", "cpp"] {
        assert_eq!(count(lang, "__attribute__((warn_unused_result)) int checked(void); void f(void) { checked(); }", "discarded-check"), 1);
        for code in ["__attribute__((warn_unused_result)) int checked(void); int f(void) { return checked(); }", "__attribute__((warn_unused_result)) int checked(void); void f(void) { if (checked()) {} }", "int value(void); void f(void) { value(); }", "__attribute__((warn_unused_result)) int checked(void); void f(int (*checked)(void)) { checked(); }"] {
            assert_eq!(count(lang, code, "discarded-check"), 0);
        }
    }
    assert_eq!(
        count(
            "cpp",
            "[[nodiscard]] int checked(); void f() { checked(); }",
            "discarded-check"
        ),
        1
    );
    assert_eq!(
        count(
            "cpp",
            "[[nodiscard]] int checked(); void f() { (void)checked(); }",
            "discarded-check"
        ),
        0
    );
}
#[test]
fn undocumented_suppressions_fire_but_explanations_and_ordinary_comments_stay_quiet() {
    for lang in ["c", "cpp"] {
        assert_eq!(
            count(
                lang,
                "// NOLINT\nvoid f(void) {}\n// NOLINTNEXTLINE\nint value;\n",
                "undocumented-suppressions"
            ),
            2
        );
        assert_eq!(count(lang, "// NOLINT(readability-identifier-naming) -- external C ABI names\nvoid f(void) {}\n// Compatibility with a vendor header requires disabling this warning.\n#pragma clang diagnostic ignored \"-Wconversion\"\n", "undocumented-suppressions"), 0);
        assert_eq!(
            count(
                lang,
                "void f(void) { const char *s = \"NOLINT\"; }",
                "undocumented-suppressions"
            ),
            0
        );
    }
}
#[test]
fn json_templates_fire_but_ordinary_formats_and_shell_braces_stay_quiet() {
    for lang in ["c", "cpp"] {
        assert_eq!(
            count(
                lang,
                r#"#include <stdio.h>
void f(char *b, char *v) { snprintf(b, 100, "{\"name\":\"%s\"}", v); }"#,
                "hand-built-json"
            ),
            1
        );
        assert_eq!(
            count(
                lang,
                r#"#include <stdio.h>
void f(char *b, char *v) { snprintf(b, 100, "name: %s", v); snprintf(b, 100, "{ shell %s; }", v); }"#,
                "hand-built-json"
            ),
            0
        );
    }
}
#[test]
fn excludes_tests_generated_and_parse_skips_keep_product_denominators() {
    for path in [
        "third_party/lib.c",
        "deps/lib.c",
        "build/lib.c",
        "cmake-build-debug/lib.c",
        "src/lib.generated.c",
    ] {
        let got = report(
            &[
                (path, "#include <stdlib.h>\nvoid f(void) { exit(1); }"),
                ("lib.c", "int ok;"),
            ],
            "{}",
        );
        assert_eq!(findings(&got, "c", "exit-in-library"), 0, "{path}");
        assert_eq!(got["lines"], 1, "{path}");
    }
    let got = report(
        &[
            (
                "tests/lib.c",
                "#include <stdlib.h>\nvoid f(void) { exit(1); }",
            ),
            ("generated.c", "// @generated\n#error not parsed"),
            ("lib.c", "int ok;"),
            ("broken.c", "void f( { ;"),
        ],
        "{}",
    );
    assert_eq!(got["lines"], 1);
    assert_eq!(findings(&got, "c", "exit-in-library"), 0);
    assert_eq!(got["skipped_files"][0]["file"], "broken.c");
}
#[test]
fn duplicate_bodies_and_large_files_share_existing_calibration() {
    for lang in ["c", "cpp"] {
        let path = if lang == "c" { "lib.c" } else { "lib.cpp" };
        let body = "consume(1);".repeat(30);
        let source = format!("void a(void) {{ {body} }}\nvoid b(void) {{ {body} }}");
        assert_eq!(
            findings(&report(&[(path, &source)], "{}"), lang, "duplicated-bodies"),
            1
        );
        let different = source.replacen("consume(1)", "consume(2)", 1);
        assert_eq!(
            findings(
                &report(&[(path, &different)], "{}"),
                lang,
                "duplicated-bodies"
            ),
            0
        );
        assert_eq!(
            count(
                lang,
                "void a(void) { wrapper(); } void b(void) { wrapper(); }",
                "duplicated-bodies"
            ),
            0
        );
        let config = format!(
            r#"{{"rules":{{"{lang}/oversized-files":{{"params":{{"max_lines":2}}}},"{lang}/oversized-line-share":{{"params":{{"max_lines":2}}}}}}}}"#
        );
        let got = report(&[(path, "int a;\nint b;\nint c;\n")], &config);
        assert_eq!(findings(&got, lang, "oversized-files"), 1);
        assert!(got["scores"]["modularity"].as_u64().unwrap() < 100);
        assert_eq!(
            findings(
                &report(&[(path, "int a;\nint b;")], &config),
                lang,
                "oversized-files"
            ),
            0
        );
    }
}
#[test]
fn cpp_headers_parse_cpp_syntax_when_a_cpp_translation_unit_is_present() {
    let got = report(
        &[
            (
                "include/lib.h",
                "namespace lib { template <class T> T twice(T x) { return x + x; } }",
            ),
            ("lib.cpp", "int value;"),
        ],
        "{}",
    );
    assert!(got["skipped_files"].is_null());
    assert_eq!(got["lines"], 2);
}

#[test]
fn embedded_redis_tests_do_not_fire_or_dilute_product_lines() {
    let got = report(&[("lib.c", "#include <stdlib.h>\nint value;\n#ifdef REDIS_TEST\nvoid test(void) { exit(1); }\n#endif\n")], "{}");
    assert_eq!(findings(&got, "c", "exit-in-library"), 0);
    assert_eq!(got["lines"], 2);
    let got = report(&[("lib.c", "#include <stdlib.h>\n#if defined(UNIT_TEST)\nvoid test(void) { exit(1); }\n#else\nvoid stop(void) { exit(1); }\n#endif\n")], "{}");
    assert_eq!(findings(&got, "c", "exit-in-library"), 1);
}

#[test]
fn unicode_findings_use_character_columns() {
    let code = "#include <stdlib.h>\nvoid f(void) { const char *s = \"é\"; exit(1); }";
    let got = report(&[("lib.c", code)], "{}");
    let rule = got["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "c" && r["rule"] == "exit-in-library")
        .unwrap();
    let expected = code
        .lines()
        .nth(1)
        .unwrap()
        .split("exit")
        .next()
        .unwrap()
        .chars()
        .count() as u64;
    assert_eq!(rule["findings"][0]["col"][0], expected);
}

#[test]
fn ordinary_comments_and_nolint_block_ends_stay_quiet() {
    assert_eq!(count("cpp", "// This describes how NOLINT suppressions work.\n// NOLINTBEGIN(readability-magic-numbers) -- external protocol constants\nint value = 17;\n// NOLINTEND(readability-magic-numbers)\n", "undocumented-suppressions"), 0);
}

#[test]
fn upstream_macro_excerpts_score_at_least_95_percent_and_keep_exact_findings() {
    for (path, source, language) in [
        (
            "lapi.c",
            include_str!("../../../fixtures/c-recovery/lapi.c"),
            "c",
        ),
        (
            "cJSON.c",
            include_str!("../../../fixtures/c-recovery/cJSON.c"),
            "c",
        ),
        (
            "tinyxml2.hpp",
            include_str!("../../../fixtures/c-recovery/tinyxml2.hpp"),
            "cpp",
        ),
    ] {
        let got = report(&[(path, source)], "{}");
        assert!(
            got["lines"].as_u64().unwrap() * 100 >= source.lines().count() as u64 * 95,
            "{path}: {got}"
        );
        assert_eq!(got["skipped"], 0, "{path}");
        assert_eq!(got["coverage"][language]["partial"], false);
        if path == "cJSON.c" {
            let rule = got["rules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["rule"] == "unsafe-buffer-calls")
                .unwrap();
            assert_eq!(rule["total_findings"], 1);
            assert_eq!(rule["findings"][0]["file"], path);
            assert_eq!(rule["findings"][0]["line"], 6);
            assert_eq!(rule["findings"][0]["col"][0], 4);
        }
    }
}

#[test]
fn recovery_keeps_valid_calls_and_drops_error_subtrees_and_incomplete_body_hashes() {
    let code = "#include <stdio.h>\nvoid f(void) {\n  @@@ sprintf(b, \"bad\");\n  sprintf(b, \"good\");\n}\n";
    let got = report(&[("lib.c", code)], "{}");
    assert_eq!(got["skipped"], 0, "{got}");
    assert_eq!(got["recovered"], 1, "{got}");
    assert_eq!(got["recovered_files"][0]["regions"], 1);
    // The parser recovers the call after @@@ separately: that complete call is
    // still safe to inspect. The malformed tokens themselves yield no facts.
    assert_eq!(findings(&got, "c", "unsafe-buffer-calls"), 2);
    let body = format!(
        "void f(void) {{\n @@@\n {}\n}}\nvoid g(void) {{\n @@@\n {}\n}}",
        "consume(1);".repeat(50),
        "consume(1);".repeat(50)
    );
    assert_eq!(
        findings(&report(&[("lib.c", &body)], "{}"), "c", "duplicated-bodies"),
        0
    );
}

#[test]
fn partial_coverage_includes_wholly_skipped_languages_and_respects_exclusions() {
    let got = report(
        &[
            ("good.c", "int ok;\n"),
            ("bad.c", "@\n".repeat(20).as_str()),
            ("third_party/bad.c", "@\n"),
        ],
        "{}",
    );
    assert_eq!(got["coverage"]["c"]["total_lines"], 21);
    assert_eq!(got["coverage"]["c"]["scored_lines"], 1);
    assert_eq!(got["coverage"]["c"]["partial"], true);
    assert_eq!(got["partial"], true);
    assert_eq!(got["skipped"], 1);
    let all_bad = report(&[("bad.cpp", "@\n@\n")], "{}");
    assert_eq!(all_bad["coverage"]["cpp"]["scored_lines"], 0);
    assert_eq!(all_bad["partial"], true);
    let excluded = report(
        &[("good.c", "int ok;\n"), ("ignored.c", "@\n")],
        r#"{"exclude":["ignored.c"]}"#,
    );
    assert_eq!(excluded["coverage"]["c"]["total_lines"], 1);
    assert_eq!(excluded["partial"], false);
}

#[test]
fn declaration_decorations_linkage_and_conditionals_keep_lines_and_calls() {
    for lang in ["c", "cpp"] {
        let code = "#include <stdio.h>\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nEXPORT_API int f(void) { char b[10]; sprintf(b, \"ok\"); return 0; }\n#ifdef __cplusplus\n}\n#endif\n";
        let got = report(
            &[(if lang == "c" { "lib.c" } else { "lib.cpp" }, code)],
            "{}",
        );
        assert_eq!(findings(&got, lang, "unsafe-buffer-calls"), 1, "{got}");
        assert_eq!(got["lines"], 8);
        assert_eq!(got["recovered"], 0);
        let rule = got["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["language"] == lang && r["rule"] == "unsafe-buffer-calls")
            .unwrap();
        assert_eq!(rule["findings"][0]["line"], 5);
        assert_eq!(rule["findings"][0]["col"][0], 37);
        let conditional = "#include <stdio.h>\n#if defined(EXPORT)\nEXPORT_API\n#endif\nvoid f(void) { char b[10]; sprintf(b, \"ok\"); }\n";
        let got = report(
            &[(if lang == "c" { "lib.c" } else { "lib.cpp" }, conditional)],
            "{}",
        );
        assert_eq!(got["lines"], 5);
        assert_eq!(got["recovered"], 0);
        assert_eq!(findings(&got, lang, "unsafe-buffer-calls"), 1);
    }
    assert_eq!(
        count(
            "c",
            "__declspec(dllexport) int f(void) { return 0; }",
            "unsafe-buffer-calls"
        ),
        0
    );
    assert_eq!(
        count(
            "cpp",
            "__attribute__((visibility(\"default\"))) int f() { return 0; }",
            "unsafe-buffer-calls"
        ),
        0
    );
}

#[test]
fn normalization_leaves_raw_strings_comments_definitions_and_expression_calls_intact() {
    let code = r###"#include <stdio.h>
#define LUA_API __declspec(dllexport)
// LUA_API int ignored(void) { sprintf(b, "fake"); }
LUA_API void f(void) {
 const char *s = R"tag(";
LUA_API int ignored(void) { sprintf(b, "fake"); }
)tag";
 CALL(type); // a macro expression is still a call
 char b[10]; sprintf(b, "%s", s);
}
"###;
    let got = report(&[("lib.cpp", code)], "{}");
    assert_eq!(got["lines"], code.lines().count());
    assert_eq!(got["recovered"], 0, "{got}");
    assert_eq!(findings(&got, "cpp", "unsafe-buffer-calls"), 1);
}

#[test]
fn incomplete_calls_do_not_emit_rules_but_their_valid_siblings_do() {
    let got = report(
        &[(
            "lib.c",
            "#include <stdio.h>\nvoid f(void) {\n sprintf(b, );\n sprintf(b, \"ok\");\n}\n",
        )],
        "{}",
    );
    assert_eq!(got["recovered"], 1, "{got}");
    assert_eq!(findings(&got, "c", "unsafe-buffer-calls"), 1, "{got}");
}

#[test]
fn file_recovery_threshold_is_ten_percent_of_product_lines() {
    let keep = format!("{}int good;\n", "@\n".repeat(9));
    let got = report(&[("lib.c", &keep)], "{}");
    assert_eq!(got["lines"], 1, "{got}");
    assert_eq!(got["skipped"], 0);
    assert_eq!(got["recovered"], 1);
    let skip = format!("{}int good;\n", "@\n".repeat(10));
    let got = report(&[("lib.c", &skip)], "{}");
    assert_eq!(got["lines"], 0);
    assert_eq!(got["skipped"], 1);
    assert_eq!(got["coverage"]["c"]["total_lines"], 11);
    assert_eq!(got["partial"], true);
}

#[test]
fn uppercase_return_types_with_qualifiers_are_not_export_macros() {
    for lang in ["c", "cpp"] {
        let got = report(&[(if lang == "c" { "lib.c" } else { "lib.cpp" },
            "#include <stdio.h>\ntypedef int STATUS;\nSTATUS const f(void) { char b[10]; sprintf(b, \"ok\"); return 0; }\n")], "{}");
        assert_eq!(got["lines"], 3);
        assert_eq!(got["recovered"], 0, "{got}");
        assert_eq!(findings(&got, lang, "unsafe-buffer-calls"), 1);
    }
}
