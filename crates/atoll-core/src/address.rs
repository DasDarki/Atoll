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

/// The address without scheme and leading `www.`, as it is typed and shown in suggestions.
pub fn display_url(address: &str) -> String {
    let without_scheme = address.split_once("://").map_or(address, |(_, rest)| rest);
    let without_www = without_scheme
        .strip_prefix("www.")
        .unwrap_or(without_scheme);
    without_www
        .strip_suffix('/')
        .unwrap_or(without_www)
        .to_owned()
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
    fn display_url_drops_scheme_www_and_trailing_slash() {
        assert_eq!(display_url("https://www.github.com/"), "github.com");
        assert_eq!(display_url("http://example.com/a/b"), "example.com/a/b");
        assert_eq!(display_url("about:blank"), "about:blank");
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
