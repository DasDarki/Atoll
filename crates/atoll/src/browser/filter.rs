//! What Atoll does with a request: the ad blocker first, then the rules of the extensions.
//!
//! This runs on the network thread of the browser engine for every request, so it only compares
//! prepared rules and never reaches into the user interface.

use std::sync::Arc;

use atoll_engine::{ContentFilter, DocumentRules, ResourceKind, ResourceRequest, Verdict};
use atoll_plugin::{RuleAction, RuleSet};

/// The ad blocker and the extensions, asked in that order.
pub struct FilterChain {
    adblock: Arc<dyn ContentFilter>,
    rules: Arc<RuleSet>,
}

impl FilterChain {
    pub fn new(adblock: Arc<dyn ContentFilter>, rules: Arc<RuleSet>) -> Self {
        Self { adblock, rules }
    }
}

impl ContentFilter for FilterChain {
    fn verdict(&self, request: &ResourceRequest<'_>) -> Verdict {
        match self.adblock.verdict(request) {
            Verdict::Allow => {}
            blocked => return blocked,
        }
        match self.rules.verdict(request.url, kind_name(request.kind)) {
            Some(RuleAction::Block) => Verdict::Block,
            Some(RuleAction::Redirect(target)) => Verdict::Redirect(target),
            None => Verdict::Allow,
        }
    }

    fn document_rules(&self, url: &str) -> DocumentRules {
        self.adblock.document_rules(url)
    }

    fn element_rules(&self, url: &str, classes: &[String], ids: &[String]) -> String {
        self.adblock.element_rules(url, classes, ids)
    }
}

/// The name extensions use for a kind of request.
fn kind_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Document => "document",
        ResourceKind::Subframe => "subframe",
        ResourceKind::Stylesheet => "stylesheet",
        ResourceKind::Script => "script",
        ResourceKind::Image => "image",
        ResourceKind::Font => "font",
        ResourceKind::Media => "media",
        ResourceKind::Object => "object",
        ResourceKind::Worker => "worker",
        ResourceKind::Xhr => "xhr",
        ResourceKind::Ping => "ping",
        ResourceKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atoll_plugin::{RequestRule, UrlPattern};

    /// An ad blocker that only ever blocks one address.
    struct OneBlocker;

    impl ContentFilter for OneBlocker {
        fn verdict(&self, request: &ResourceRequest<'_>) -> Verdict {
            if request.url.contains("werbung") {
                Verdict::Block
            } else {
                Verdict::Allow
            }
        }

        fn document_rules(&self, _url: &str) -> DocumentRules {
            DocumentRules::default()
        }

        fn element_rules(&self, _url: &str, _classes: &[String], _ids: &[String]) -> String {
            String::new()
        }
    }

    fn request<'a>(url: &'a str, kind: ResourceKind) -> ResourceRequest<'a> {
        ResourceRequest {
            url,
            source_url: "https://example.org/",
            page_url: "https://example.org/",
            kind,
        }
    }

    #[test]
    fn the_ad_blocker_comes_first_and_the_rules_after_it() {
        let rules = Arc::new(RuleSet::default());
        rules.set(
            &"demo".to_owned(),
            vec![RequestRule {
                plugin: "demo".to_owned(),
                id: "bilder".to_owned(),
                patterns: vec![UrlPattern::parse("*://slow.test/*").unwrap()],
                kinds: vec!["image".to_owned()],
                action: RuleAction::Redirect("https://fast.test/x.png".to_owned()),
            }],
        );
        let chain = FilterChain::new(Arc::new(OneBlocker), rules.clone());

        assert_eq!(
            chain.verdict(&request(
                "https://ads.test/werbung.js",
                ResourceKind::Script
            )),
            Verdict::Block
        );
        assert_eq!(
            chain.verdict(&request("https://slow.test/a.png", ResourceKind::Image)),
            Verdict::Redirect("https://fast.test/x.png".to_owned())
        );
        assert_eq!(
            chain.verdict(&request("https://slow.test/a.js", ResourceKind::Script)),
            Verdict::Allow
        );
        assert_eq!(rules.take_hits().len(), 1, "only the rule counts as a hit");
    }
}
