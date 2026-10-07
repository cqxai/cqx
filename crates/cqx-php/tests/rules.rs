use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (p, s) in files {
        vfs.insert(*p, *s);
    }
    let mut facts = Vec::new();
    cqx_php::run(&vfs, &mut facts, &|| {}).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let c = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&c, &Metrics::compute(&stream, &c))
}
fn count(code: &str, rule: &str) -> u64 {
    let p = report(&[("Library.php", code)], "{}");
    assert!(p["skipped_files"].is_null(), "{p}");
    findings(&p, rule)
}
fn findings(p: &serde_json::Value, rule: &str) -> u64 {
    p["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "php" && r["rule"] == rule)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
#[test]
fn exits_fire_but_entry_scripts_front_controllers_and_custom_methods_stay_quiet() {
    assert_eq!(
        count("<?php function f(){exit(1);die;}", "exit-in-library"),
        2
    );
    assert_eq!(
        count("<?php function f($obj){$obj->exit(1);}", "exit-in-library"),
        0
    );
    for path in ["public/index.php", "app/public/index.php", "index.php"] {
        assert_eq!(
            findings(
                &report(
                    &[("app/composer.json", "{}"), (path, "<?php exit(0);")],
                    "{}"
                ),
                "exit-in-library"
            ),
            0
        )
    }
    let p = report(
        &[
            ("pkg/composer.json", r#"{"bin":["tools/cli"]}"#),
            ("pkg/tools/cli", "#!/usr/bin/env php\n<?php exit(0);"),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "exit-in-library"), 0);
    assert_eq!(p["lines"], 2);
}
#[test]
fn dynamic_commands_fire_but_literals_argv_custom_apis_and_escaped_backticks_stay_quiet() {
    assert_eq!(count("<?php function f($input){system($input);shell_exec($input);exec($input);passthru($input);proc_open($input,$pipes,$out);$s=`git $input`;}","nonliteral-process"),6);
    assert_eq!(
        count(
            "<?php use function system as run; function f($input){run($input);}",
            "nonliteral-process"
        ),
        1
    );
    for code in [
        "<?php function f($input){system('git status');proc_open(['git',$input],$pipes,$out);}",
        "<?php namespace App; function system($input){} function f($input){system($input);}",
        "<?php function f($obj,$input){$obj->exec($input);}",
        r#"<?php $s=`echo \$input`;"#,
    ] {
        assert_eq!(count(code, "nonliteral-process"), 0, "{code}")
    }
    assert_eq!(
        count("<?php system(command: $input);", "nonliteral-process"),
        1
    );
}
#[test]
fn eval_fires_and_legacy_string_apis_require_declared_legacy_runtime() {
    assert_eq!(count("<?php eval($input);", "dynamic-code"), 1);
    for code in [
        "<?php assert($input);create_function('$arg',$input);",
        "<?php function f($obj){$obj->eval($input);}",
    ] {
        assert_eq!(count(code, "dynamic-code"), 0, "{code}")
    }
    let code="<?php function f(string $input){assert($input);create_function('$arg',$input);assert($input!==null);}";
    let p = report(
        &[
            ("composer.json", r#"{"require":{"php":"^7.4 || ^8.1"}}"#),
            ("Library.php", code),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "dynamic-code"), 2);
    let p = report(
        &[
            ("composer.json", r#"{"require":{"php":">=8.1"}}"#),
            ("Library.php", code),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "dynamic-code"), 0);
    let p = report(
        &[
            ("composer.json", r#"{"require":{"php":"^7.4"}}"#),
            (
                "Library.php",
                "<?php function f(bool $input){assert($input);}",
            ),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "dynamic-code"), 0);
}
#[test]
fn deserialization_fires_but_class_free_data_literals_and_internal_cache_stay_quiet() {
    assert_eq!(count("<?php function f(string $encoded){unserialize($encoded);unserialize($_POST['payload']);}","unsafe-deserialization"),2);
    for code in [
        "<?php function f($input){unserialize($input,['allowed_classes'=>false]);}",
        "<?php $cache='cached';unserialize($cache);",
        "<?php unserialize('s:3:\"foo\";');",
        "<?php namespace App; function unserialize($input){} function f($input){unserialize($input);}",
    ] {
        assert_eq!(count(code, "unsafe-deserialization"), 0, "{code}")
    }
}
#[test]
fn concatenated_sql_fires_only_at_bound_query_apis() {
    assert_eq!(
        count(
            "<?php function f(\\PDO $db,string $input){$db->query('SELECT '.$input);}",
            "concatenated-sql"
        ),
        1
    );
    assert_eq!(count("<?php namespace App;use PDO as Database;function f(Database $db,string $input){$db->query('SELECT '.$input);}","concatenated-sql"),1);
    assert_eq!(count("<?php $db=new mysqli();$db->query('SELECT '.$input);mysqli_query($db,'SELECT '.$input);","concatenated-sql"),2);
    for code in ["<?php function f(\\PDO $db,string $input){$db->query('SELECT 1');$db->query('SELECT '.'1');$db->prepare('SELECT ?');}","<?php function f($db,$input){$db->query('SELECT '.$input);}","<?php namespace App;function f(PDO $db,$input){$db->query('SELECT '.$input);}","<?php namespace App; class PDO{} function f(PDO $db,$input){$db->query('SELECT '.$input);}"]{assert_eq!(count(code,"concatenated-sql"),0,"{code}")}
}
#[test]
fn catch_and_error_suppression_fire_but_handling_and_attributes_stay_quiet() {
    assert_eq!(
        count(
            "<?php function f(){try{work();}catch(Exception $e){} @work();}",
            "swallowed-errors"
        ),
        1
    );
    assert_eq!(
        count("<?php function f(){@work();}", "error-suppression"),
        1
    );
    for code in ["<?php function f(){try{work();}catch(Exception $e){throw $e;}}","<?php function f(){try{optional();}catch(Exception $e){/* Missing optional file is expected. */}}"]{assert_eq!(count(code,"swallowed-errors"),0,"{code}")}
    assert_eq!(
        count(
            "<?php #[Attribute] class Marker{} function f(){work();}",
            "error-suppression"
        ),
        0
    );
}
#[test]
fn suppressions_fire_but_reasons_and_strings_stay_quiet() {
    assert_eq!(
        count(
            "<?php // phpcs:ignore\n$x=1; // @phpstan-ignore\n",
            "undocumented-suppressions"
        ),
        2
    );
    for code in ["<?php // phpcs:ignore Generic.Code -- compatibility with legacy source\n$x=1;","<?php // @phpstan-ignore variable.undefined (optional plugin variable)\n$x=1;","<?php /* @phpstan-ignore variable.undefined (optional plugin variable) */ $x=1;","<?php // Optional plugin registers this variable.\n$x=1; // @phpstan-ignore variable.undefined\n","<?php $s='@phpstan-ignore';"]{assert_eq!(count(code,"undocumented-suppressions"),0,"{code}")}
}
#[test]
fn exclusions_and_parse_failures_preserve_the_rest_of_the_scan() {
    for path in [
        "vendor/Library.php",
        "generated/Library.php",
        "tests/Library.php",
        "LibraryTest.php",
    ] {
        let p = report(
            &[
                (path, "<?php function f(){exit(1);}"),
                ("Quiet.php", "<?php function quiet(){}"),
            ],
            "{}",
        );
        assert_eq!(findings(&p, "exit-in-library"), 0);
        assert_eq!(p["lines"], 1)
    }
    let p = report(
        &[
            ("Broken.php", "<?php function broken( {"),
            ("Library.php", "<?php function f(){exit(1);}"),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "exit-in-library"), 1);
    assert_eq!(p["skipped_files"][0]["file"], "Broken.php");
    let p = report(
        &[
            ("Library.php", "<?php // @generated\nfunction f(){exit(1);}"),
            ("Quiet.php", "<?php function quiet(){}"),
        ],
        "{}",
    );
    assert_eq!(p["lines"], 1);
}
#[test]
fn duplicate_size_and_optional_type_facts_are_measured_without_inventing_types() {
    let body = "consume(1);".repeat(30);
    let code = format!("<?php function a(){{{body}}}function b(){{{body}}}");
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
            "<?php function a(){wrap();}function b(){wrap();}",
            "duplicated-bodies"
        ),
        0
    );
    let cfg = r#"{"rules":{"php/oversized-files":{"params":{"max_lines":2}},"php/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let p = report(&[("Library.php", "<?php function f(){\nwork();\n}")], cfg);
    assert_eq!(findings(&p, "oversized-files"), 1);
    assert!(p["scores"]["modularity"].as_u64().unwrap() < 100);
    assert_eq!(
        findings(
            &report(&[("Library.php", "<?php function f(){\n}")], cfg),
            "oversized-files"
        ),
        0
    );
    let mut vfs = Vfs::new("typed");
    vfs.insert(
        "Library.php",
        "<?php declare(strict_types=1);function f(string $input): int {return 1;}",
    );
    let mut facts = Vec::new();
    cqx_php::run(&vfs, &mut facts, &|| {}).unwrap();
    let facts = String::from_utf8(facts).unwrap();
    assert!(facts.contains("php:strict_types\":\"true"));
    assert!(facts.contains("php:return\":\"int"));
    assert!(facts.contains("string $input"));
}

#[test]
fn numeric_or_quoted_sql_values_stay_quiet() {
    for code in [
        "<?php function f(\\PDO $db,int $id){$db->query('SELECT '.$id);}",
        "<?php function f(\\PDO $db,$input){$db->query('SELECT '.(int)$input);}",
        "<?php function f(\\PDO $db,string $input){$db->query('SELECT '.$db->quote($input));}",
    ] {
        assert_eq!(count(code, "concatenated-sql"), 0, "{code}")
    }
}

#[test]
fn phpunit_binding_is_distinct_from_custom_testcase_names() {
    assert_eq!(
        count(
            "<?php class Production extends OtherTestCase{function f(){exit(1);}}",
            "exit-in-library"
        ),
        1
    );
    let p=report(&[("ExampleTest.php","<?php use PHPUnit\\Framework\\TestCase;class Example extends TestCase{function testStop(){exit(1);}}"),("Quiet.php","<?php function quiet(){}")],"{}");
    assert_eq!(findings(&p, "exit-in-library"), 0);
    assert_eq!(p["lines"], 1);
}

#[test]
fn process_termination_is_counted_once_per_site() {
    assert_eq!(count("<?php exit(1);", "exit-in-library"), 1);
    assert_eq!(count("<?php die('bad');", "exit-in-library"), 1);
    assert_eq!(count("<?php exit(1); exit(1);", "exit-in-library"), 2);
    assert_eq!(count("<?php exit; die; exit(1);", "exit-in-library"), 3);
}
#[test]
fn query_builders_and_concatenated_placeholder_sql_are_quiet() {
    for code in [
        "<?php function f($builder,$input){$builder->query('select * from t where id='.$input);}",
        "<?php function f(PDO $db){$db->query('select * from t'.' where id = ?');}",
        "<?php function f(PDO $db){$db->prepare('select * from t'.' where id = ?')->execute([$input]);}",
    ] { assert_eq!(count(code,"concatenated-sql"),0,"{code}"); }
}
