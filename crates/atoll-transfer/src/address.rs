use std::fmt;

use url::{ParseError, Url};

const SUPPORTED_SCHEMES: &[&str] = &[
    "http",
    "https",
    "file",
    "ftp",
    "about",
    "data",
    "view-source",
    "chrome",
    "devtools",
];

/// Why a line of a URL list could not be imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    NotAnAddress,
    UnsupportedScheme(String),
}

impl fmt::Display for InvalidReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidReason::NotAnAddress => formatter.write_str("keine gültige Adresse"),
            InvalidReason::UnsupportedScheme(scheme) => {
                write!(formatter, "Schema \"{scheme}\" wird nicht unterstützt")
            }
        }
    }
}

/// Validates a single address and returns its normalised form.
///
/// Addresses without a scheme such as `example.com/page` are accepted as `https`.
pub fn validate_address(candidate: &str) -> Result<String, InvalidReason> {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.chars().any(char::is_whitespace) {
        return Err(InvalidReason::NotAnAddress);
    }

    match Url::parse(candidate) {
        Ok(url) if SUPPORTED_SCHEMES.contains(&url.scheme()) => {
            if matches!(url.scheme(), "http" | "https") && url.host_str().is_none_or(str::is_empty)
            {
                return Err(InvalidReason::NotAnAddress);
            }
            Ok(url.into())
        }
        Ok(_) if looks_like_host_with_port(candidate) => with_default_scheme(candidate),
        Ok(url) => Err(InvalidReason::UnsupportedScheme(url.scheme().to_owned())),
        Err(ParseError::RelativeUrlWithoutBase) => with_default_scheme(candidate),
        Err(_) => Err(InvalidReason::NotAnAddress),
    }
}

fn looks_like_host_with_port(candidate: &str) -> bool {
    candidate.split_once(':').is_some_and(|(host, rest)| {
        host.contains('.') && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    })
}

fn with_default_scheme(candidate: &str) -> Result<String, InvalidReason> {
    let url =
        Url::parse(&format!("https://{candidate}")).map_err(|_| InvalidReason::NotAnAddress)?;
    let host = url.host_str().unwrap_or_default();
    let is_plausible_host = host == "localhost"
        || (host.contains('.') && !host.starts_with('.') && !host.ends_with('.'));
    if is_plausible_host {
        Ok(url.into())
    } else {
        Err(InvalidReason::NotAnAddress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_normalises_web_addresses() {
        assert_eq!(
            validate_address("https://Example.com").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            validate_address("  http://example.com/a?b=c#d ").unwrap(),
            "http://example.com/a?b=c#d"
        );
    }

    #[test]
    fn adds_https_to_bare_hosts() {
        assert_eq!(
            validate_address("github.com/brave/adblock-rust").unwrap(),
            "https://github.com/brave/adblock-rust"
        );
        assert_eq!(
            validate_address("example.com:8080/x").unwrap(),
            "https://example.com:8080/x"
        );
        assert_eq!(validate_address("localhost").unwrap(), "https://localhost/");
    }

    #[test]
    fn rejects_text_and_foreign_schemes() {
        assert_eq!(
            validate_address("hello world"),
            Err(InvalidReason::NotAnAddress)
        );
        assert_eq!(validate_address("nodot"), Err(InvalidReason::NotAnAddress));
        assert_eq!(
            validate_address("opera://settings"),
            Err(InvalidReason::UnsupportedScheme("opera".into()))
        );
        assert_eq!(
            validate_address("https://"),
            Err(InvalidReason::NotAnAddress)
        );
    }
}
