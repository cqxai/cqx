//! Findings as SARIF, which is how GitHub shows them on the line they are on.
//!
//! A pull request comment is the thing everybody builds first and everybody
//! learns to scroll past. SARIF goes somewhere else: uploaded by
//! `github/codeql-action/upload-sarif`, each finding appears in the Files
//! changed tab beside the code, and the ones that were already there stay out
//! of the way. It also outlives GitHub — the format is an OASIS standard, and
//! GitLab and the editors read it too.
//!
//! Only what the schema requires and what a reader gains from. A SARIF file
//! can carry taxonomies, graphs and code flows; none of them would say
//! anything cqx knows that the location and the rule do not.

use std::path::Path;

use serde_json::{json, Value};

const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

/// How loudly a rule speaks *in this repository*.
///
/// cqx has no severity field, and its weights are close enough to each other
/// that mapping them directly made every finding an error — which is the same
/// as having no levels at all. What separates them is how much of its weight
/// a rule actually took here: a rule at its cap is the reason a category
/// dropped, and one that cost nothing is context.
fn level(weight: f64, deducted: f64) -> &'static str {
    if weight <= 0.0 || deducted <= 0.0 {
        return "note";
    }
    if deducted / weight >= 0.5 {
        "error"
    } else {
        "warning"
    }
}

pub fn build(report: &Value, root: &Path, scanned: &[String]) -> Value {
    let empty = Vec::new();
    let rules = report
        .get("rules")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let mut descriptors: Vec<Value> = Vec::new();
    let mut results: Vec<Value> = Vec::new();

    for rule in rules {
        let name = rule.get("rule").and_then(Value::as_str).unwrap_or("");
        let language = rule.get("language").and_then(Value::as_str).unwrap_or("rust");
        let weight = rule.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
        let deducted = rule.get("deducted").and_then(Value::as_f64).unwrap_or(0.0);
        let level = level(weight, deducted);
        let category = rule.get("category").and_then(Value::as_str).unwrap_or("");
        let describes = rule.get("describes").and_then(Value::as_str).unwrap_or("");
        let remedy = rule.get("remedy").and_then(Value::as_str).unwrap_or("");
        // The id a reader will see, look up and eventually configure. The same
        // spelling the explorer shows and the docs will use, so all three
        // agree without anybody translating.
        let id = format!("{language}/{name}");

        descriptors.push(json!({
            "id": id,
            "name": name,
            "shortDescription": { "text": describes },
            "fullDescription": { "text": if remedy.is_empty() { describes } else { remedy } },
            "help": {
                "text": remedy,
                "markdown": format!("{remedy}\n\n[{id}](https://docs.cqx.dev/{id})"),
            },
            "helpUri": format!("https://docs.cqx.dev/{id}"),
            "defaultConfiguration": { "level": level },
            "properties": { "tags": [category], "weight": weight },
        }));

        let findings = rule
            .get("findings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for finding in &findings {
            let file = finding.get("file").and_then(Value::as_str).unwrap_or("");
            // A finding whose `file` is a symbol rather than a path has no
            // place in a diff, and a location that does not resolve is worse
            // than none: GitHub drops the whole run rather than the result.
            if file.is_empty() || !root.join(file).is_file() {
                continue;
            }
            let line = finding.get("line").and_then(Value::as_u64).unwrap_or(0);
            let what = finding.get("what").and_then(Value::as_str).unwrap_or("");

            let mut region = json!({ "startLine": line.max(1) });
            if let Some(col) = finding.get("col").and_then(Value::as_array) {
                let from = col.first().and_then(Value::as_u64).unwrap_or(0);
                let to = col.get(1).and_then(Value::as_u64).unwrap_or(0);
                if to > from {
                    // SARIF counts columns from one; the extractor counts from
                    // zero, and `endColumn` is exclusive in both.
                    region["startColumn"] = json!(from + 1);
                    region["endColumn"] = json!(to + 1);
                }
            }
            if let Some(text) = finding.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    region["snippet"] = json!({ "text": text });
                }
            }

            results.push(json!({
                "ruleId": id,
                "level": level,
                "message": { "text": what },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                        "region": region,
                    }
                }],
                // What makes a finding the same finding after the lines around
                // it move. Without it, adding an import at the top of a file
                // closes every alert in it and opens them all again.
                "partialFingerprints": {
                    "cqx/v1": format!("{id}:{file}:{what}"),
                },
            }));
        }
    }

    // Every file the scan read, in SARIF's own vocabulary. Without it the
    // code scanning page says "no summary of scanned files reported by cqx",
    // which reads as a tool that did not look rather than one that found
    // nothing here.
    let artifacts: Vec<Value> = scanned
        .iter()
        .map(|path| json!({ "location": { "uri": path }, "roles": ["analysisTarget"] }))
        .collect();

    json!({
        "$schema": SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "cqx",
                "semanticVersion": env!("CARGO_PKG_VERSION"),
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://cqx.dev",
                "rules": descriptors,
            }},
            "artifacts": artifacts,
            "results": results,
            // Results are for the whole tree, not only what the pull request
            // touched. Saying so is what stops GitHub treating the ones it did
            // not receive this time as fixed.
            "properties": { "scope": "whole-tree" },
        }],
    })
}
