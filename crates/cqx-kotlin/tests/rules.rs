use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (p, s) in files {
        vfs.insert(*p, *s);
    }
    let mut facts = Vec::new();
    cqx_kotlin::run(&vfs, &mut facts, &|| {}).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let c = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&c, &Metrics::compute(&stream, &c))
}
fn count(code: &str, rule: &str) -> u64 {
    let p = report(&[("Library.kt", code)], "{}");
    assert!(p["skipped_files"].is_null(), "{p}");
    findings(&p, rule)
}
fn findings(p: &serde_json::Value, rule: &str) -> u64 {
    p["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "kotlin" && r["rule"] == rule)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
#[test]
fn exits_fire_but_main_import_bindings_and_user_functions_stay_quiet() {
    assert_eq!(count("import kotlin.system.exitProcess\nfun stop(){exitProcess(1);kotlin.system.exitProcess(1)}","exit-in-library"),2);
    assert_eq!(
        count(
            "import kotlin.system.exitProcess as stop\nfun f(){stop(1)}",
            "exit-in-library"
        ),
        1
    );
    for code in [
        "fun main(){kotlin.system.exitProcess(1)}",
        "fun main(vararg args:String){kotlin.system.exitProcess(1)}",
        "fun main(args:Array<String>){kotlin.system.exitProcess(1)}",
        "object App { @JvmStatic fun main(args:Array<String>){kotlin.system.exitProcess(1)} }",
        "fun exitProcess(n:Int){}\nfun f(){exitProcess(1)}",
        "import custom.exitProcess\nfun f(){exitProcess(1)}",
    ] {
        assert_eq!(count(code, "exit-in-library"), 0, "{code}");
    }
    assert_eq!(
        count(
            "fun wrapper(){fun main(){kotlin.system.exitProcess(1)}}",
            "exit-in-library"
        ),
        1
    );
    assert_eq!(
        count(
            "class Library { fun main(){kotlin.system.exitProcess(1)} }",
            "exit-in-library"
        ),
        1
    );
    let p = report(
        &[("L.kt", "fun stop(){kotlin.system.exitProcess(1)}")],
        "{}",
    );
    assert!(p["scores"]["containment"].as_u64().unwrap() < 100);
}
#[test]
fn nonliteral_runtime_exec_fires_but_literal_commands_and_custom_receivers_stay_quiet() {
    assert_eq!(count("fun f(input:String){Runtime.getRuntime().exec(input);java.lang.Runtime.getRuntime().exec(input)}","nonliteral-process"),2);
    for code in ["fun f(input:String){Runtime.getRuntime().exec(\"git status\")}","fun f(input:String){Runtime.getRuntime().exec(arrayOf(\"git\",input))}","fun f(Runtime:Other,input:String){Runtime.getRuntime().exec(input)}","class Runtime { companion object { fun getRuntime():Other = Other() } }\nfun f(input:String){Runtime.getRuntime().exec(input)}"]{assert_eq!(count(code,"nonliteral-process"),0,"{code}");}
    assert_eq!(
        count(
            "fun f(input:String){Runtime.getRuntime().exec(\"git $input\")}",
            "nonliteral-process"
        ),
        1
    );
}
#[test]
fn empty_catches_fire_but_handling_and_explanations_stay_quiet() {
    assert_eq!(
        count(
            "fun f(){try{work()}catch(e:Exception){}}",
            "swallowed-errors"
        ),
        1
    );
    for code in [
        "fun f(){try{work()}catch(e:Exception){throw e}}",
        "fun f(){try{optional()}catch(e:Exception){/* Missing optional feature is expected. */}}",
    ] {
        assert_eq!(count(code, "swallowed-errors"), 0, "{code}");
    }
}
#[test]
fn suppressions_fire_but_reason_comments_and_user_annotations_stay_quiet() {
    assert_eq!(
        count("@Suppress(\"ALL\") fun f(){}", "undocumented-suppressions"),
        1
    );
    for code in ["// Validated generic cast at the interoperability boundary.\n@Suppress(\"UNCHECKED_CAST\") fun f(){}","@Suppress(\"UNCHECKED_CAST\") // Validated cast at this boundary.\nfun f(){}","fun f(){val text=\"@Suppress\"}","annotation class Suppress(val value:String)\n@Suppress(\"custom\") fun f(){}"]{assert_eq!(count(code,"undocumented-suppressions"),0,"{code}");}
}
#[test]
fn roles_and_parse_failures_preserve_the_rest_of_the_scan() {
    for path in [
        "build/Generated.kt",
        ".gradle/Generated.kt",
        "src/commonTest/kotlin/L.kt",
        "src/jvmTest/kotlin/L.kt",
        "src/test/kotlin/LTest.kt",
        "src/test/kotlin/L.kt",
    ] {
        let p = report(
            &[
                (path, "fun f(){kotlin.system.exitProcess(1)}"),
                ("Quiet.kt", "fun quiet(){}"),
            ],
            "{}",
        );
        assert_eq!(findings(&p, "exit-in-library"), 0, "{path}");
        assert_eq!(p["lines"], 1);
    }
    let p = report(
        &[
            (
                "Generated.kt",
                "// @generated\nfun f(){kotlin.system.exitProcess(1)}",
            ),
            ("Quiet.kt", "fun quiet(){}"),
        ],
        "{}",
    );
    assert_eq!(p["lines"], 1);
    assert_eq!(findings(&p, "exit-in-library"), 0);
    let p = report(
        &[
            ("Library.kt", "fun f(){kotlin.system.exitProcess(1)}"),
            ("Broken.kt", "fun broken( {"),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "exit-in-library"), 1);
    assert_eq!(p["skipped_files"][0]["file"], "Broken.kt");
}
#[test]
fn duplicate_and_size_rules_fire_with_config_and_quiet_counterexamples() {
    let body = "consume(1);".repeat(30);
    let code = format!("fun a(){{{body}}}\nfun b(){{{body}}}");
    assert_eq!(count(&code, "duplicated-bodies"), 1);
    assert_eq!(
        count(
            &code.replacen("consume(1)", "consume(2)", 1),
            "duplicated-bodies"
        ),
        0
    );
    assert_eq!(
        count("fun a(){wrap()}\nfun b(){wrap()}", "duplicated-bodies"),
        0
    );
    let cfg = r#"{"rules":{"kotlin/oversized-files":{"params":{"max_lines":2}},"kotlin/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let p = report(&[("L.kt", "fun f(){\n work()\n}")], cfg);
    assert_eq!(findings(&p, "oversized-files"), 1);
    assert!(p["scores"]["modularity"].as_u64().unwrap() < 100);
    assert_eq!(
        findings(&report(&[("L.kt", "fun f(){\n}")], cfg), "oversized-files"),
        0
    );
}
