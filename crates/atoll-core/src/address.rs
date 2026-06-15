use url::Url;

/// Returns the host of `address` without a leading `www.`, for compact display.
pub fn display_host(address: &str) -> Option<String> {
    let host = host_of(address)?;
    Some(host.strip_prefix("www.").unwrap_or(&host).to_owned())
}

/// Returns the lower-cased host of `address`, if it has one.
pub fn host_of(address: &str) -> Option<String> {
    let parsed = Url::parse(address).ok()?;
    parsed.host_str().map(str::to_ascii_lowercase)
}

/// Normalises an address so that equivalent spellings compare equal.
pub fn normalize(address: &str) -> String {
    Url::parse(address.trim())
        .map(String::from)
        .unwrap_or_else(|_| address.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_lower_cased() {
        assert_eq!(
            host_of("https://Example.COM/path"),
            Some("example.com".into())
        );
    }

    #[test]
    fn display_host_strips_www() {
        assert_eq!(
            display_host("https://www.rust-lang.org"),
            Some("rust-lang.org".into())
        );
    }

    #[test]
    fn invalid_address_has_no_host() {
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn normalize_unifies_trailing_slash_and_case() {
        assert_eq!(
            normalize("https://EXAMPLE.com"),
            normalize("https://example.com/")
        );
    }
}
