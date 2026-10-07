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
        assert_eq!(count(lang, "// NOLINT(readability-identifier-naming)\nvoid f(void) {}\n#pragma clang diagnostic ignored \"-Wconversion\"\n", "undocumented-suppressions"), 2);
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
            (
                "generated.c",
                "// Generated by tool; DO NOT EDIT\n#error not parsed",
            ),
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
