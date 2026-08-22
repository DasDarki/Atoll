//! Styles and scripts extensions put into pages.
//!
//! Such a script runs in every page the browser loads, so where it may run is decided in the page
//! itself: the wrapper checks the address against the sites of the manifest and against what the
//! extension asked for, and stops right away if either says no.

use serde_json::Value;

use crate::pattern::UrlPattern;

/// Which JavaScript world a script runs in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScriptWorld {
    /// A world of its own, which the page can neither see nor change.
    #[default]
    Isolated,
    /// The world of the page's own scripts, for extensions that have to work with them.
    Page,
}

impl ScriptWorld {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "isolated" => Some(ScriptWorld::Isolated),
            "page" => Some(ScriptWorld::Page),
            _ => None,
        }
    }
}

/// When a script runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunAt {
    /// Before the page's own scripts.
    #[default]
    DocumentStart,
    /// Once the document is read.
    DocumentEnd,
}

impl RunAt {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "documentStart" => Some(RunAt::DocumentStart),
            "documentEnd" => Some(RunAt::DocumentEnd),
            _ => None,
        }
    }
}

/// What an extension wants to put into pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentScript {
    /// The name the browser knows the script by, made from the extension and the script.
    pub name: String,
    pub source: String,
    pub world: ScriptWorld,
}

/// The script that adds `css` to a page that is already open.
pub(crate) fn stylesheet(css: &str) -> String {
    format!(
        "(() => {{ const sheet = new CSSStyleSheet(); sheet.replaceSync({}); \
         document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet]; }})()",
        Value::from(css)
    )
}

/// What an extension asked for, before it is turned into a script for the browser.
pub(crate) struct Request<'a> {
    pub(crate) plugin: &'a str,
    pub(crate) id: &'a str,
    /// The sites of the manifest, which the script may never leave.
    pub(crate) allowed: &'a [UrlPattern],
    /// The sites the extension picked for this script.
    pub(crate) wanted: &'a [UrlPattern],
    pub(crate) css: &'a str,
    pub(crate) js: &'a str,
    pub(crate) run_at: RunAt,
    pub(crate) world: ScriptWorld,
}

/// Builds the script that is handed to the browser.
pub(crate) fn build(request: Request<'_>) -> ContentScript {
    let Request {
        plugin,
        id,
        allowed,
        wanted,
        css,
        js,
        run_at,
        world,
    } = request;
    let expressions = |patterns: &[UrlPattern]| {
        patterns
            .iter()
            .map(|pattern| format!("new RegExp({})", Value::from(pattern.as_regex())))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut body = String::new();
    if !css.is_empty() {
        body.push_str(&format!(
            "try {{ const sheet = new CSSStyleSheet(); sheet.replaceSync({}); \
             document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet]; }} \
             catch (error) {{ console.error(error); }}\n",
            Value::from(css)
        ));
    }
    if !js.is_empty() {
        body.push_str(js);
    }
    let immediately = matches!(run_at, RunAt::DocumentStart);
    let source = WRAPPER
        .replace("__ALLOWED__", &expressions(allowed))
        .replace("__WANTED__", &expressions(wanted))
        .replace(
            "__IMMEDIATELY__",
            if immediately { "true" } else { "false" },
        )
        .replace("__BODY__", &body);
    ContentScript {
        name: format!("{plugin}/{id}"),
        source,
        world,
    }
}

const WRAPPER: &str = r#"(() => {
  const allowed = [__ALLOWED__];
  const wanted = [__WANTED__];
  const here = location.href;
  if (!allowed.some((pattern) => pattern.test(here))) return;
  if (!wanted.some((pattern) => pattern.test(here))) return;
  const run = () => {
__BODY__
  };
  if (__IMMEDIATELY__ || document.readyState !== "loading") {
    run();
  } else {
    document.addEventListener("DOMContentLoaded", run, { once: true });
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(sources: &[&str]) -> Vec<UrlPattern> {
        sources
            .iter()
            .map(|source| UrlPattern::parse(source).unwrap())
            .collect()
    }

    #[test]
    fn the_script_checks_the_manifest_and_the_wish() {
        let script = build(Request {
            plugin: "demo",
            id: "clean",
            allowed: &patterns(&["*://*.example.org/*"]),
            wanted: &patterns(&["*://*.example.org/shop/*"]),
            css: ".ad { display: none }",
            js: "console.log('hallo')",
            run_at: RunAt::DocumentStart,
            world: ScriptWorld::Isolated,
        });
        assert_eq!(script.name, "demo/clean");
        assert_eq!(script.world, ScriptWorld::Isolated);
        assert!(script.source.contains("const allowed = [new RegExp("));
        assert!(script.source.contains("shop"), "the wish is checked too");
        assert!(script.source.contains("adoptedStyleSheets"));
        assert!(script.source.contains("console.log('hallo')"));
        assert!(!script.source.contains("__IMMEDIATELY__"));
        assert!(script.source.contains("if (true || document.readyState"));
    }

    #[test]
    fn a_script_for_the_end_waits_for_the_document() {
        let script = build(Request {
            plugin: "demo",
            id: "late",
            allowed: &patterns(&["*://*/*"]),
            wanted: &patterns(&["*://*/*"]),
            css: "",
            js: "done()",
            run_at: RunAt::DocumentEnd,
            world: ScriptWorld::Page,
        });
        assert!(script.source.contains("if (false || document.readyState"));
        assert!(
            !script.source.contains("adoptedStyleSheets"),
            "no styles, no sheet"
        );
        assert_eq!(script.world, ScriptWorld::Page);
    }

    #[test]
    fn worlds_and_times_are_read_from_their_names() {
        assert_eq!(ScriptWorld::parse("page"), Some(ScriptWorld::Page));
        assert_eq!(ScriptWorld::parse("nirgends"), None);
        assert_eq!(RunAt::parse("documentEnd"), Some(RunAt::DocumentEnd));
        assert_eq!(RunAt::parse("später"), None);
    }
}
