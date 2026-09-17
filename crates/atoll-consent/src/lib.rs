//! Answering cookie banners.
//!
//! A cookie banner is a question, and for most sites the honest answer is no. This crate holds
//! what Atoll knows about the consent tools the web uses, and builds the script that gives that
//! answer: it waits for the banner of a tool it recognises and presses its refusing button, the
//! same one the user would press.
//!
//! The script is handed to a page like the rules of the ad blocker. It only presses buttons that
//! are there, so a site that asks nothing stays untouched.

use std::sync::OnceLock;

use serde::Deserialize;

/// How long the script watches a page for a banner, in milliseconds. Banners often come late,
/// with the consent tool loaded after everything else.
const PATIENCE: u64 = 20_000;

/// How long one step waits for its button, in milliseconds.
const STEP_PATIENCE: u64 = 4_000;

/// What Atoll knows about one consent tool.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// A short name of the tool, for the log and the tests.
    pub id: String,
    /// The tool as its makers call it.
    pub name: String,
    /// The selector that says the tool is on the page.
    pub detect: String,
    /// What to press, in order.
    pub steps: Vec<Step>,
}

/// One press on the way to the refusal.
#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    /// The buttons that would do, the first one that is there is pressed.
    pub any: Vec<String>,
    /// The element whose shadow tree the buttons live in, if they do.
    #[serde(default)]
    pub root: Option<String>,
    /// Whether the refusal is complete once this was pressed.
    #[serde(default, rename = "final")]
    pub last: bool,
    /// Whether the step may be passed over when its button never appears.
    #[serde(default)]
    pub optional: bool,
}

const RULES: &str = include_str!("rules.json");
const SCRIPT: &str = include_str!("answer.js");

/// The consent tools Atoll can answer.
pub fn rules() -> &'static [Rule] {
    static PARSED: OnceLock<Vec<Rule>> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(RULES).expect("the consent rules are readable"))
}

/// The script that answers the banner of a page, ready to be put into a document.
pub fn answer_script() -> &'static str {
    static BUILT: OnceLock<String> = OnceLock::new();
    BUILT.get_or_init(|| {
        SCRIPT
            .replace("__RULES__", RULES.trim())
            .replace("__PATIENCE__", &PATIENCE.to_string())
            .replace("__STEP_PATIENCE__", &STEP_PATIENCE.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_tool_says_how_it_is_recognised_and_refused() {
        let rules = rules();
        assert!(rules.len() > 10, "the common tools are covered");
        let mut seen = HashSet::new();
        for rule in rules {
            assert!(seen.insert(&rule.id), "{} is there twice", rule.id);
            assert!(!rule.name.trim().is_empty(), "{} has no name", rule.id);
            assert!(
                !rule.detect.trim().is_empty(),
                "{} is not recognised",
                rule.id
            );
            assert!(!rule.steps.is_empty(), "{} refuses nothing", rule.id);
            for step in &rule.steps {
                assert!(
                    !step.any.is_empty(),
                    "{} has a step without a button",
                    rule.id
                );
                for selector in &step.any {
                    assert!(
                        !selector.trim().is_empty() && !selector.contains('\n'),
                        "{} has a selector that cannot be one: {selector:?}",
                        rule.id
                    );
                }
            }
            assert!(
                rule.steps.iter().any(|step| step.last),
                "{} never arrives at a refusal",
                rule.id
            );
        }
    }

    #[test]
    fn a_rule_that_only_has_steps_that_may_be_passed_over_does_nothing() {
        for rule in rules() {
            assert!(
                !rule.steps.iter().all(|step| step.optional),
                "{} would press nothing at all",
                rule.id
            );
        }
    }

    #[test]
    fn the_script_carries_the_rules_and_nothing_left_to_fill_in() {
        let script = answer_script();
        assert!(!script.contains("__RULES__"));
        assert!(!script.contains("__PATIENCE__"));
        assert!(!script.contains("__STEP_PATIENCE__"));
        assert!(script.contains(&PATIENCE.to_string()));
        for rule in rules() {
            assert!(
                script.contains(&format!("\"id\": \"{}\"", rule.id)),
                "{} is missing from the script",
                rule.id
            );
        }
    }
}
