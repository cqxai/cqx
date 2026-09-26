//! The job summary: what somebody sees without opening anything.
//!
//! GitHub renders `$GITHUB_STEP_SUMMARY` on the run's own page, above the log.
//! It is the one surface a reviewer reaches by accident, so it carries the
//! scores and the movement and nothing else — the findings are already on the
//! lines they belong to, and repeating them here is how a summary becomes a
//! thing people collapse.

use serde_json::Value;

use crate::Scanned;

pub fn build(here: &Scanned, against: Option<(&str, &Scanned)>) -> String {
    let mut out = String::new();
    out.push_str("## CodeQuality Score\n\n");

    let was: std::collections::BTreeMap<String, u64> = against
        .map(|(_, s)| s.scores().into_iter().collect())
        .unwrap_or_default();

    if let Some((reference, _)) = against {
        out.push_str(&format!("| | score | against `{reference}` |\n|---|--:|--:|\n"));
    } else {
        out.push_str("| | score |\n|---|--:|\n");
    }

    for (category, value) in here.scores() {
        let mark = if value >= 90 {
            "🟢"
        } else if value >= 50 {
            "🟡"
        } else {
            "🔴"
        };
        match was.get(&category) {
            Some(then) if *then == value => {
                out.push_str(&format!("| {mark} {category} | **{value}** | — |\n"));
            }
            Some(then) if value > *then => {
                out.push_str(&format!("| {mark} {category} | **{value}** | +{} |\n", value - then));
            }
            Some(then) => {
                out.push_str(&format!(
                    "| {mark} {category} | **{value}** | **−{}** |\n",
                    then - value
                ));
            }
            None => out.push_str(&format!("| {mark} {category} | **{value}** | |\n")),
        }
    }

    out.push_str(&format!(
        "\n{} files · {} lines · {:.1}s · cqx {}\n",
        here.files,
        here.lines,
        here.ms as f64 / 1000.0,
        env!("CARGO_PKG_VERSION")
    ));

    // The rules that cost something, worst first. A reader who wants the
    // places opens the Files changed tab, where they already are.
    let empty = Vec::new();
    let rules = here.report.get("rules").and_then(Value::as_array).unwrap_or(&empty);
    let mut costly: Vec<&Value> = rules
        .iter()
        .filter(|r| r.get("deducted").and_then(Value::as_f64).unwrap_or(0.0) > 0.0)
        .collect();
    costly.sort_by(|a, b| {
        let f = |v: &Value| v.get("deducted").and_then(Value::as_f64).unwrap_or(0.0);
        f(b).total_cmp(&f(a))
    });

    if !costly.is_empty() {
        out.push_str("\n<details><summary>What it cost</summary>\n\n| rule | findings | cost |\n|---|--:|--:|\n");
        for rule in costly.iter().take(12) {
            let id = format!(
                "{}/{}",
                rule.get("language").and_then(Value::as_str).unwrap_or("rust"),
                rule.get("rule").and_then(Value::as_str).unwrap_or("?"),
            );
            out.push_str(&format!(
                "| [`{id}`]({}) | {} | −{:.1} |\n",
                crate::RULES_DOC,
                rule.get("total_findings").and_then(Value::as_u64).unwrap_or(0),
                rule.get("deducted").and_then(Value::as_f64).unwrap_or(0.0),
            ));
        }
        out.push_str("\n</details>\n");
    }

    out
}
