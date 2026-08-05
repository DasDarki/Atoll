//! Passkey requests are refused. Chromium asks for passkeys and security keys with a window of its
//! own that an off-screen browser cannot show, so the page would wait for it and stop reacting.

/// Hides the Web Authentication API from pages, so most of them do not offer passkeys at all.
pub(crate) const DISABLE_WEB_AUTHENTICATION: &str = "disable-blink-features=WebAuth";

/// The console message the passkey guard reports a refused request with.
pub(crate) const PASSKEY_REPORT: &str = "\u{1}atoll-passkey";

/// The guard script, run in every frame when its JavaScript context is created.
pub(crate) fn passkey_guard_script() -> String {
    PASSKEY_GUARD.replace(
        "__REPORT__",
        &serde_json::Value::from(PASSKEY_REPORT).to_string(),
    )
}

/// Rejects passkey requests the way Chromium does when the user cancels them. Requests waiting
/// for the autofill list only end when the page aborts them, as there is nothing to pick.
const PASSKEY_GUARD: &str = r#"(() => {
  const report = __REPORT__;
  const container = navigator.credentials && Object.getPrototypeOf(navigator.credentials);
  if (!container || !container.get || !container.create) return;
  const aborted = (signal) => signal.reason ?? new DOMException("The operation was aborted.", "AbortError");
  const guard = (original) => function (options) {
    if (!options || !options.publicKey) return original.apply(this, arguments);
    const signal = options.signal;
    if (signal && signal.aborted) return Promise.reject(aborted(signal));
    if (options.mediation === "conditional") {
      return new Promise((resolve, reject) => {
        if (signal) signal.addEventListener("abort", () => reject(aborted(signal)), { once: true });
      });
    }
    console.log(report);
    return Promise.reject(new DOMException(
      "The operation either timed out or was not allowed.", "NotAllowedError"));
  };
  container.get = guard(container.get);
  container.create = guard(container.create);
  if (window.PublicKeyCredential) {
    PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable = () => Promise.resolve(false);
    PublicKeyCredential.isConditionalMediationAvailable = () => Promise.resolve(false);
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_reports_with_the_passkey_message() {
        assert!(passkey_guard_script().contains(r#"const report = "\u0001atoll-passkey";"#));
    }
}
