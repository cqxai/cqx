//! Which way a change to the rules moves, and who is allowed to move it.
//!
//! The asymmetry this file exists for: **an agent may tighten a constraint;
//! only a person may loosen one.**
//!
//! It is the whole argument for letting an agent touch `cqx.json` at all.
//! Without it, "make the score go up" has two solutions — write better code,
//! or lower the bar — and the second is faster, always available, and looks
//! identical in a diff to somebody skimming. An agent that can only ratchet
//! has one solution. The standard becomes a thing that can be raised by
//! whoever notices it should be, and lowered only by somebody who has decided
//! to lower it and signed their name to it.
//!
//! That is also why this is not a policy in whatever code happens to call it.
//! It is one function, here, beside the rules it classifies, because a second
//! opinion about which direction is which is the bug that makes the guarantee
//! worthless.
//!
//! ## Conservative on purpose
//!
//! Anything this cannot *prove* is a tightening is [`Direction::Unclear`], and
//! unclear is refused for an agent exactly as a loosening is. A rule parameter
//! is the clearest case: `max_lines` going from 1000 to 800 is stricter and
//! `min_matches` going from 3 to 2 is also stricter, but the direction depends
//! on what the parameter means, and this file knows only its name. Guessing
//! would be right most of the time, and a guarantee that is right most of the
//! time is not one.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::{Config, Rule};

/// Which way a single change moves the standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Strictly harder to pass. An agent may make this change.
    Tighter,
    /// Strictly easier to pass. A person may make this change; an agent may not.
    Looser,
    /// Could be either, or is not a standard at all. Treated as a loosening,
    /// because a guarantee that holds only when the answer is obvious is not a
    /// guarantee.
    Unclear,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Direction::Tighter => "tighter",
            Direction::Looser => "looser",
            Direction::Unclear => "unclear",
        })
    }
}

/// One field, moved.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    /// The rule, or `""` for a change to the configuration as a whole.
    pub rule: String,
    /// `enabled`, `weight`, `free`, `full`, `params.max_lines`, `min_score`,
    /// `exclude`.
    pub field: String,
    pub before: String,
    pub after: String,
    pub direction: Direction,
    /// Why it is that direction, in a clause. Written for the person reading a
    /// refusal, who is owed a reason rather than a verdict.
    pub why: String,
}

/// Every difference between two configurations, classified.
///
/// Order is stable — configuration first, then rules alphabetically, then
/// fields in the order they are declared — so two runs over the same pair
/// produce the same list and a test can assert on it.
pub fn compare(before: &Config, after: &Config) -> Vec<Change> {
    let mut out = Vec::new();

    // The floor. Raising it is a tightening even though it changes no rule:
    // it is the number a run is refused for falling under.
    match (before.min_score, after.min_score) {
        // A guard does not count towards exhaustiveness, so the unchanged case
        // is spelled out rather than leaned on.
        (None, None) => {}
        (a, b) if a == b => {}
        (None, Some(n)) => out.push(Change {
            rule: String::new(),
            field: "min_score".into(),
            before: "none".into(),
            after: n.to_string(),
            direction: Direction::Tighter,
            why: "a run that could not fail on score now can".into(),
        }),
        (Some(n), None) => out.push(Change {
            rule: String::new(),
            field: "min_score".into(),
            before: n.to_string(),
            after: "none".into(),
            direction: Direction::Looser,
            why: "removing the floor means no score can fail a run".into(),
        }),
        (Some(a), Some(b)) => out.push(Change {
            rule: String::new(),
            field: "min_score".into(),
            before: a.to_string(),
            after: b.to_string(),
            direction: if b > a { Direction::Tighter } else { Direction::Looser },
            why: if b > a { "a higher floor refuses more" } else { "a lower floor refuses less" }.into(),
        }),
    }

    // Exclusions. Code that is not measured cannot fail, so every path added
    // here is a loosening however reasonable it is — vendored code and
    // generated files are exactly the sort of thing a person should exclude,
    // and exactly the sort of thing an agent should not be able to.
    for path in after.exclude.iter().filter(|p| !before.exclude.contains(*p)) {
        out.push(Change {
            rule: String::new(),
            field: "exclude".into(),
            before: "measured".into(),
            after: path.clone(),
            direction: Direction::Looser,
            why: "excluded code is not scored, so nothing in it can fail".into(),
        });
    }
    for path in before.exclude.iter().filter(|p| !after.exclude.contains(*p)) {
        out.push(Change {
            rule: String::new(),
            field: "exclude".into(),
            before: path.clone(),
            after: "measured".into(),
            direction: Direction::Tighter,
            why: "code that was not scored now is".into(),
        });
    }

    let names: BTreeMap<&String, ()> = before
        .rules
        .keys()
        .chain(after.rules.keys())
        .map(|k| (k, ()))
        .collect();

    for name in names.into_keys() {
        match (before.rules.get(name), after.rules.get(name)) {
            (Some(a), Some(b)) => rule(name, a, b, &mut out),
            // A rule that did not exist and now does. Enabled, that is more
            // checking; disabled, it changes nothing and is not reported.
            (None, Some(b)) if b.enabled => out.push(Change {
                rule: name.clone(),
                field: "rule".into(),
                before: "absent".into(),
                after: "enabled".into(),
                direction: Direction::Tighter,
                why: "a rule that was not being applied now is".into(),
            }),
            (None, Some(_)) => {}
            // A rule that has gone. It may fall back to a stricter default or
            // to nothing at all, and which of those it is depends on a table
            // this function was not given.
            (Some(_), None) => out.push(Change {
                rule: name.clone(),
                field: "rule".into(),
                before: "present".into(),
                after: "absent".into(),
                direction: Direction::Unclear,
                why: "a removed rule falls back to a default this cannot see".into(),
            }),
            (None, None) => unreachable!("a name came from one of the two maps"),
        }
    }

    out
}

fn rule(name: &str, a: &Rule, b: &Rule, out: &mut Vec<Change>) {
    let mut push = |field: &str, before: String, after: String, direction, why: &str| {
        out.push(Change {
            rule: name.to_string(),
            field: field.into(),
            before,
            after,
            direction,
            why: why.into(),
        });
    };

    if a.enabled != b.enabled {
        push(
            "enabled",
            a.enabled.to_string(),
            b.enabled.to_string(),
            if b.enabled { Direction::Tighter } else { Direction::Looser },
            if b.enabled { "a rule that was off is now on" } else { "a rule that was on is now off" },
        );
    }

    // Weight is the most this rule can ever deduct, so more of it is more at
    // stake. Note that a rule which is off deducts nothing whatever its weight
    // — but the weight is still the standard, and raising it while off is
    // still raising it.
    if a.weight != b.weight {
        push(
            "weight",
            a.weight.to_string(),
            b.weight.to_string(),
            if b.weight > a.weight { Direction::Tighter } else { Direction::Looser },
            if b.weight > a.weight { "more points are at stake" } else { "fewer points are at stake" },
        );
    }

    // `free` is how much costs nothing. Less of it is less tolerance.
    if a.free != b.free {
        push(
            "free",
            a.free.to_string(),
            b.free.to_string(),
            if b.free < a.free { Direction::Tighter } else { Direction::Looser },
            if b.free < a.free { "less is tolerated for free" } else { "more is tolerated for free" },
        );
    }

    // `full` is where the whole weight is deducted. Reaching it sooner is
    // steeper.
    if a.full != b.full {
        push(
            "full",
            a.full.to_string(),
            b.full.to_string(),
            if b.full < a.full { Direction::Tighter } else { Direction::Looser },
            if b.full < a.full { "the full deduction arrives sooner" } else { "the full deduction arrives later" },
        );
    }

    // A ramp that runs backwards is not a stricter ramp, it is a broken one,
    // and the scorer's behaviour on it is not something to reason about from
    // here. Said as its own change so a refusal names the actual problem.
    if b.free > b.full {
        push(
            "free",
            format!("{} ≤ {}", a.free, a.full),
            format!("{} > {}", b.free, b.full),
            Direction::Unclear,
            "free must not exceed full; the ramp would run backwards",
        );
    }

    // Parameters. The direction depends on what the parameter means, and only
    // the rule knows that — see the note at the top of this file.
    let keys: BTreeMap<&String, ()> = a.params.keys().chain(b.params.keys()).map(|k| (k, ())).collect();
    for key in keys.into_keys() {
        let (was, now) = (a.params.get(key), b.params.get(key));
        if was == now {
            continue;
        }
        push(
            &format!("params.{key}"),
            was.map(f64::to_string).unwrap_or_else(|| "unset".into()),
            now.map(f64::to_string).unwrap_or_else(|| "unset".into()),
            Direction::Unclear,
            "which way a parameter tightens depends on what it measures",
        );
    }

    // Moving a rule between categories changes which score it affects, and
    // therefore which category could fail. Not a tightening of anything.
    if a.category != b.category {
        push(
            "category",
            a.category.clone(),
            b.category.clone(),
            Direction::Unclear,
            "the deduction moves to a different score",
        );
    }
}

/// Whether a proposal is one an agent may apply on its own.
///
/// True only when something changed and every change is a tightening. An empty
/// proposal is not a ratchet — it is a no-op, and answering "yes, apply it"
/// to a change that changes nothing invites a caller to stop checking.
pub fn tightens(changes: &[Change]) -> bool {
    !changes.is_empty() && changes.iter().all(|c| c.direction == Direction::Tighter)
}

/// The ones that are not tightenings, which is what a refusal should list.
pub fn objections(changes: &[Change]) -> Vec<&Change> {
    changes.iter().filter(|c| c.direction != Direction::Tighter).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::defaults;

    fn config() -> Config {
        Config {
            rules: defaults(),
            origins: BTreeMap::new(),
            min_score: None,
            exclude: Vec::new(),
            loaded_from: None,
        }
    }

    /// The rule everything below is measured against: a real one, with a real
    /// ramp, so the directions are being decided about a shape that exists.
    const RULE: &str = "oversized-files";

    fn tweak(f: impl FnOnce(&mut Rule)) -> Config {
        let mut after = config();
        f(after.rules.get_mut(RULE).expect("a rule that ships by default"));
        after
    }

    fn one(after: &Config) -> Change {
        let changes = compare(&config(), after);
        assert_eq!(changes.len(), 1, "expected one change, got {changes:#?}");
        changes.into_iter().next().unwrap()
    }

    #[test]
    fn nothing_changed_is_not_a_ratchet() {
        let changes = compare(&config(), &config());
        assert!(changes.is_empty());
        // The important half: a no-op must not be approvable, or a caller that
        // checks `tightens` and nothing else can be handed an empty proposal.
        assert!(!tightens(&changes));
    }

    #[test]
    fn less_tolerance_is_tighter() {
        let change = one(&tweak(|r| r.free /= 2.0));
        assert_eq!(change.direction, Direction::Tighter);
        assert_eq!(change.field, "free");
    }

    #[test]
    fn more_tolerance_is_looser() {
        assert_eq!(one(&tweak(|r| r.free *= 2.0)).direction, Direction::Looser);
    }

    #[test]
    fn a_steeper_ramp_is_tighter_and_a_shallower_one_is_not() {
        assert_eq!(one(&tweak(|r| r.full /= 2.0)).direction, Direction::Tighter);
        assert_eq!(one(&tweak(|r| r.full *= 2.0)).direction, Direction::Looser);
    }

    #[test]
    fn more_at_stake_is_tighter() {
        assert_eq!(one(&tweak(|r| r.weight += 10.0)).direction, Direction::Tighter);
        assert_eq!(one(&tweak(|r| r.weight -= 10.0)).direction, Direction::Looser);
    }

    #[test]
    fn turning_a_rule_on_is_tighter_and_off_is_not() {
        let off = tweak(|r| r.enabled = false);
        assert_eq!(one(&off).direction, Direction::Looser);
        // And the other way round, from that config back to the default.
        let back = compare(&off, &config());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].direction, Direction::Tighter);
        assert!(tightens(&back));
    }

    #[test]
    fn a_parameter_is_never_assumed() {
        // Smaller, which for `max_lines` is stricter — and this must still
        // refuse, because the next parameter will not be `max_lines`.
        let change = one(&tweak(|r| {
            r.params.insert("max_lines".into(), 400.0);
        }));
        assert_eq!(change.direction, Direction::Unclear);
        assert_eq!(change.field, "params.max_lines");
        assert!(!tightens(std::slice::from_ref(&change)));
    }

    #[test]
    fn a_backwards_ramp_is_refused_even_though_both_halves_tighten() {
        // free 3.5 → 8.0 would be looser on its own; full 12 → 2 is tighter on
        // its own; together they invert the ramp. The point of the test is
        // that the invalid shape is reported rather than netting out.
        let after = tweak(|r| {
            r.free = 8.0;
            r.full = 2.0;
        });
        let changes = compare(&config(), &after);
        assert!(!tightens(&changes));
        assert!(
            changes.iter().any(|c| c.direction == Direction::Unclear
                && c.why.contains("run backwards")),
            "{changes:#?}"
        );
    }

    #[test]
    fn excluding_code_is_looser_however_reasonable_it_is() {
        let mut after = config();
        after.exclude.push("vendor/".into());
        let change = one(&after);
        assert_eq!(change.direction, Direction::Looser);
        assert_eq!(change.field, "exclude");

        // And removing an exclusion is a tightening.
        let back = compare(&after, &config());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].direction, Direction::Tighter);
    }

    #[test]
    fn raising_the_floor_is_tighter_and_removing_it_is_not() {
        let mut floored = config();
        floored.min_score = Some(70);
        assert_eq!(one(&floored).direction, Direction::Tighter);

        let mut higher = config();
        higher.min_score = Some(80);
        assert_eq!(compare(&floored, &higher)[0].direction, Direction::Tighter);
        assert_eq!(compare(&higher, &floored)[0].direction, Direction::Looser);
        assert_eq!(compare(&floored, &config())[0].direction, Direction::Looser);
    }

    #[test]
    fn a_tightening_hidden_among_loosenings_is_still_refused() {
        // The shape somebody would actually try: three real improvements and
        // one quiet exclusion. `tightens` is an all, not an any, and this is
        // the test that says so.
        let mut after = config();
        for name in ["oversized-files", "bare-string-params", "duplicated-bodies"] {
            after.rules.get_mut(name).unwrap().free = 0.0;
        }
        after.exclude.push("crates/legacy/".into());

        let changes = compare(&config(), &after);
        assert!(!tightens(&changes));
        let refused = objections(&changes);
        assert_eq!(refused.len(), 1, "only the exclusion should be objected to");
        assert_eq!(refused[0].field, "exclude");
    }

    #[test]
    fn every_default_rule_can_be_tightened_and_the_classification_holds() {
        // Not a spot check: every rule that ships, so a rule added later with
        // an odd shape cannot quietly escape the classification.
        for name in defaults().keys() {
            let mut after = config();
            let r = after.rules.get_mut(name).unwrap();
            r.weight += 1.0;
            let changes = compare(&config(), &after);
            assert!(tightens(&changes), "{name} could not be tightened: {changes:#?}");
        }
    }
}
