//! Changing a rule, in one place, so that nothing can disagree about what a
//! change means.
//!
//! There are three callers now — `cqx config set`, the desktop application,
//! and `propose_rule` over MCP — and they must agree about four things: which
//! fields a rule has, how a value is spelled in the file, which way the change
//! moves the standard, and what the file looks like afterwards.
//!
//! The first version of this had two of those answers in the same function:
//! the document was edited one way and a copy of the configuration was edited
//! another, and they agreed only because one person wrote both on the same
//! afternoon. That is the kind of agreement that stops being true quietly.
//!
//! So a change is *proposed* first — parsed, validated, classified, and turned
//! into the document that would be written — and only then written. Whether it
//! may be written at all is the caller's decision, because it is a different
//! decision for a person at a terminal than for an agent.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::{defaults, Config, Rule};
use crate::ratchet::{self, Change};

/// A change that has been worked out but not yet written.
#[derive(Debug)]
pub struct Proposal {
    /// Where it would be written.
    pub path: PathBuf,
    /// The rule and field, as given.
    pub rule: String,
    pub field: String,
    /// The value as it will appear in the file.
    pub value: Value,
    /// Every difference this makes to the effective configuration.
    pub changes: Vec<Change>,
    /// Whether every one of them is a tightening. This is the question an
    /// agent is allowed to act on and a person is merely told the answer to.
    pub tightens: bool,
    /// The whole `cqx.json` that would be written.
    pub document: Value,
}

impl Proposal {
    /// The changes that are not tightenings — what a refusal should list.
    pub fn objections(&self) -> Vec<&Change> {
        ratchet::objections(&self.changes)
    }

    /// Write it. Separate from working it out, because whether it may be
    /// written is not this module's decision.
    pub fn write(&self) -> Result<(), String> {
        let mut text = serde_json::to_string_pretty(&self.document)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        text.push('\n');
        std::fs::write(&self.path, text).map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// Work out what setting `rule.field` to `value` would do.
///
/// `path` is the `cqx.json` to change, which may not exist yet. `root` is the
/// repository, used to resolve what is currently in force — which is not the
/// same thing, because a configuration can be inherited from a parent
/// directory and the change still belongs here.
///
/// Nothing is written.
pub fn propose(
    path: &Path,
    root: &Path,
    rule: &str,
    field: &str,
    value: &str,
    why: Option<&str>,
) -> Result<Proposal, String> {
    let defaults = defaults();
    let default_rule = defaults.get(rule).ok_or_else(|| {
        format!("no rule named '{rule}'. `cqx config show` lists them.")
    })?;

    let parsed = parse(default_rule, field, value)?;

    let mut document = read(path)?;
    put(&mut document, rule, field, &parsed, default_rule, why);

    let before = Config::resolve(Some(path), root).or_else(|_| Config::resolve(None, root))?;
    let mut after = before.clone();
    if let Some(r) = after.rules.get_mut(rule) {
        apply(r, field, &parsed);
    }
    let changes = ratchet::compare(&before, &after);

    Ok(Proposal {
        path: path.to_path_buf(),
        rule: rule.to_string(),
        field: field.to_string(),
        value: parsed,
        tightens: ratchet::tightens(&changes),
        changes,
        document,
    })
}

/// Which fields a rule accepts, said once.
pub fn fields(rule: &Rule) -> Vec<String> {
    let mut out: Vec<String> = ["weight", "free", "full", "enabled"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.extend(rule.params.keys().cloned());
    out
}

fn parse(rule: &Rule, field: &str, value: &str) -> Result<Value, String> {
    let known = matches!(field, "weight" | "free" | "full" | "enabled")
        || rule.params.contains_key(field);
    if !known {
        return Err(format!(
            "rule has no field '{field}'. It accepts {}.",
            fields(rule).join(", ")
        ));
    }

    if field == "enabled" {
        return match value {
            "true" | "1" | "yes" => Ok(Value::Bool(true)),
            "false" | "0" | "no" => Ok(Value::Bool(false)),
            other => Err(format!("enabled expects true or false, got '{other}'")),
        };
    }

    match value.parse::<f64>() {
        // A whole number is written as one. This file is read by people as
        // well as parsers, and `max_lines: 2500.0` reads like a mistake.
        Ok(v) if v.fract() == 0.0 && v.abs() < 9e15 => Ok(json!(v as i64)),
        Ok(v) => Ok(json!(v)),
        Err(_) => Err(format!("{field} expects a number, got '{value}'")),
    }
}

/// The existing file, or the shape of a new one.
fn read(path: &Path) -> Result<Value, String> {
    if !path.is_file() {
        return Ok(json!({ "version": 1, "rules": {} }));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

/// Put the field into the document, leaving everything else alone.
fn put(
    document: &mut Value,
    rule: &str,
    field: &str,
    value: &Value,
    default_rule: &Rule,
    why: Option<&str>,
) {
    let Some(rules) = document.as_object_mut().and_then(|o| {
        o.entry("rules")
            .or_insert_with(|| json!({}))
            .as_object_mut()
    }) else {
        return;
    };
    let entry = rules.entry(rule.to_string()).or_insert_with(|| json!({}));
    let Some(object) = entry.as_object_mut() else {
        return;
    };

    if default_rule.params.contains_key(field) {
        if let Some(params) = object
            .entry("params")
            .or_insert_with(|| json!({}))
            .as_object_mut()
        {
            params.insert(field.to_string(), value.clone());
        }
    } else {
        object.insert(field.to_string(), value.clone());
    }

    // The reason, beside the rule, in the file.
    //
    // A threshold somebody finds in a year with no explanation is a threshold
    // nobody dares change, and the person who set it has forgotten. cqx does
    // not read this key — it is not part of `RulePatch`, and serde ignores
    // what it does not know — so it costs nothing but the line it occupies.
    if let Some(why) = why.filter(|w| !w.trim().is_empty()) {
        object.insert("why".into(), json!(why.trim()));
    }
}

/// Apply one field to a rule in memory, exactly as the document applies it.
pub fn apply(rule: &mut Rule, field: &str, value: &Value) {
    match field {
        "enabled" => rule.enabled = value.as_bool().unwrap_or(rule.enabled),
        "weight" => rule.weight = value.as_f64().unwrap_or(rule.weight),
        "free" => rule.free = value.as_f64().unwrap_or(rule.free),
        "full" => rule.full = value.as_f64().unwrap_or(rule.full),
        other => {
            if let Some(v) = value.as_f64() {
                rule.params.insert(other.to_string(), v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratchet::Direction;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cqx-edit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn a_whole_number_is_written_as_one() {
        let dir = sandbox("whole");
        let p = propose(&dir.join("cqx.json"), &dir, "oversized-files", "weight", "40", None).unwrap();
        assert_eq!(p.document["rules"]["oversized-files"]["weight"], json!(40));
        let text = serde_json::to_string(&p.document).unwrap();
        assert!(!text.contains("40.0"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fraction_survives_being_one() {
        let dir = sandbox("fraction");
        let p = propose(&dir.join("cqx.json"), &dir, "oversized-files", "free", "1.5", None).unwrap();
        assert_eq!(p.document["rules"]["oversized-files"]["free"], json!(1.5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_direction_is_the_ratchets_and_not_a_second_opinion() {
        let dir = sandbox("direction");
        let file = dir.join("cqx.json");
        let tighter = propose(&file, &dir, "oversized-files", "free", "1.0", None).unwrap();
        assert!(tighter.tightens);
        assert_eq!(tighter.changes[0].direction, Direction::Tighter);

        // Below `full`, which is 12: one objection, the loosening itself.
        let looser = propose(&file, &dir, "oversized-files", "free", "9.0", None).unwrap();
        assert!(!looser.tightens);
        assert_eq!(looser.objections().len(), 1);

        // Above it, the ramp inverts and that is a second, different problem.
        // Reported as its own objection rather than folded into the first,
        // because "you loosened it" and "this ramp runs backwards" are not the
        // same thing to tell somebody.
        let broken = propose(&file, &dir, "oversized-files", "free", "99.0", None).unwrap();
        assert_eq!(broken.objections().len(), 2, "{:#?}", broken.changes);
        assert!(broken.objections().iter().any(|c| c.why.contains("run backwards")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole reason `propose` and `write` are separate.
    #[test]
    fn proposing_writes_nothing() {
        let dir = sandbox("dry");
        let file = dir.join("cqx.json");
        propose(&file, &dir, "oversized-files", "free", "1.0", None).unwrap();
        assert!(!file.exists(), "a proposal created a file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_then_proposing_again_sees_the_first_change() {
        let dir = sandbox("again");
        let file = dir.join("cqx.json");
        propose(&file, &dir, "oversized-files", "free", "1.0", None)
            .unwrap()
            .write()
            .unwrap();

        // From 1.0, going back to 3.5 is now a loosening — which it would not
        // be if this read the defaults instead of the file it just wrote.
        let back = propose(&file, &dir, "oversized-files", "free", "3.5", None).unwrap();
        assert!(!back.tightens, "{:#?}", back.changes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_else_in_the_file_is_disturbed() {
        let dir = sandbox("keep");
        let file = dir.join("cqx.json");
        std::fs::write(
            &file,
            r#"{"version":1,"min_score":70,"exclude":["vendor/"],"rules":{"exit-in-library":{"weight":50}},"somethingNew":42}"#,
        )
        .unwrap();

        let p = propose(&file, &dir, "oversized-files", "weight", "40", None).unwrap();
        assert_eq!(p.document["min_score"], json!(70));
        assert_eq!(p.document["exclude"], json!(["vendor/"]));
        assert_eq!(p.document["rules"]["exit-in-library"]["weight"], json!(50));
        assert_eq!(p.document["somethingNew"], json!(42));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reason_is_kept_beside_the_rule_and_does_not_break_reading_it() {
        let dir = sandbox("why");
        let file = dir.join("cqx.json");
        propose(
            &file,
            &dir,
            "oversized-files",
            "weight",
            "40",
            Some("  we split files at review anyway  "),
        )
        .unwrap()
        .write()
        .unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("we split files at review anyway"), "{text}");
        // And cqx must still read the file it just wrote — `why` is not a
        // field `RulePatch` knows, and serde must be ignoring it rather than
        // refusing the document.
        let config = Config::resolve(Some(&file), &dir).expect("cqx can still read it");
        assert_eq!(config.rules["oversized-files"].weight, 40.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_rule_or_field_is_refused_before_anything_is_read() {
        let dir = sandbox("unknown");
        let file = dir.join("cqx.json");
        let e = propose(&file, &dir, "no-such-rule", "weight", "1", None).unwrap_err();
        assert!(e.contains("no rule named"), "{e}");
        let e = propose(&file, &dir, "oversized-files", "colour", "1", None).unwrap_err();
        assert!(e.contains("no field 'colour'"), "{e}");
        assert!(e.contains("max_lines"), "it should say what the rule accepts: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enabled_takes_words_and_refuses_nonsense() {
        let dir = sandbox("enabled");
        let file = dir.join("cqx.json");
        for word in ["true", "1", "yes"] {
            let p = propose(&file, &dir, "oversized-files", "enabled", word, None).unwrap();
            assert_eq!(p.value, json!(true));
        }
        let e = propose(&file, &dir, "oversized-files", "enabled", "maybe", None).unwrap_err();
        assert!(e.contains("expects true or false"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
