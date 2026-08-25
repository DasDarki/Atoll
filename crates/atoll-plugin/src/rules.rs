//! Rules extensions use to stop or redirect requests.
//!
//! The browser asks for every request on its network thread, where no script may run. Extensions
//! therefore hand in rules instead of a function, and the rules are compared without Luau.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError, RwLock};

use crate::host::PluginId;
use crate::pattern::UrlPattern;

/// How many hits are kept for the extensions until they are picked up.
const KEPT_HITS: usize = 200;

/// What happens to a request that matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleAction {
    Block,
    /// Fetch this address instead.
    Redirect(String),
}

/// One rule of one extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRule {
    pub plugin: PluginId,
    pub id: String,
    pub patterns: Vec<UrlPattern>,
    /// The kinds of request this applies to, empty for all of them.
    pub kinds: Vec<String>,
    pub action: RuleAction,
}

impl RequestRule {
    fn matches(&self, url: &str, kind: &str) -> bool {
        (self.kinds.is_empty() || self.kinds.iter().any(|wanted| wanted == kind))
            && self.patterns.iter().any(|pattern| pattern.matches(url))
    }
}

/// A request a rule caught, for the extension that asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleHit {
    pub plugin: PluginId,
    pub rule: String,
    pub url: String,
    pub kind: String,
    pub action: RuleAction,
}

/// Every rule of every extension. The browser reads it from its network thread, the extensions
/// change it from the user interface thread, so it carries its own locks.
#[derive(Debug, Default)]
pub struct RuleSet {
    rules: RwLock<Vec<RequestRule>>,
    hits: Mutex<VecDeque<RuleHit>>,
}

impl RuleSet {
    /// Adds a rule or replaces the one of the same name.
    pub fn set_rule(&self, plugin: &PluginId, rule: RequestRule) {
        let mut all = self.rules.write().unwrap_or_else(PoisonError::into_inner);
        all.retain(|candidate| candidate.plugin != *plugin || candidate.id != rule.id);
        all.push(rule);
    }

    /// Replaces the rules of one extension.
    pub fn set(&self, plugin: &PluginId, rules: Vec<RequestRule>) {
        let mut all = self.rules.write().unwrap_or_else(PoisonError::into_inner);
        all.retain(|rule| rule.plugin != *plugin);
        all.extend(rules);
    }

    /// Removes one rule of an extension, or all of them when `id` is `None`.
    pub fn remove(&self, plugin: &PluginId, id: Option<&str>) {
        let mut all = self.rules.write().unwrap_or_else(PoisonError::into_inner);
        all.retain(|rule| rule.plugin != *plugin || id.is_some_and(|id| rule.id != id));
    }

    pub fn is_empty(&self) -> bool {
        self.rules
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// What the rules say about a request, and remembers the hit.
    pub fn verdict(&self, url: &str, kind: &str) -> Option<RuleAction> {
        let rules = self.rules.read().unwrap_or_else(PoisonError::into_inner);
        let rule = rules.iter().find(|rule| rule.matches(url, kind))?;
        let hit = RuleHit {
            plugin: rule.plugin.clone(),
            rule: rule.id.clone(),
            url: url.to_owned(),
            kind: kind.to_owned(),
            action: rule.action.clone(),
        };
        let action = rule.action.clone();
        drop(rules);
        let mut hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        if hits.len() >= KEPT_HITS {
            hits.pop_front();
        }
        hits.push_back(hit);
        Some(action)
    }

    /// Takes the hits that happened since the last time, for the `request.blocked` events.
    pub fn take_hits(&self) -> Vec<RuleHit> {
        let mut hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        hits.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(sources: &[&str]) -> Vec<UrlPattern> {
        sources
            .iter()
            .map(|source| UrlPattern::parse(source).unwrap())
            .collect()
    }

    fn rule(
        plugin: &str,
        id: &str,
        sources: &[&str],
        kinds: &[&str],
        action: RuleAction,
    ) -> RequestRule {
        RequestRule {
            plugin: plugin.to_owned(),
            id: id.to_owned(),
            patterns: patterns(sources),
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            action,
        }
    }

    #[test]
    fn rules_stop_and_redirect_by_pattern_and_kind() {
        let rules = RuleSet::default();
        rules.set(
            &"demo".to_owned(),
            vec![
                rule("demo", "ads", &["*://ads.test/*"], &[], RuleAction::Block),
                rule(
                    "demo",
                    "images",
                    &["*://slow.test/*"],
                    &["image"],
                    RuleAction::Redirect("https://fast.test/one.png".to_owned()),
                ),
            ],
        );
        assert_eq!(
            rules.verdict("https://ads.test/banner.js", "script"),
            Some(RuleAction::Block)
        );
        assert_eq!(
            rules.verdict("https://slow.test/a.png", "image"),
            Some(RuleAction::Redirect("https://fast.test/one.png".to_owned()))
        );
        assert_eq!(
            rules.verdict("https://slow.test/a.js", "script"),
            None,
            "the kind has to match"
        );
        assert_eq!(rules.verdict("https://other.test/a.js", "script"), None);
    }

    #[test]
    fn every_extension_keeps_its_own_rules() {
        let rules = RuleSet::default();
        rules.set(
            &"a".to_owned(),
            vec![rule("a", "one", &["*://a.test/*"], &[], RuleAction::Block)],
        );
        rules.set(
            &"b".to_owned(),
            vec![rule("b", "one", &["*://b.test/*"], &[], RuleAction::Block)],
        );
        rules.set(&"a".to_owned(), Vec::new());
        assert_eq!(rules.verdict("https://a.test/x", "script"), None);
        assert_eq!(
            rules.verdict("https://b.test/x", "script"),
            Some(RuleAction::Block),
            "removing the rules of one extension leaves the others alone"
        );
        rules.remove(&"b".to_owned(), Some("one"));
        assert!(rules.is_empty());
    }

    #[test]
    fn hits_are_kept_until_they_are_picked_up() {
        let rules = RuleSet::default();
        rules.set(
            &"demo".to_owned(),
            vec![rule(
                "demo",
                "ads",
                &["*://ads.test/*"],
                &[],
                RuleAction::Block,
            )],
        );
        rules.verdict("https://ads.test/a.js", "script");
        rules.verdict("https://ads.test/b.js", "script");
        rules.verdict("https://fine.test/c.js", "script");
        let hits = rules.take_hits();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://ads.test/a.js");
        assert_eq!(hits[0].plugin, "demo");
        assert_eq!(hits[0].rule, "ads");
        assert!(rules.take_hits().is_empty(), "they are only reported once");
    }
}
