//! Filling logins into the forms of a page.

/// Prefix of the console messages the fill script reports with.
pub(crate) const FILL_REPORT_PREFIX: &str = "\u{1}atoll-fill:";

/// What is filled into the login form of a page.
#[derive(Clone, Copy)]
pub struct LoginFill<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub one_time_code: Option<&'a str>,
    /// Whether fields that already hold something are left alone, which is what filling by
    /// itself does: it must never overwrite what the user typed.
    pub only_empty: bool,
}

/// Which fields of a login form one frame filled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilledFields {
    pub username: bool,
    pub password: bool,
    pub one_time_code: bool,
}

impl FilledFields {
    pub fn any(self) -> bool {
        self.username || self.password || self.one_time_code
    }
}

/// The script filling `login` into the visible login form of a frame.
pub(crate) fn fill_script(login: LoginFill<'_>) -> String {
    let arguments = serde_json::json!([
        login.username,
        login.password,
        login.one_time_code.unwrap_or_default(),
        FILL_REPORT_PREFIX,
        login.only_empty,
    ]);
    format!("({FILL_FUNCTION})(...{arguments});")
}

/// The fields a frame filled, from a report of the fill script, `None` for other messages.
pub(crate) fn parse_fill_report(message: &str) -> Option<FilledFields> {
    let payload = message.strip_prefix(FILL_REPORT_PREFIX)?;
    let (username, password, one_time_code) = serde_json::from_str(payload).ok()?;
    Some(FilledFields {
        username,
        password,
        one_time_code,
    })
}

/// Fills username, password or one-time code into the visible fields that look like them, also
/// inside open shadow roots, the way typing would, so pages built with frameworks notice the
/// values. Reports which fields it filled.
const FILL_FUNCTION: &str = r#"(username, secret, code, prefix, onlyEmpty) => {
  const inputs = [];
  const collect = (root) => {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.tagName === "INPUT") inputs.push(node);
      if (node.shadowRoot) collect(node.shadowRoot);
    }
  };
  collect(document);
  const visible = (field) => field.getClientRects().length > 0
    && getComputedStyle(field).visibility !== "hidden" && !field.disabled && !field.readOnly;
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  const fill = (field, value) => {
    if (onlyEmpty && field.value) return false;
    field.focus();
    setter.call(field, value);
    field.dispatchEvent(new InputEvent("input", {
      bubbles: true, composed: true, inputType: "insertText", data: value,
    }));
    field.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
    return field.value === value;
  };
  const hint = (field) => [field.autocomplete, field.name, field.id, field.placeholder,
    field.getAttribute("aria-label")].join(" ").toLowerCase();
  const textual = (field) => ["text", "email", "tel", ""].includes(field.type);
  const fields = inputs.filter(visible);
  const password = fields.find((field) => field.type === "password");
  const oneTime = fields.find((field) => /one-time-code/.test(field.autocomplete)
    || /otp|totp|2fa|mfa|one.?time|verification|einmal/.test(hint(field)));
  const user = fields.find((field) => /\busername\b/.test(field.autocomplete) && textual(field))
    || (password
      ? fields.slice(0, fields.indexOf(password)).reverse().find((field) => textual(field) && field !== oneTime)
      : fields.find((field) => textual(field) && field !== oneTime
        && (field.type === "email" || /user|login|mail|konto|account|name|benutzer|identifier/.test(hint(field)))));
  const filled = [
    Boolean(user && username && fill(user, username)),
    Boolean(password && secret && fill(password, secret)),
    Boolean(!password && oneTime && code && fill(oneTime, code)),
  ];
  console.log(prefix + JSON.stringify(filled));
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_passed_as_json_arguments() {
        let script = fill_script(LoginFill {
            username: "kim",
            password: "a\"b</script>\\",
            one_time_code: None,
            only_empty: false,
        });
        assert!(script.starts_with("((username, secret, code, prefix, onlyEmpty) =>"));
        assert!(
            script.ends_with(r#")(...["kim","a\"b</script>\\","","\u0001atoll-fill:",false]);"#)
        );
    }

    #[test]
    fn fill_reports_are_parsed() {
        let message = format!("{FILL_REPORT_PREFIX}[true,true,false]");
        let fields = parse_fill_report(&message).unwrap();
        assert_eq!(
            fields,
            FilledFields {
                username: true,
                password: true,
                one_time_code: false,
            }
        );
        assert!(fields.any());
        assert!(!FilledFields::default().any());
        assert_eq!(parse_fill_report("[true,true,false]"), None);
        assert_eq!(parse_fill_report(&format!("{FILL_REPORT_PREFIX}[1]")), None);
    }
}
