use serde_json::Value;

/// One rule per selector: a single invalid selector would otherwise drop a whole group.
pub(crate) fn hide_rules<S: AsRef<str>>(selectors: impl IntoIterator<Item = S>) -> String {
    let mut css = String::new();
    for selector in selectors {
        let selector = selector.as_ref().trim();
        if is_safe_selector(selector) {
            css.push_str(selector);
            css.push_str(" { display: none !important; }\n");
        }
    }
    css
}

/// CSS for the procedural rules that are plain selectors with a `style` or `remove` action.
/// Rules needing script, such as `:has-text()`, are left out.
pub(crate) fn procedural_rules<S: AsRef<str>>(actions: impl IntoIterator<Item = S>) -> String {
    let mut css = String::new();
    for action in actions {
        let Ok(rule) = serde_json::from_str::<Value>(action.as_ref()) else {
            continue;
        };
        let Some(selector) = plain_selector(&rule) else {
            continue;
        };
        let declarations = match rule["action"]["type"].as_str() {
            Some("style") => match rule["action"]["arg"].as_str() {
                Some(style) if is_safe_style(style) => style.to_owned(),
                _ => continue,
            },
            Some("remove") => "display: none !important".to_owned(),
            _ => continue,
        };
        css.push_str(&format!("{selector} {{ {declarations} }}\n"));
    }
    css
}

fn plain_selector(rule: &Value) -> Option<&str> {
    let [operator] = rule["selector"].as_array()?.as_slice() else {
        return None;
    };
    if operator["type"].as_str()? != "css-selector" {
        return None;
    }
    operator["arg"]
        .as_str()
        .map(str::trim)
        .filter(|selector| is_safe_selector(selector))
}

/// Selectors must not be able to close the rule and start another one.
fn is_safe_selector(selector: &str) -> bool {
    !selector.is_empty() && !selector.contains(['{', '}']) && !selector.contains("/*")
}

fn is_safe_style(style: &str) -> bool {
    !style.contains(['{', '}']) && !style.contains("/*") && !style.contains("url(")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_selector_gets_its_own_rule() {
        assert_eq!(
            hide_rules([".ad", "#banner > div"]),
            ".ad { display: none !important; }\n#banner > div { display: none !important; }\n"
        );
    }

    #[test]
    fn selectors_that_could_escape_the_rule_are_dropped() {
        assert_eq!(hide_rules(["a} body { color: red", "", "b /* x"]), "");
    }

    #[test]
    fn plain_style_and_remove_actions_become_css() {
        let style = r#"{"selector":[{"type":"css-selector","arg":".box"}],"action":{"type":"style","arg":"margin: 0"}}"#;
        let remove =
            r##"{"selector":[{"type":"css-selector","arg":"#promo"}],"action":{"type":"remove"}}"##;
        assert_eq!(
            procedural_rules([style, remove]),
            ".box { margin: 0 }\n#promo { display: none !important }\n"
        );
    }

    #[test]
    fn procedural_selectors_are_left_out() {
        let has_text = r#"{"selector":[{"type":"css-selector","arg":"div"},{"type":"has-text","arg":"Anzeige"}],"action":null}"#;
        let tracking_style = r#"{"selector":[{"type":"css-selector","arg":"a"}],"action":{"type":"style","arg":"background: url(https://example.org/x)"}}"#;
        let attribute = r#"{"selector":[{"type":"css-selector","arg":"a"}],"action":{"type":"remove-attr","arg":"onclick"}}"#;
        assert_eq!(procedural_rules([has_text, tracking_style, attribute]), "");
    }
}
