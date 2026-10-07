use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;

fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (path, source) in files {
        vfs.insert(*path, *source);
    }
    let mut out = Vec::new();
    cqx_ts::run(&vfs, &mut out).expect("valid source");
    let stream = Stream::from_ndjson(&String::from_utf8(out).unwrap());
    let config = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&config, &Metrics::compute(&stream, &config))
}
fn findings(report: &serde_json::Value, id: &str) -> u64 {
    report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "typescript" && r["rule"] == id)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
fn count(source: &str, id: &str) -> u64 {
    findings(&report(&[("src/library.tsx", source)], "{}"), id)
}

#[test]
fn undocumented_suppressions_deduct_only_without_reason() {
    let bad = report(&[("src/lib.ts", "// @ts-ignore\nfoo();\n/* eslint-disable no-console */\nconsole.log(1);\n// eslint-disable-next-line --\nbar();")], "{}");
    assert_eq!(findings(&bad, "undocumented-suppressions"), 3);
    assert!(bad["scores"]["quality"].as_u64().unwrap() < 100);
    assert_eq!(count("// @ts-ignore: upstream types omit this\nfoo();\n/* eslint-disable no-console -- operational log */\nconsole.log(1);\nconst s = '// @ts-ignore';", "undocumented-suppressions"), 0);
}

#[test]
fn swallowed_errors_deduct_but_handled_and_documented_catches_do_not() {
    let bad = report(&[("src/lib.js", "try { work(); } catch (error) {}")], "{}");
    assert_eq!(findings(&bad, "swallowed-errors"), 1);
    assert!(bad["scores"]["quality"].as_u64().unwrap() < 100);
    assert_eq!(count("try { work(); } catch (error) { throw error; }\ntry { optional(); } catch { /* best effort cleanup; absence is expected */ }", "swallowed-errors"), 0);
}

#[test]
fn global_dynamic_code_deducts_but_shadowed_apis_and_callbacks_do_not() {
    let bad = report(&[("src/lib.ts", "eval(input); new Function('x', input); Function(input); setTimeout('work()', 1); setInterval(`work()`, 1);")], "{}");
    assert_eq!(findings(&bad, "dynamic-code"), 5);
    assert!(bad["scores"]["security"].as_u64().unwrap() < 100);
    assert_eq!(count("function f(eval: (s: string) => void, Function: any, setTimeout: any) { eval('x'); new Function(); setTimeout('x'); }\nsetTimeout(() => work(), 1); setInterval(work, 1); obj.eval(input);\nconst s = 'eval(input)';", "dynamic-code"), 0);
}

#[test]
fn exits_deduct_for_libraries_but_not_entrypoints_or_local_process_objects() {
    let bad = report(
        &[
            ("src/index.js", "process.exit(1);"),
            ("consumer.js", "import './src/index.js';"),
        ],
        "{}",
    );
    assert_eq!(findings(&bad, "exit-in-library"), 1);
    assert!(bad["scores"]["containment"].as_u64().unwrap() < 100);
    for entry in [
        "main.ts",
        "src/main.js",
        "src/cli.mjs",
        "tools/bin/start.cjs",
    ] {
        assert_eq!(
            findings(
                &report(&[(entry, "process.exit(1)")], "{}"),
                "exit-in-library"
            ),
            0
        );
    }
    assert_eq!(
        findings(
            &report(
                &[
                    ("package.json", r#"{"bin":{"tool":"./tools/start.ts"}}"#),
                    ("tools/start.ts", "process.exit(1)")
                ],
                "{}"
            ),
            "exit-in-library"
        ),
        0
    );
    assert_eq!(
        count(
            "function f(process: { exit(n: number): void }) { process.exit(1); }",
            "exit-in-library"
        ),
        0
    );
    assert_eq!(
        findings(
            &report(
                &[("tools/start.js", "#!/usr/bin/env node\nprocess.exit(1)")],
                "{}"
            ),
            "exit-in-library"
        ),
        0
    );
}

#[test]
fn html_review_rule_is_opt_in_and_accepts_literals() {
    let config = r#"{"version":1,"rules":{"typescript/dynamic-html":{"enabled":true}}}"#;
    let source =
        "node.innerHTML = input; const el = <div dangerouslySetInnerHTML={{__html: input}} />;";
    let bad = report(&[("src/lib.tsx", source)], config);
    assert_eq!(findings(&bad, "dynamic-html"), 2);
    assert!(bad["scores"]["security"].as_u64().unwrap() < 100);
    let good = report(&[("src/lib.tsx", "node.innerHTML = '<b>static</b>'; node.innerHTML = `static`; const el = <div dangerouslySetInnerHTML={{__html: 'static'}} />; node.textContent = input;")], config);
    assert_eq!(findings(&good, "dynamic-html"), 0);
    // Sanitizer contracts cannot be inferred: legitimate sanitized HTML does
    // not lose points under the defaults.
    let sanitized = report(
        &[("src/lib.tsx", "node.innerHTML = sanitize(input);")],
        "{}",
    );
    assert_eq!(sanitized["scores"]["security"], 100);
}

#[test]
fn any_density_is_opt_in_and_counts_types_not_names_or_strings() {
    let config = r#"{"version":1,"rules":{"typescript/any-density":{"enabled":true}}}"#;
    let bad = report(
        &[(
            "src/lib.ts",
            "let x: any; type T = any[]; const value = input as any;",
        )],
        config,
    );
    assert_eq!(findings(&bad, "any-density"), 3);
    assert!(bad["scores"]["quality"].as_u64().unwrap() < 100);
    let good = report(
        &[(
            "src/lib.ts",
            "let x: unknown; const any = 'any'; type T = string;",
        )],
        config,
    );
    assert_eq!(findings(&good, "any-density"), 0);
    assert_eq!(
        report(&[("src/lib.ts", "declare const interop: any;")], "{}")["scores"]["quality"],
        100
    );
}

#[test]
fn excludes_and_test_roles_remove_findings_and_denominators() {
    let got = report(
        &[
            ("src/lib.ts", "const x = 1;"),
            ("vendor/lib.ts", "eval(input);"),
            ("src/lib.test.ts", "eval(input);"),
        ],
        r#"{"exclude":["vendor/"]}"#,
    );
    assert_eq!(findings(&got, "dynamic-code"), 0);
    assert_eq!(got["lines"], 1);
}

#[test]
fn neutral_file_rules_use_existing_params_and_ramps() {
    let config = r#"{"rules":{"typescript/oversized-files":{"params":{"max_lines":2}},"typescript/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let bad = report(
        &[("src/lib.ts", "const a = 1;\nconst b = 2;\nconst c = 3;")],
        config,
    );
    assert_eq!(findings(&bad, "oversized-files"), 1);
    assert!(bad["scores"]["modularity"].as_u64().unwrap() < 100);
    let good = report(&[("src/lib.js", "const a = 1;\nconst b = 2;")], config);
    assert_eq!(findings(&good, "oversized-files"), 0);
    assert_eq!(good["scores"]["modularity"], 100);
}

#[test]
fn duplicate_tokens_deduct_but_small_wrappers_and_different_literals_do_not() {
    let body = "let total = firstValue + secondValue;".repeat(10) + "return total;";
    // Repeated assignments, rather than declarations, make a valid body.
    let body = body.replace("let total =", "total =");
    let source = format!(
        "function first() {{ {body} }}\nfunction second() {{\n{}\n}}",
        body.replace(';', ";\n")
    );
    let bad = report(&[("src/lib.ts", &source)], "{}");
    assert_eq!(findings(&bad, "duplicated-bodies"), 1);
    let duplicate = bad["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "typescript" && r["rule"] == "duplicated-bodies")
        .unwrap();
    assert_eq!(duplicate["findings"][0]["file"], "src/lib.ts");
    assert_eq!(duplicate["findings"][0]["line"], 2);
    assert!(bad["scores"]["quality"].as_u64().unwrap() < 100);
    assert_eq!(
        count(
            "function a() { return shared(); } function b() { return shared(); }",
            "duplicated-bodies"
        ),
        0
    );
    let left = "consume('a b');".repeat(30);
    let right = "consume('ab');".repeat(30);
    assert_eq!(
        count(
            &format!("function a() {{ {left} }} function b() {{ {right} }}"),
            "duplicated-bodies"
        ),
        0
    );
}

#[test]
fn every_extension_parses_and_bad_files_are_reported() {
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        let path = format!("src/lib.{ext}");
        let got = report(&[(&path, "function work() { eval(input); }")], "{}");
        assert_eq!(findings(&got, "dynamic-code"), 1);
    }
    let mut vfs = Vfs::new("broken");
    vfs.insert("bad.ts", "const = ;");
    let stats = cqx_ts::run(&vfs, Vec::new()).unwrap();
    assert_eq!(stats.files, 0);
    assert!(stats.unparsed[0].starts_with("bad.ts:"));
    let got = report(
        &[("bad.ts", "const = ;"), ("valid.ts", "eval(input);")],
        "{}",
    );
    assert_eq!(findings(&got, "dynamic-code"), 1);
    assert_eq!(got["skipped_files"][0]["file"], "bad.ts");
}

#[test]
fn configuration_cannot_create_a_sixth_category() {
    assert!(Config::from_text(Some(
        r#"{"rules":{"typescript/dynamic-code":{"category":"new-category"}}}"#
    ))
    .is_err());
}

#[test]
fn imported_node_process_is_detected_but_other_modules_and_shadowing_are_not() {
    assert_eq!(
        count(
            "import p from 'node:process'; p.exit(1);",
            "exit-in-library"
        ),
        1
    );
    assert_eq!(
        count(
            "import * as p from 'process'; p.exit(1);",
            "exit-in-library"
        ),
        1
    );
    assert_eq!(
        count("import p from './process'; p.exit(1);", "exit-in-library"),
        0
    );
    assert_eq!(
        count(
            "import p from 'node:process'; function f(p: {exit(n: number): void}) { p.exit(1); }",
            "exit-in-library"
        ),
        0
    );
}

#[test]
fn signatures_are_real_type_edges_in_the_shared_graph() {
    let mut vfs = Vfs::new("types");
    vfs.insert(
        "src/lib.ts",
        "export function f(input: string): number { return input.length; }",
    );
    let mut out = Vec::new();
    cqx_ts::run(&vfs, &mut out).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(out).unwrap());
    assert!(stream
        .edges
        .iter()
        .any(|e| e.kind == cqx_schema::EdgeKind::Param && e.to.0 == "type:string"));
    assert!(stream
        .edges
        .iter()
        .any(|e| e.kind == cqx_schema::EdgeKind::Returns && e.to.0 == "type:number"));
}

#[test]
fn with_object_scopes_are_not_mistaken_for_global_eval() {
    let got = report(
        &[(
            "src/lib.cjs",
            "with ({ eval(code) { return code; } }) { eval('ordinary method'); }",
        )],
        "{}",
    );
    assert_eq!(findings(&got, "dynamic-code"), 0);
}

#[test]
fn review_generated_and_declaration_files_do_not_score_or_dilute() {
    for (path, prefix) in [
        ("src/types.d.ts", ""),
        ("dist/bundle.min.js", ""),
        ("dist/bundle.min.mjs", ""),
        ("src/generated.ts", "  // @generated by codegen\n"),
    ] {
        let source = format!("{prefix}eval(input);\n{}", "work();\n".repeat(1000));
        let got = report(&[(path, &source), ("src/product.ts", "eval(input);")], "{}");
        assert_eq!(findings(&got, "dynamic-code"), 1, "{path}");
        assert_eq!(got["lines"], 1, "{path}");
        assert!(got["skipped_files"].is_null(), "{path}");
        assert_eq!(got["scores"]["security"], 70, "{path}");
    }
    for (path, source) in [
        ("src/api.generated.ts", "eval(input);"),
        ("src/api.generated.jsx", "eval(input);"),
        ("src/generated.js", "/* eslint-disable */\neval(input);"),
    ] {
        assert_eq!(
            findings(&report(&[(path, source)], "{}"), "dynamic-code"),
            1
        );
    }
    // A scoped disable with a reason is ordinary source, not a generated banner.
    assert_eq!(
        count(
            "/* eslint-disable no-console -- operational logging */\neval(input);",
            "dynamic-code"
        ),
        1
    );
    assert_eq!(
        count(
            "// @generatedLater is an ordinary annotation\neval(input);",
            "dynamic-code"
        ),
        1
    );
}

#[test]
fn review_direct_package_scripts_and_configuration_entries_stay_quiet() {
    for runner in ["node", "tsx", "bun"] {
        for ext in ["js", "ts"] {
            let path = format!("tools/run.{ext}");
            let manifest = format!(r#"{{"scripts":{{"start":"{runner} ./{path} --verbose"}}}}"#);
            let got = report(
                &[("package.json", &manifest), (&path, "process.exit(1);")],
                "{}",
            );
            assert_eq!(findings(&got, "exit-in-library"), 0, "{runner} {path}");
        }
    }
    let nested = report(
        &[
            (
                "packages/task/package.json",
                r#"{"scripts":{"start":"tsx ./tools/task.ts"}}"#,
            ),
            ("packages/task/tools/task.ts", "process.exit(1);"),
        ],
        "{}",
    );
    assert_eq!(findings(&nested, "exit-in-library"), 0);
    for path in [
        "scripts/task.ts",
        "tools/scripts/task.js",
        "bin/task.js",
        "vite.config.ts",
        "tools/build.config.js",
        "rollup.config.mjs",
        "eslint.config.cjs",
    ] {
        assert_eq!(
            findings(
                &report(&[(path, "process.exit(1);")], "{}"),
                "exit-in-library"
            ),
            0,
            "{path}"
        );
    }
    // Running a package, inline code or a test tool does not declare a library entry.
    for command in [
        "bun run library.ts",
        "node -e library.ts",
        "vitest library.ts",
    ] {
        let manifest = format!(r#"{{"scripts":{{"test":"{command}"}}}}"#);
        assert_eq!(
            findings(
                &report(
                    &[
                        ("package.json", &manifest),
                        ("library.ts", "function stop() { process.exit(1); }")
                    ],
                    "{}"
                ),
                "exit-in-library"
            ),
            1
        );
    }
}

#[test]
fn review_literal_function_constructors_stay_quiet() {
    let got = report(
        &[(
            "src/lib.ts",
            "Function('constant'); new Function('a', 'return a'); Function(); new Function();",
        )],
        "{}",
    );
    assert_eq!(findings(&got, "dynamic-code"), 0);
    assert_eq!(got["scores"]["security"], 100);
    assert_eq!(
        count(
            "Function(input); new Function('a', input); new Function(...args);",
            "dynamic-code"
        ),
        3
    );
}

#[test]
fn review_with_only_suppresses_globals_inside_its_body() {
    let got = report(&[("src/lib.cjs", "eval(before); with (eval(object)) { eval(method); with (other) { eval(nested); } } eval(after);")], "{}");
    assert_eq!(findings(&got, "dynamic-code"), 3);
    assert_eq!(got["scores"]["security"], 70);
}

#[test]
fn review_documented_json_parse_and_feature_detection_catches_stay_quiet() {
    let got = report(&[("src/lib.ts", "try { JSON.parse(candidate); } catch { /* Invalid JSON means try the next format. */ }\ntry { browser.optionalFeature(); } catch { // Feature is unavailable on older browsers; use the fallback.\n}")], "{}");
    assert_eq!(findings(&got, "swallowed-errors"), 0);
    assert_eq!(got["scores"]["quality"], 100);
}

#[test]
fn npm_nested_relative_scripts_and_unimported_mjs_are_entries() {
    let got = report(
        &[
            (
                "npm/package.json",
                r#"{"scripts":{"build":"node ../npm/build.mjs","test":"node ./test.mjs"}}"#,
            ),
            (
                "npm/build.mjs",
                "if (!process.argv[2]) { process.exit(2); } process.exit(1);",
            ),
            (
                "npm/test.mjs",
                "if (!binary) process.exit(2); process.exit(failed ? 1 : 0);",
            ),
            ("tools/check.mjs", "if (!ready) process.exit(2);"),
        ],
        "{}",
    );
    assert_eq!(findings(&got, "exit-in-library"), 0);
    assert_eq!(got["scores"]["containment"], 100);
    for importer in [
        "import '../npm/build.mjs';",
        "export * from '../npm/build.mjs';",
        "import('../npm/build.mjs');",
        "require('../npm/build.mjs');",
    ] {
        let got = report(
            &[
                ("src/lib.js", importer),
                ("npm/build.mjs", "process.exit(2);"),
            ],
            "{}",
        );
        assert_eq!(findings(&got, "exit-in-library"), 1, "{importer}");
    }
    let got = report(
        &[(
            "tools/lib.mjs",
            "export function stop() { process.exit(2); }",
        )],
        "{}",
    );
    assert_eq!(findings(&got, "exit-in-library"), 1);
}

#[test]
fn review_standalone_scripts_cover_all_non_jsx_extensions() {
    for ext in ["mjs", "js", "cjs", "ts", "mts", "cts"] {
        let path = format!("tools/check.{ext}");
        let got = report(&[(&path, "if (!ready) process.exit(2);")], "{}");
        assert_eq!(findings(&got, "exit-in-library"), 0, "{ext}");
        assert_eq!(got["lines"], 1, "{ext}");
        for importer in [
            format!("import '../{path}';"),
            format!("export * from '../{path}';"),
            format!("import('../{path}');"),
            format!("require('../{path}');"),
        ] {
            let got = report(
                &[("src/lib.js", &importer), (&path, "process.exit(2);")],
                "{}",
            );
            assert_eq!(findings(&got, "exit-in-library"), 1, "{ext}: {importer}");
        }
        let got = report(&[(&path, "function stop() { process.exit(2); }")], "{}");
        assert_eq!(findings(&got, "exit-in-library"), 1, "{ext}");
    }
}

#[test]
fn review_nested_runners_and_bins_resolve_outside_package_but_not_repo() {
    for runner in ["tsx", "bun"] {
        for target in ["./x.ts", "../shared/x.ts"] {
            let manifest = format!(r#"{{"scripts":{{"start":"{runner} {target}"}}}}"#);
            let path = if target.starts_with("./") {
                "packages/task/x.ts"
            } else {
                "packages/shared/x.ts"
            };
            let got = report(
                &[
                    ("packages/task/package.json", &manifest),
                    (path, "function stop() { process.exit(2); } stop();"),
                ],
                "{}",
            );
            assert_eq!(findings(&got, "exit-in-library"), 0, "{runner} {target}");
        }
    }
    for bin in [r#""../shared/x.cts""#, r#"{"app":"./../shared/x.cts"}"#] {
        let manifest = format!(r#"{{"bin":{bin}}}"#);
        let got = report(
            &[
                ("packages/task/package.json", &manifest),
                (
                    "packages/shared/x.cts",
                    "function stop() { process.exit(2); } stop();",
                ),
            ],
            "{}",
        );
        assert_eq!(findings(&got, "exit-in-library"), 0, "{bin}");
        assert_eq!(got["lines"], 1);
    }
    for field in [
        r#""bin":"../../../outside.ts""#,
        r#""bin":{"app":"../../../outside.ts"}"#,
        r#""scripts":{"start":"tsx ../../../outside.ts"}"#,
    ] {
        let manifest = format!("{{{field}}}");
        let got = report(
            &[
                ("packages/task/package.json", &manifest),
                ("outside.ts", "function stop() { process.exit(2); }"),
            ],
            "{}",
        );
        assert_eq!(findings(&got, "exit-in-library"), 1, "{field}");
    }
}

#[test]
fn review_extensionless_and_js_spelled_typescript_imports_are_libraries() {
    for (path, specifier) in [
        ("tools/check.ts", "../tools/check"),
        ("tools/check.ts", "../tools/check.js"),
        ("tools/check.mts", "../tools/check.mjs"),
        ("tools/check.cts", "../tools/check.cjs"),
        ("tools/check/index.ts", "../tools/check"),
    ] {
        let importer = format!("import '{specifier}';");
        let got = report(
            &[("src/lib.ts", &importer), (path, "process.exit(2);")],
            "{}",
        );
        assert_eq!(findings(&got, "exit-in-library"), 1, "{path}: {specifier}");
    }
}
