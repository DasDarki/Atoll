//! Content filtering: blocking requests and hiding page elements, as done by an ad blocker.

use cef::sys::cef_resource_type_t;

/// Prefix of the console messages the element observer reports class names and ids with.
pub(crate) const ELEMENT_REPORT_PREFIX: &str = "\u{1}atoll-elements:";

/// The most class names or ids taken from a single report.
const MAX_REPORTED_NAMES: usize = 4096;

/// What a request loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Subframe,
    Stylesheet,
    Script,
    Image,
    Font,
    Media,
    Object,
    Worker,
    Xhr,
    Ping,
    Other,
}

impl ResourceKind {
    pub(crate) fn of(kind: cef_resource_type_t) -> Self {
        match kind {
            cef_resource_type_t::RT_MAIN_FRAME
            | cef_resource_type_t::RT_NAVIGATION_PRELOAD_MAIN_FRAME => ResourceKind::Document,
            cef_resource_type_t::RT_SUB_FRAME
            | cef_resource_type_t::RT_NAVIGATION_PRELOAD_SUB_FRAME => ResourceKind::Subframe,
            cef_resource_type_t::RT_STYLESHEET => ResourceKind::Stylesheet,
            cef_resource_type_t::RT_SCRIPT => ResourceKind::Script,
            cef_resource_type_t::RT_IMAGE | cef_resource_type_t::RT_FAVICON => ResourceKind::Image,
            cef_resource_type_t::RT_FONT_RESOURCE => ResourceKind::Font,
            cef_resource_type_t::RT_MEDIA => ResourceKind::Media,
            cef_resource_type_t::RT_OBJECT | cef_resource_type_t::RT_PLUGIN_RESOURCE => {
                ResourceKind::Object
            }
            cef_resource_type_t::RT_WORKER
            | cef_resource_type_t::RT_SHARED_WORKER
            | cef_resource_type_t::RT_SERVICE_WORKER => ResourceKind::Worker,
            cef_resource_type_t::RT_XHR => ResourceKind::Xhr,
            cef_resource_type_t::RT_PING | cef_resource_type_t::RT_CSP_REPORT => ResourceKind::Ping,
            _ => ResourceKind::Other,
        }
    }
}

/// A request a page is about to make.
#[derive(Debug, Clone, Copy)]
pub struct ResourceRequest<'a> {
    pub url: &'a str,
    /// The document the request comes from, a frame of the page.
    pub source_url: &'a str,
    /// The top level document of the page.
    pub page_url: &'a str,
    pub kind: ResourceKind,
}

/// Styles and script applied to a document when it starts loading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentRules {
    /// Complete CSS rules added to the document.
    pub stylesheet: String,
    /// Script run in the document before its own scripts.
    pub script: String,
    /// Whether the class names and ids of the document's elements are passed to
    /// [`ContentFilter::element_rules`] as they appear. Only done for the top level document.
    pub observe_elements: bool,
}

/// Decides which requests are blocked and which elements are hidden, see
/// [`crate::EngineConfig::content_filter`].
pub trait ContentFilter: Send + Sync {
    /// Whether `request` is blocked. Called on the network thread for every request except
    /// navigations of the top level document, so it has to be fast.
    fn blocks(&self, request: &ResourceRequest<'_>) -> bool;

    /// The rules for a document starting to load at `url`, called on the main thread.
    fn document_rules(&self, url: &str) -> DocumentRules;

    /// CSS rules for elements with the given class names and ids on the page at `url`, called on
    /// the main thread as elements appear.
    fn element_rules(&self, url: &str, classes: &[String], ids: &[String]) -> String;
}

/// The script applying `rules` to a document.
pub(crate) fn document_script(rules: &DocumentRules) -> Option<String> {
    if rules.stylesheet.is_empty() && rules.script.is_empty() && !rules.observe_elements {
        return None;
    }
    let mut script = String::from("(() => {\n");
    if !rules.stylesheet.is_empty() {
        script.push_str(&stylesheet_statement(&rules.stylesheet));
    }
    if !rules.script.is_empty() {
        script.push_str("try {\n");
        script.push_str(&rules.script);
        script.push_str("\n} catch (error) {}\n");
    }
    if rules.observe_elements {
        script.push_str(&ELEMENT_OBSERVER.replace(
            "__PREFIX__",
            &serde_json::Value::from(ELEMENT_REPORT_PREFIX).to_string(),
        ));
    }
    script.push_str("})();");
    Some(script)
}

/// The script adding `css` to the current document.
pub(crate) fn stylesheet_script(css: &str) -> String {
    format!("(() => {{\n{}}})();", stylesheet_statement(css))
}

fn stylesheet_statement(css: &str) -> String {
    format!(
        "try {{ const sheet = new CSSStyleSheet(); sheet.replaceSync({}); \
         document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet]; }} catch (error) {{}}\n",
        serde_json::Value::from(css)
    )
}

/// Class names and ids from an element report of the observer, `None` for other messages.
pub(crate) fn parse_element_report(message: &str) -> Option<(Vec<String>, Vec<String>)> {
    let payload = message.strip_prefix(ELEMENT_REPORT_PREFIX)?;
    let (classes, ids): (Vec<String>, Vec<String>) = serde_json::from_str(payload).ok()?;
    let limit = |mut names: Vec<String>| {
        names.truncate(MAX_REPORTED_NAMES);
        names
    };
    Some((limit(classes), limit(ids)))
}

/// Reports the class names and ids of all elements, new ones in batches as they appear.
const ELEMENT_OBSERVER: &str = r#"(() => {
  const prefix = __PREFIX__;
  const seenClasses = new Set();
  const seenIds = new Set();
  let classes = [];
  let ids = [];
  let timer = 0;
  const report = () => {
    timer = 0;
    if (classes.length === 0 && ids.length === 0) return;
    console.log(prefix + JSON.stringify([classes, ids]));
    classes = [];
    ids = [];
  };
  const schedule = () => {
    if (!timer) timer = setTimeout(report, 50);
  };
  const add = (element) => {
    if (element.id && !seenIds.has(element.id)) {
      seenIds.add(element.id);
      ids.push(element.id);
    }
    if (element.classList) {
      for (const name of element.classList) {
        if (!seenClasses.has(name)) {
          seenClasses.add(name);
          classes.push(name);
        }
      }
    }
  };
  const scan = (node) => {
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    add(node);
    for (const element of node.querySelectorAll("[id],[class]")) add(element);
  };
  if (document.documentElement) scan(document.documentElement);
  schedule();
  new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      if (mutation.type === "attributes") add(mutation.target);
      else for (const node of mutation.addedNodes) scan(node);
    }
    schedule();
  }).observe(document, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class", "id"],
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rules_need_no_script() {
        assert_eq!(document_script(&DocumentRules::default()), None);
    }

    #[test]
    fn stylesheets_are_embedded_as_string_literals() {
        let script = document_script(&DocumentRules {
            stylesheet: ".ad { display: none !important; }\n\"quoted\" `tick`".to_owned(),
            ..DocumentRules::default()
        })
        .unwrap();
        assert!(
            script
                .contains(r#"replaceSync(".ad { display: none !important; }\n\"quoted\" `tick`")"#)
        );
        assert!(!script.contains("MutationObserver"));
    }

    #[test]
    fn observed_documents_get_the_element_observer() {
        let script = document_script(&DocumentRules {
            observe_elements: true,
            ..DocumentRules::default()
        })
        .unwrap();
        assert!(script.contains("MutationObserver"));
        assert!(script.contains(r#"const prefix = "\u0001atoll-elements:";"#));
    }

    #[test]
    fn element_reports_are_parsed_and_limited() {
        let message = format!(r#"{ELEMENT_REPORT_PREFIX}[["banner","ad"],["top"]]"#);
        assert_eq!(
            parse_element_report(&message),
            Some((
                vec!["banner".to_owned(), "ad".to_owned()],
                vec!["top".to_owned()]
            ))
        );
        assert_eq!(parse_element_report("hello"), None);
        assert_eq!(
            parse_element_report(&format!("{ELEMENT_REPORT_PREFIX}not json")),
            None
        );
        let many: Vec<String> = (0..MAX_REPORTED_NAMES + 10)
            .map(|n| n.to_string())
            .collect();
        let message = format!(
            "{ELEMENT_REPORT_PREFIX}{}",
            serde_json::to_string(&(many, Vec::<String>::new())).unwrap()
        );
        assert_eq!(
            parse_element_report(&message).unwrap().0.len(),
            MAX_REPORTED_NAMES
        );
    }
}
