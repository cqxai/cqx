use cqx_score::{config::Config, metrics::Metrics};
use cqx_store::facts::Stream;
use cqx_vfs::Vfs;
fn report(files: &[(&str, &str)], config: &str) -> serde_json::Value {
    let mut vfs = Vfs::new("fixture");
    for (p, s) in files {
        vfs.insert(*p, *s);
    }
    let mut facts = Vec::new();
    cqx_python::run(&vfs, &mut facts, &|| {}).unwrap();
    let stream = Stream::from_ndjson(&String::from_utf8(facts).unwrap());
    let c = Config::from_text(Some(config)).unwrap();
    cqx_score::report_json(&c, &Metrics::compute(&stream, &c))
}
fn count(code: &str, rule: &str) -> u64 {
    let p = report(&[("lib.py", code)], "{}");
    assert!(p["skipped_files"].is_null(), "{p}");
    findings(&p, rule)
}
fn findings(p: &serde_json::Value, rule: &str) -> u64 {
    p["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == "python" && r["rule"] == rule)
        .map(|r| r["total_findings"].as_u64().unwrap())
        .unwrap_or(0)
}
#[test]
fn exits_fire_but_guards_entry_modules_and_custom_bindings_stay_quiet() {
    assert_eq!(count("import sys as system\nfrom os import _exit as stop\ndef f():\n system.exit(1)\n stop(1)\n","exit-in-library"),2);
    for code in [
        "import sys\nif __name__ == \"__main__\":\n sys.exit(0)\n",
        "import custom as sys\ndef f():\n sys.exit(1)\n",
        "import sys\ndef f(sys):\n sys.exit(1)\n",
    ] {
        assert_eq!(count(code, "exit-in-library"), 0, "{code}")
    }
    assert_eq!(
        findings(
            &report(&[("__main__.py", "import sys\nsys.exit(0)\n")], "{}"),
            "exit-in-library"
        ),
        0
    );
    for (manifest,config)in [("pkg/pyproject.toml","[project.scripts]\ncli='library:main'\n"),("pkg/setup.cfg","[options.entry_points]\nconsole_scripts =\n cli = library:main\n"),("pkg/setup.py","from setuptools import setup\nsetup(entry_points={'console_scripts':['cli=library:main']})\n")] {let p=report(&[(manifest,config),("pkg/src/library.py","import sys\ndef main():\n sys.exit(0)\n")],"{}");assert_eq!(findings(&p,"exit-in-library"),0,"{manifest}")}
}
#[test]
fn shell_commands_fire_but_literal_commands_argv_and_custom_receivers_stay_quiet() {
    assert_eq!(count("import os\nimport subprocess as sp\ndef f(input):\n os.system(input)\n sp.run(input,shell=True)\n","nonliteral-process"),2);
    for code in [
        "import subprocess\ndef f(input):\n subprocess.run(['git',input],shell=False)\n",
        "import os\nos.system('git status')\n",
        "import custom as subprocess\nsubprocess.run(input,shell=True)\n",
    ] {
        assert_eq!(count(code, "nonliteral-process"), 0, "{code}")
    }
    assert_eq!(
        count(
            "import os\nos.system(f'git {input}')\n",
            "nonliteral-process"
        ),
        1
    );
}
#[test]
fn dynamic_code_fires_but_literal_eval_and_shadowed_helpers_stay_quiet() {
    assert_eq!(count("eval(input)\nexec(input)\n", "dynamic-code"), 2);
    for code in [
        "import ast\nast.literal_eval(input)\n",
        "def eval(input):\n return input\neval(input)\n",
        "from custom import exec\nexec(input)\n",
    ] {
        assert_eq!(count(code, "dynamic-code"), 0, "{code}")
    }
}
#[test]
fn unsafe_deserialization_fires_but_safe_loaders_json_and_custom_bindings_stay_quiet() {
    assert_eq!(
        count(
            "import pickle\nimport yaml\npickle.load(stream)\nyaml.load(input)\n",
            "unsafe-deserialization"
        ),
        2
    );
    for code in [
        "import yaml\nyaml.load(input,Loader=yaml.SafeLoader)\n",
        "from yaml import load, CSafeLoader as Safe\nload(input,Loader=Safe)\n",
        "import yaml\nyaml.safe_load(input)\n",
        "import yaml\nyaml.load(input, yaml.SafeLoader)\n",
        "import json\njson.load(stream)\n",
        "import custom as pickle\npickle.load(stream)\n",
    ] {
        assert_eq!(count(code, "unsafe-deserialization"), 0, "{code}")
    }
}
#[test]
fn broad_discards_fire_but_handling_reraise_and_explanations_stay_quiet() {
    assert_eq!(
        count(
            "try:\n work()\nexcept:\n pass\ntry:\n work()\nexcept Exception:\n pass\n",
            "swallowed-errors"
        ),
        2
    );
    for code in [
        "try:\n work()\nexcept Exception:\n log(error)\n",
        "try:\n work()\nexcept:\n cleanup()\n raise\n",
        "try:\n optional()\nexcept Exception:\n # Missing optional file is expected.\n pass\n",
    ] {
        assert_eq!(count(code, "swallowed-errors"), 0, "{code}")
    }
}
#[test]
fn suppressions_fire_but_explicit_reasons_and_ordinary_comments_stay_quiet() {
    assert_eq!(
        count(
            "x=1 # noqa: F401\ny=2 # type: ignore[arg-type]\n",
            "undocumented-suppressions"
        ),
        2
    );
    for code in [
        "x=1 # noqa: F401 -- imported for registration\n",
        "y=2 # type: ignore[arg-type] # validated by external schema\n",
        "# Registered plugin needs the side-effect import.\nx=1 # noqa: F401\n",
        "x='# noqa'\n",
        "# We avoid noqa directives here.\nx=1\n",
    ] {
        assert_eq!(count(code, "undocumented-suppressions"), 0, "{code}")
    }
}
#[test]
fn exclusions_and_parse_failures_preserve_the_rest_of_the_scan() {
    for path in [
        ".venv/lib.py",
        "venv/lib.py",
        "__pycache__/lib.py",
        "site-packages/lib.py",
        "tests/lib.py",
        "test_lib.py",
        "conftest.py",
    ] {
        let p = report(
            &[
                (path, "import sys\nsys.exit(1)\n"),
                ("quiet.py", "def quiet():\n pass\n"),
            ],
            "{}",
        );
        assert_eq!(findings(&p, "exit-in-library"), 0);
        assert_eq!(p["lines"], 2)
    }
    let p = report(
        &[
            ("broken.py", "def broken( :"),
            ("lib.py", "import sys\nsys.exit(1)\n"),
        ],
        "{}",
    );
    assert_eq!(findings(&p, "exit-in-library"), 1);
    assert_eq!(p["skipped_files"][0]["file"], "broken.py");
    let p = report(
        &[
            (
                "generated.py",
                "# generated by tool\nimport sys\nsys.exit(1)\n",
            ),
            ("quiet.py", "pass\n"),
        ],
        "{}",
    );
    assert_eq!(p["lines"], 1);
}
#[test]
fn duplication_and_size_fire_with_config_and_quiet_counterexamples() {
    let body = " consume(1)\n".repeat(30);
    let code = format!("def a():\n{body}def b():\n{body}");
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
            "def a(): return wrap()\ndef b(): return wrap()\n",
            "duplicated-bodies"
        ),
        0
    );
    let cfg = r#"{"rules":{"python/oversized-files":{"params":{"max_lines":2}},"python/oversized-line-share":{"params":{"max_lines":2}}}}"#;
    let p = report(&[("lib.py", "def f():\n work()\n work()\n")], cfg);
    assert_eq!(findings(&p, "oversized-files"), 1);
    assert!(p["scores"]["modularity"].as_u64().unwrap() < 100);
    assert_eq!(
        findings(
            &report(&[("lib.py", "def f():\n pass\n")], cfg),
            "oversized-files"
        ),
        0
    );
}

#[test]
fn keyword_shell_arguments_and_guard_else_are_bound_correctly() {
    assert_eq!(
        count(
            "import subprocess\nsubprocess.run(args=input,shell=True)\n",
            "nonliteral-process"
        ),
        1
    );
    assert_eq!(
        count(
            "import sys\nif __name__ == \"__main__\":\n pass\nelse:\n sys.exit(1)\n",
            "exit-in-library"
        ),
        1
    );
    assert_eq!(count("ｅｖａｌ(input)\n", "dynamic-code"), 1);
}
