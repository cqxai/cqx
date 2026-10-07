use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (p, s) in files {
        vfs.insert(*p, *s);
    }
    let mut facts = Vec::new();
    cqx_swift::run(&vfs, &mut facts, &|| {}).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let c = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&c, &Metrics::compute(&stream, &c))
}
fn count(code: &str, rule: &str) -> u64 {
    let p = report(&[("Library.swift", code)], "{}");
    assert!(p["skipped_files"].is_null(), "{p}");
    findings(&p, rule)
}
fn findings(p: &serde_json::Value, rule: &str) -> u64 {
    p["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "swift" && r["rule"] == rule)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
#[test]
fn exits_fire_but_entries_and_user_functions_stay_quiet() {
    assert_eq!(
        count(
            "func stop(){exit(1);fatalError(\"bad\")}",
            "exit-in-library"
        ),
        2
    );
    for code in [
        "@main struct App { static func main(){exit(0)} }",
        "exit(0)",
        "func exit(_ code:Int){}; func f(){exit(1)}",
        "func f(fatalError: (String)->Void){fatalError(\"bad\")}",
    ] {
        assert_eq!(count(code, "exit-in-library"), 0, "{code}")
    }
    assert_eq!(
        count("struct Library {func main(){exit(0)}}", "exit-in-library"),
        1
    );
    assert_eq!(
        findings(
            &report(&[("main.swift", "exit(0)")], "{}"),
            "exit-in-library"
        ),
        0
    );
}
#[test]
fn dynamic_process_paths_fire_but_literals_arguments_and_custom_types_stay_quiet() {
    let dynamic="import Foundation\nfunc f(input:String){let p=Process();p.executableURL=URL(fileURLWithPath:input);try p.run()}";
    assert_eq!(count(dynamic, "nonliteral-process"), 1);
    assert_eq!(count("import Foundation\nfunc f(input:String){let p=Process();p.launchPath=input;p.launch()}","nonliteral-process"),1);
    for code in [dynamic.replace("fileURLWithPath:input","fileURLWithPath:\"/bin/git\""),"import Foundation\nfunc f(input:String){let p=Process();p.launchPath=\"/bin/git\";p.arguments=[input];p.launch()}".into(),"struct Process {}; func f(input:String){let p=Process();p.launchPath=input;p.launch()}".into(),"func f(p:Custom,input:String){p.launchPath=input;p.launch()}".into()]{assert_eq!(count(&code,"nonliteral-process"),0,"{code}")}
}
#[test]
fn forced_operations_fire_but_optional_handling_and_type_annotations_stay_quiet() {
    assert_eq!(
        count(
            "func f(value:Int?){let a=value!;let b=try! work()}",
            "forced-operations"
        ),
        2
    );
    for code in [
        "func f(value:Int?){if let value {consume(value)};let b=try? work()}",
        "func f(flag:Bool){if !flag {work()}}",
        "class View {var outlet: Widget!}",
    ] {
        assert_eq!(count(code, "forced-operations"), 0, "{code}")
    }
}
#[test]
fn empty_catches_fire_but_handling_and_explanations_stay_quiet() {
    assert_eq!(
        count("func f(){do{try work()}catch{}}", "swallowed-errors"),
        1
    );
    for code in [
        "func f(){do{try work()}catch{throw error}}",
        "func f(){do{try optional()}catch{/* Optional file may be absent. */}}",
    ] {
        assert_eq!(count(code, "swallowed-errors"), 0, "{code}")
    }
}
#[test]
fn roles_and_parse_failures_preserve_other_files() {
    for path in [
        ".build/L.swift",
        "Pods/L.swift",
        "DerivedData/L.swift",
        "Tests/L.swift",
    ] {
        let p = report(
            &[
                (path, "func f(){exit(1)}"),
                ("Quiet.swift", "func quiet(){}"),
            ],
            "{}",
        );
        assert_eq!(findings(&p, "exit-in-library"), 0, "{path}");
        assert_eq!(p["lines"], 1);
    }
    let p = report(
        &[
            ("Broken.swift", "func broken( {"),
            ("Library.swift", "func f(){exit(1)}"),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "exit-in-library"), 1);
    assert_eq!(p["skipped_files"][0]["file"], "Broken.swift");
    let p = report(
        &[
            ("L.swift", "// @generated\nfunc f(){exit(1)}"),
            ("Quiet.swift", "func quiet(){}"),
        ],
        "{}",
    );
    assert_eq!(p["lines"], 1);
}
#[test]
fn duplicate_and_size_rules_fire_with_config_and_quiet_counterexamples() {
    let body = "consume(1);".repeat(30);
    let code = format!("func a(){{{body}}}\nfunc b(){{{body}}}");
    assert_eq!(count(&code, "duplicated-bodies"), 1);
    assert_eq!(
        count(
            &code.replacen("consume(1)", "consume(2)", 1),
            "duplicated-bodies"
        ),
        0
    );
    assert_eq!(
        count("func a(){wrap()}\nfunc b(){wrap()}", "duplicated-bodies"),
        0
    );
    let cfg = r#"{"rules":{"swift/oversized-files":{"params":{"max_lines":2}},"swift/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let p = report(&[("L.swift", "func f(){\nwork()\n}")], cfg);
    assert_eq!(findings(&p, "oversized-files"), 1);
    assert!(p["scores"]["modularity"].as_u64().unwrap() < 100);
    assert_eq!(
        findings(
            &report(&[("L.swift", "func f(){\n}")], cfg),
            "oversized-files"
        ),
        0
    );
}

#[test]
fn forced_density_excludes_package_script_top_level_and_test_targets() {
    for path in ["Package.swift", "main.swift", "Tests/Helper.swift"] {
        let p = report(&[(path, "let n = value!; let r = try! work()")], "{}");
        assert_eq!(findings(&p, "forced-operations"), 0, "{path}");
    }
    let p = report(
        &[(
            "main.swift",
            "let n = value!; func helper(value:Int?){let n = value!; let r = try! work()}",
        )],
        "{}",
    );
    assert_eq!(findings(&p, "forced-operations"), 2);
}
