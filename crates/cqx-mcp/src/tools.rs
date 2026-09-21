//! The four tools an agent actually calls.
//!
//! Each result is `{ content: [{ type: "text", text: <pretty JSON> }] }`.
//! A tool that fails answers the same way with `isError: true` and a
//! sentence a person could read — not a JSON-RPC error, because a tool
//! failing is a result, not a protocol fault.

use serde_json::{json, Value};

use crate::{Server, Trouble};

pub(crate) fn list() -> Value {
    json!({
        "tools": [
            {
                "name": "score",
                "description": "Category scores for the tree on disk, plus how long the scan took.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_prop(),
                    },
                },
                "annotations": { "readOnlyHint": true },
            },
            {
                "name": "findings",
                "description": "The findings. Optional rule and file filters. Each finding includes its rule and category.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_prop(),
                        "rule": {
                            "type": "string",
                            "description": "Only findings from this rule.",
                        },
                        "file": {
                            "type": "string",
                            "description": "Only findings in this file.",
                        },
                    },
                },
                "annotations": { "readOnlyHint": true },
            },
            {
                "name": "rules",
                "description": "Every rule in force: what it measures, what to do about it, its weight, and whether it is deducting anything.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_prop(),
                    },
                },
                "annotations": { "readOnlyHint": true },
            },
            {
                "name": "propose_rule",
                "description": "Raise the standard: change a rule, but only in the direction that makes it stricter. A change that would lower it is refused and the reason is returned — only a person may loosen a rule. Say why; the reason is kept beside the rule in cqx.json.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_prop(),
                        "rule": {
                            "type": "string",
                            "description": "The rule to change, for example oversized-files.",
                        },
                        "field": {
                            "type": "string",
                            "description": "weight, free, full, enabled, or one of the rule's own parameters. Call explain to see them.",
                        },
                        "value": {
                            "description": "The new value. A number, or true/false for enabled.",
                        },
                        "because": {
                            "type": "string",
                            "description": "Why this rule should be stricter. Kept in cqx.json beside the rule, for whoever reads it next.",
                        },
                    },
                    "required": ["rule", "field", "value", "because"],
                },
                // Not read-only, and it says so. It writes exactly one file,
                // `cqx.json`, and only ever in the stricter direction — which
                // is why it is not destructive either: the worst it can do is
                // hold this repository to a higher standard than somebody
                // wanted, and a person can undo that in one command.
                "annotations": {
                    "readOnlyHint": false,
                    "destructiveHint": false,
                    "idempotentHint": true,
                },
            },
            {
                "name": "explain",
                "description": "One rule, named, in full, plus its findings in this repository.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": path_prop(),
                        "rule": {
                            "type": "string",
                            "description": "The rule to explain, for example exit-in-library.",
                        },
                    },
                    "required": ["rule"],
                },
                "annotations": { "readOnlyHint": true },
            },
        ],
    })
}

fn path_prop() -> Value {
    json!({
        "type": "string",
        "description": "Repository root. Defaults to the current working directory.",
    })
}

pub(crate) fn call(server: &mut Server, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match name {
        "score" => wrap(score(server, &args)),
        "findings" => wrap(findings(server, &args)),
        "rules" => wrap(rules(server, &args)),
        "explain" => wrap(explain(server, &args)),
        "propose_rule" => wrap(propose_rule(server, &args)),
        "" => fail("tools/call needs the name of a tool."),
        other => fail(format!(
            "there is no tool named '{other}'. cqx mcp has score, findings, rules, explain and propose_rule."
        )),
    }
}

fn score(server: &mut Server, args: &Value) -> Result<Value, Trouble> {
    let report = server.report_for(args)?;
    Ok(json!({
        "scores": report.get("scores").cloned().unwrap_or(json!({})),
        "scan": report.get("scan").cloned().unwrap_or(json!({})),
    }))
}

fn findings(server: &mut Server, args: &Value) -> Result<Value, Trouble> {
    let report = server.report_for(args)?;
    let rule_filter = args.get("rule").and_then(Value::as_str);
    let file_filter = args.get("file").and_then(Value::as_str);
    Ok(json!({ "findings": collect_findings(&report, rule_filter, file_filter) }))
}

fn rules(server: &mut Server, args: &Value) -> Result<Value, Trouble> {
    let report = server.report_for(args)?;
    Ok(json!({ "rules": collect_rules(&report) }))
}

fn explain(server: &mut Server, args: &Value) -> Result<Value, Trouble> {
    let want = args
        .get("rule")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(Trouble::Missing("explain needs the name of a rule."))?;
    let report = server.report_for(args)?;
    if let Some(rule) = rule_in_report(&report, want) {
        return Ok(describe(rule, want));
    }
    // A rule can be in the configuration and still absent from the report
    // (disabled, or this tree produced no measurement). The agent asked
    // for it by name, so the description is still the answer.
    if let Some(rule) = rule_in_config(&report, want) {
        return Ok(json!({
            "name": want,
            "category": rule.get("category").cloned().unwrap_or(json!("")),
            "describes": rule.get("describes").cloned().unwrap_or(json!("")),
            "remedy": rule.get("remedy").cloned().unwrap_or(json!("")),
            "weight": rule.get("weight").cloned().unwrap_or(json!(0)),
            "deducting": false,
            "findings": [],
        }));
    }
    Err(Trouble::NoSuchRule(want.to_string()))
}

/// Raise the standard, and only ever raise it.
///
/// This is the one thing in `cqx mcp` that writes, and the asymmetry is the
/// reason it is allowed to. **An agent may tighten a rule; only a person may
/// loosen one.** Without that, "make the score go up" has two solutions —
/// write better code, or lower the bar — and the second is faster, always
/// available, and looks identical in a diff to anybody skimming.
///
/// It is also why this is safe rather than merely guarded. Tightening a rule
/// is never in an agent's short-term interest: it makes the score it is being
/// measured by harder to reach. What it is good for is the thing a reviewer
/// currently does by hand — noticing that a standard should be higher, and
/// saying so once instead of correcting the same thing every week.
///
/// `because` is required. A threshold somebody finds in a year with no
/// explanation is a threshold nobody dares change, and it is written into
/// `cqx.json` beside the rule rather than into a log nobody keeps.
fn propose_rule(server: &mut Server, args: &Value) -> Result<Value, Trouble> {
    let rule = text(args, "rule", "propose_rule needs the name of a rule.")?;
    let field = text(args, "field", "propose_rule needs the name of a field.")?;
    let because = text(
        args,
        "because",
        "propose_rule needs `because`: why this rule should be stricter. It is kept in cqx.json beside the rule.",
    )?;
    let value = match args.get("value") {
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => {
            return Err(Trouble::Missing(
                "propose_rule needs a value: a number, or true/false for enabled.",
            ))
        }
    };

    let root = crate::root_from(args)?.canonicalize().map_err(|e| Trouble::Unreadable {
        path: crate::root_from(args).unwrap_or_default(),
        why: e.to_string(),
    })?;

    let proposal = cqx_score::edit::propose(
        &root.join("cqx.json"),
        &root,
        &rule,
        &field,
        &value,
        Some(&because),
    )
    .map_err(Trouble::Analysis)?;

    if !proposal.tightens {
        let objections = proposal.objections();
        return Err(Trouble::NotATightening {
            rule: rule.clone(),
            field: field.clone(),
            loosening: objections
                .iter()
                .any(|c| c.direction == cqx_score::ratchet::Direction::Looser),
            objections: objections
                .iter()
                .map(|c| format!("{} {} → {}: {}", c.field, c.before, c.after, c.why))
                .collect(),
            nothing: proposal.changes.is_empty(),
        });
    }

    proposal.write().map_err(Trouble::Analysis)?;
    // The next question after changing a rule is what it did to the score, and
    // the cached scan was computed under the old rules.
    server.forget();

    Ok(json!({
        "written": proposal.path.display().to_string(),
        "rule": rule,
        "field": field,
        "value": proposal.value,
        "because": because,
        "changes": proposal.changes,
        "note": "The standard is now stricter. Re-run score to see what it cost.",
    }))
}

fn text(args: &Value, key: &str, missing: &'static str) -> Result<String, Trouble> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or(Trouble::Missing(missing))
}

fn describe(rule: &Value, want: &str) -> Value {
    let deducted = rule.get("deducted").and_then(Value::as_f64).unwrap_or(0.0);
    json!({
        "name": rule.get("rule").and_then(Value::as_str).unwrap_or(want),
        "category": rule.get("category").cloned().unwrap_or(json!("")),
        "describes": rule.get("describes").cloned().unwrap_or(json!("")),
        "remedy": rule.get("remedy").cloned().unwrap_or(json!("")),
        "weight": rule.get("weight").cloned().unwrap_or(json!(0)),
        "deducting": deducted > 0.0,
        "findings": rule.get("findings").cloned().unwrap_or(json!([])),
    })
}

fn collect_rules(report: &Value) -> Vec<Value> {
    let empty = Vec::new();
    report
        .get("rules")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .map(|rule| {
            let deducted = rule.get("deducted").and_then(Value::as_f64).unwrap_or(0.0);
            json!({
                "name": rule.get("rule").and_then(Value::as_str).unwrap_or(""),
                "category": rule.get("category").and_then(Value::as_str).unwrap_or(""),
                "describes": rule.get("describes").and_then(Value::as_str).unwrap_or(""),
                "remedy": rule.get("remedy").and_then(Value::as_str).unwrap_or(""),
                "weight": rule.get("weight").cloned().unwrap_or(json!(0)),
                "deducting": deducted > 0.0,
            })
        })
        .collect()
}

fn collect_findings(
    report: &Value,
    rule_filter: Option<&str>,
    file_filter: Option<&str>,
) -> Vec<Value> {
    let empty = Vec::new();
    let mut out = Vec::new();
    for rule in report
        .get("rules")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        let name = rule.get("rule").and_then(Value::as_str).unwrap_or("");
        if let Some(want) = rule_filter {
            if !same_rule(name, want) {
                continue;
            }
        }
        let category = rule.get("category").and_then(Value::as_str).unwrap_or("");
        for finding in rule
            .get("findings")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            let file = finding.get("file").and_then(Value::as_str).unwrap_or("");
            if let Some(want) = file_filter {
                if !file_matches(file, want) {
                    continue;
                }
            }
            // Rule and category travel on the finding. A flat list an
            // agent has to re-associate with the rules array is a list
            // it will get wrong.
            let mut one = finding.clone();
            if let Some(obj) = one.as_object_mut() {
                obj.insert("rule".into(), json!(name));
                obj.insert("category".into(), json!(category));
            }
            out.push(one);
        }
    }
    out
}

fn rule_in_report<'a>(report: &'a Value, want: &str) -> Option<&'a Value> {
    report
        .get("rules")
        .and_then(Value::as_array)?
        .iter()
        .find(|rule| {
            rule.get("rule")
                .and_then(Value::as_str)
                .is_some_and(|name| same_rule(name, want))
        })
}

fn rule_in_config<'a>(report: &'a Value, want: &str) -> Option<&'a Value> {
    let rules = report.pointer("/config/rules")?.as_object()?;
    rules.get(want).or_else(|| {
        rules
            .iter()
            .find_map(|(name, rule)| same_rule(name, want).then_some(rule))
    })
}

fn same_rule(have: &str, want: &str) -> bool {
    have == want
        || want.rsplit_once('/').is_some_and(|(_, name)| name == have)
        || have.rsplit_once('/').is_some_and(|(_, name)| name == want)
}

fn file_matches(file: &str, filter: &str) -> bool {
    let file = file.replace('\\', "/");
    let filter = filter.replace('\\', "/");
    file == filter || file.ends_with(&format!("/{filter}"))
}

fn wrap(result: Result<Value, Trouble>) -> Value {
    match result {
        Ok(value) => ok(value),
        // The sentence an agent reads is the error's own Display. One place
        // decides how each kind of trouble is worded.
        Err(trouble) => fail(trouble.to_string()),
    }
}

fn ok(value: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
        }],
    })
}

fn fail(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true,
    })
}
