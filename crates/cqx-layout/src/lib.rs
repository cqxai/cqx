//! Project roots, source roles and bounded idiom policy shared by every frontend.
//! Paths are snapshot-relative, slash-separated; roots end with `/` (except `""`).
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, ops::Range};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    pub roots: BTreeSet<String>,
    pub virtualenvs: BTreeSet<String>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            roots: BTreeSet::from([String::new()]),
            virtualenvs: BTreeSet::new(),
        }
    }
}
pub fn is_manifest(name: &str) -> bool {
    matches!(
        name,
        "CMakeLists.txt"
            | "Makefile"
            | "meson.build"
            | "configure.ac"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "settings.gradle"
            | "settings.gradle.kts"
            | "Package.swift"
            | "build.zig"
            | "pyproject.toml"
            | "setup.py"
            | "setup.cfg"
            | "composer.json"
            | "package.json"
            | "go.mod"
            | "Cargo.toml"
    ) || name.ends_with(".csproj")
}
impl Layout {
    pub fn from_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        let mut layout = Self::default();
        for path in paths {
            let (root, name) = path
                .rsplit_once('/')
                .map_or(("", path), |(root, name)| (root, name));
            let root = if root.is_empty() {
                String::new()
            } else {
                format!("{root}/")
            };
            if is_manifest(name) {
                layout.roots.insert(root.clone());
            }
            if name == "pyvenv.cfg" {
                layout.virtualenvs.insert(root);
            }
        }
        layout
    }
    pub fn relatives<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.roots
            .iter()
            .filter_map(move |root| path.strip_prefix(root))
    }
    pub fn excluded_dir(&self, language: &str, path: &str) -> bool {
        self.virtualenvs.iter().any(|root| path.starts_with(root))
            || self.relatives(path).any(|relative| {
                let Some((dir, _)) = relative.split_once('/') else {
                    return false;
                };
                excluded_name(language, dir)
            })
    }
    pub fn excluded(&self, language: &str, path: &str, source: &str) -> bool {
        self.excluded_dir(language, path) || generated_header(language, source)
    }
    pub fn is_test(&self, language: &str, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        match language {
            "java" | "kotlin" => self.relatives(path).any(|relative| {
                let mut parts = relative.split('/');
                if parts.next() != Some("src") {
                    return false;
                }
                let Some(set) = parts.next() else {
                    return false;
                };
                matches!(
                    set,
                    "test" | "testFixtures" | "androidTest" | "integrationTest"
                ) || language == "kotlin"
                    && set
                        .strip_suffix("Test")
                        .is_some_and(|prefix| !prefix.is_empty())
            }),
            "swift" => self.relatives(path).any(|p| p.starts_with("Tests/")),
            "zig" => name.ends_with("_test.zig"),
            "python" => {
                self.relatives(path).any(|p| p.starts_with("tests/"))
                    || name == "conftest.py"
                    || name.starts_with("test_") && name.ends_with(".py")
                    || name.ends_with("_test.py")
            }
            "php" => {
                self.relatives(path).any(|p| p.starts_with("tests/"))
                    || name.ends_with("Test.php")
                    || name.ends_with(".phpt")
            }
            "go" => {
                name.ends_with("_test.go")
                    || self.relatives(path).any(|p| p.starts_with("testdata/"))
            }
            "typescript" => {
                name.contains(".test.")
                    || name.contains(".spec.")
                    || self.relatives(path).any(|p| {
                        p.starts_with("test/")
                            || p.starts_with("tests/")
                            || p.starts_with("__tests__/")
                    })
            }
            "c" | "cpp" => {
                self.relatives(path).any(|p| {
                    p.split_once('/').is_some_and(|(dir, _)| {
                        matches!(
                            dir,
                            "test"
                                | "tests"
                                | "testing"
                                | "unittest"
                                | "unittests"
                                | "fuzz"
                                | "fuzzing"
                        )
                    })
                }) || name.starts_with("test_")
                    || name.contains("_test.")
                    || name.contains("_tests.")
            }
            "csharp" => {
                self.relatives(path).any(|p| {
                    p.split_once('/').is_some_and(|(dir, _)| {
                        matches!(
                            dir.to_ascii_lowercase().as_str(),
                            "test" | "tests" | "benchmarks"
                        )
                    })
                }) || self.roots.iter().any(|root| {
                    path.starts_with(root)
                        && root
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .is_some_and(|name| name.ends_with(".Tests") || name.ends_with(".Test"))
                }) || name.ends_with("Tests.cs")
                    || name.ends_with("Test.cs")
            }
            "rust" => self.relatives(path).any(|p| p.starts_with("tests/")),
            _ => false,
        }
    }
    pub fn front_controller(&self, path: &str) -> bool {
        self.relatives(path)
            .any(|p| matches!(p, "index.php" | "public/index.php" | "web/index.php"))
    }
}
/// Discovery uses the union; analysis uses the ecosystem's names. Never apply
/// this predicate to arbitrary path segments without a project-root anchor.
pub fn excluded_name(language: &str, name: &str) -> bool {
    let common = matches!(
        name,
        ".git" | ".tmp" | "vendor" | "generated" | "build" | "target"
    ) || name == ".target";
    common
        || match language {
            "java" | "kotlin" => matches!(name, ".gradle" | "generated-sources"),
            "swift" => matches!(name, ".build" | "Pods" | "DerivedData" | "Carthage"),
            "zig" => matches!(name, "zig-cache" | ".zig-cache" | "zig-out"),
            "python" => {
                matches!(
                    name,
                    "dist" | ".tox" | "node_modules" | "__pycache__" | "site-packages"
                ) || name.ends_with(".egg-info")
            }
            "php" => name == "cache",
            "typescript" => matches!(
                name,
                "node_modules"
                    | ".next"
                    | "dist"
                    | "coverage"
                    | ".open-next"
                    | ".wrangler"
                    | ".turbo"
            ),
            "c" | "cpp" => {
                matches!(
                    name,
                    "deps" | "third_party" | "third-party" | "external" | "out" | "_deps"
                ) || name.starts_with("cmake-build-")
            }
            "csharp" => matches!(name, "obj" | "bin" | "packages"),
            "go" | "rust" => false,
            "" => ["java", "swift", "zig", "python", "php", "typescript"]
                .iter()
                .any(|l| excluded_name(l, name)),
            _ => false,
        }
}
/// Real header markers only. Ordinary comments discussing generated values,
/// "generated by" or "do not edit" prose are source, not provenance markers.
/// Annotation-based markers are supplied by the frontend's parsed annotation.
pub fn generated_header(language: &str, source: &str) -> bool {
    let mut block = false;
    for raw in source.lines() {
        let in_block = block;
        let line = raw.trim();
        let line = if language == "php" {
            line.strip_prefix("<?php").unwrap_or(line).trim_start()
        } else {
            line
        };
        if line.starts_with("/*") {
            block = true;
        }
        if line.contains("*/") {
            block = false;
        }
        if line.is_empty() || line == "<?php" || line.starts_with("#!") {
            continue;
        }
        let comment = if language == "python" {
            line.strip_prefix('#')
        } else {
            line.strip_prefix("//")
                .or_else(|| {
                    (language == "php")
                        .then(|| line.strip_prefix('#'))
                        .flatten()
                })
                .or_else(|| line.strip_prefix("/*"))
                .or_else(|| in_block.then(|| line.strip_prefix('*')).flatten())
        };
        let Some(comment) = comment else {
            break;
        };
        let comment = comment.trim().trim_end_matches("*/").trim();
        if comment
            .strip_prefix("@generated")
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
        {
            return true;
        }
        if (language == "python"
            || language == "go" && !in_block && raw.starts_with("// Code generated "))
            && comment
                .strip_prefix("Code generated ")
                .and_then(|s| s.strip_suffix(" DO NOT EDIT."))
                .is_some()
        {
            return true;
        }
        if language == "csharp" && matches!(comment, "<auto-generated>" | "<auto-generated />") {
            return true;
        }
    }
    false
}

/// AST declarations establish boundaries. An entry never grants an exemption to
/// a nested helper/closure; top-level scripts exempt only calls outside them.
#[derive(Default)]
pub struct Entries {
    declarations: Vec<Range<usize>>,
    entries: Vec<Range<usize>>,
    script: bool,
}
impl Entries {
    pub fn script(&mut self, yes: bool) {
        self.script = yes;
    }
    pub fn declaration(&mut self, range: Range<usize>, entry: bool) {
        self.declarations.push(range.clone());
        if entry {
            self.entries.push(range);
        }
    }
    pub fn block(&mut self, range: Range<usize>) {
        self.entries.push(range);
    }
    pub fn in_script_body(&self, range: &Range<usize>) -> bool {
        self.script
            && !self
                .declarations
                .iter()
                .any(|d| d.start <= range.start && d.end >= range.end)
    }
    pub fn contains(&self, range: &Range<usize>) -> bool {
        let contains = |outer: &Range<usize>| outer.start <= range.start && outer.end >= range.end;
        let declaration = self
            .declarations
            .iter()
            .filter(|r| contains(r))
            .min_by_key(|r| r.end - r.start);
        self.entries.iter().any(|entry| {
            contains(entry)
                && declaration.is_none_or(|d| entry.start >= d.start && entry.end <= d.end)
        }) || self.script && declaration.is_none()
    }
}

#[derive(Clone, Copy)]
pub enum SuppressionScope {
    Member,
    Class,
    File,
}
/// Codes come from parsed annotations or an exact directive grammar, never
/// arbitrary words in a comment. Narrow member/line codes stay quiet.
pub fn broad_suppression(scope: SuppressionScope, codes: &[&str]) -> bool {
    matches!(scope, SuppressionScope::Class)
        || codes.is_empty()
        || codes
            .iter()
            .any(|c| c.eq_ignore_ascii_case("all") || *c == "*")
}
pub fn broad_directive(language: &str, comment: &str) -> bool {
    let comment = comment
        .trim()
        .trim_start_matches(['#', '/', '*'])
        .trim()
        .trim_end_matches("*/")
        .trim();
    let comment = comment.split_once("--").map_or(comment, |(c, _)| c).trim();
    match language {
        "c" | "cpp" => {
            if comment.starts_with("NOLINTEND") {
                return false;
            }
            for directive in ["NOLINTNEXTLINE", "NOLINTBEGIN", "NOLINT"] {
                if let Some(tail) = comment.strip_prefix(directive) {
                    if !(tail.is_empty() || tail.starts_with(['(', ' '])) {
                        continue;
                    }
                    let codes: Vec<_> = tail
                        .strip_prefix('(')
                        .and_then(|s| s.split_once(')'))
                        .into_iter()
                        .flat_map(|(s, _)| s.split(','))
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect();
                    return broad_suppression(SuppressionScope::Member, &codes);
                }
            }
            if let Some(tail) = comment
                .strip_prefix("pragma ")
                .and_then(|s| s.strip_prefix("GCC ").or_else(|| s.strip_prefix("clang ")))
                .and_then(|s| s.strip_prefix("diagnostic ignored "))
            {
                // A directive must name a quoted warning code. Prose or an
                // incomplete pragma is not an empty (blanket) suppression.
                if let Some((code, _)) = tail
                    .trim()
                    .strip_prefix('"')
                    .and_then(|s| s.split_once('"'))
                    .filter(|(code, _)| !code.is_empty())
                {
                    return broad_suppression(SuppressionScope::Member, &[code]);
                }
            }
            false
        }
        "csharp" => {
            let Some(tail) = comment.strip_prefix("pragma warning disable") else {
                return false;
            };
            let codes: Vec<_> = tail
                .split("//")
                .next()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            broad_suppression(SuppressionScope::Member, &codes)
        }
        "python" => {
            if let Some(tail) = comment.strip_prefix("noqa") {
                if !(tail.is_empty() || tail.starts_with([':', ' ', '#'])) {
                    return false;
                }
                let codes: Vec<_> = tail
                    .strip_prefix(':')
                    .into_iter()
                    .flat_map(|s| s.split(','))
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()))
                    .collect();
                return broad_suppression(SuppressionScope::Member, &codes);
            }
            if let Some(tail) = comment.strip_prefix("type: ignore") {
                let codes: Vec<_> = tail
                    .strip_prefix('[')
                    .and_then(|t| t.split_once(']'))
                    .into_iter()
                    .flat_map(|(s, _)| s.split(','))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                return broad_suppression(SuppressionScope::Member, &codes);
            }
            false
        }
        "php" => {
            for directive in [
                "phpcs:ignore",
                "phpcs:disable",
                "@phpstan-ignore-next-line",
                "@phpstan-ignore-line",
                "@phpstan-ignore",
            ] {
                if let Some(tail) = comment.strip_prefix(directive) {
                    if !(tail.is_empty() || tail.starts_with(char::is_whitespace)) {
                        continue;
                    }
                    let codes: Vec<_> = tail
                        .trim()
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
                        .collect();
                    return broad_suppression(SuppressionScope::Member, &codes);
                }
            }
            false
        }
        _ => false,
    }
}
