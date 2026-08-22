//! Address patterns, written the way browser extensions write them:
//! `*://*.example.org/shop/*`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An address pattern of a manifest or a request rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UrlPattern {
    source: String,
    scheme: SchemeMatch,
    host: HostMatch,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchemeMatch {
    /// `*`, which means http and https, like in other browsers.
    Web,
    Exact(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostMatch {
    Any,
    /// `*.example.org`, which also matches `example.org` itself.
    Suffix(String),
    Exact(String),
}

/// Why a pattern could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("the pattern has no \"://\"")]
    MissingScheme,
    #[error("the pattern has no path, it has to end with at least \"/\"")]
    MissingPath,
    #[error("\"{0}\" is not a host")]
    InvalidHost(String),
}

impl UrlPattern {
    /// Reads a pattern such as `*://*.example.org/*` or `https://example.org/shop/*`.
    pub fn parse(pattern: &str) -> Result<Self, PatternError> {
        let (scheme, rest) = pattern
            .split_once("://")
            .ok_or(PatternError::MissingScheme)?;
        let (host, path) = match rest.find('/') {
            Some(slash) => rest.split_at(slash),
            None => return Err(PatternError::MissingPath),
        };
        let scheme = match scheme {
            "*" => SchemeMatch::Web,
            other => SchemeMatch::Exact(other.to_lowercase()),
        };
        let host = match host {
            "*" => HostMatch::Any,
            host => {
                let name = host.strip_prefix("*.").unwrap_or(host).to_lowercase();
                if name.is_empty() || name.contains('*') || name.contains('/') {
                    return Err(PatternError::InvalidHost(host.to_owned()));
                }
                if host.starts_with("*.") {
                    HostMatch::Suffix(name)
                } else {
                    HostMatch::Exact(name)
                }
            }
        };
        Ok(Self {
            source: pattern.to_owned(),
            scheme,
            host,
            path: path.to_owned(),
        })
    }

    /// Whether `url` is covered by the pattern.
    pub fn matches(&self, url: &str) -> bool {
        let Some((scheme, rest)) = url.split_once("://") else {
            return false;
        };
        let scheme = scheme.to_lowercase();
        match &self.scheme {
            SchemeMatch::Web => {
                if scheme != "http" && scheme != "https" {
                    return false;
                }
            }
            SchemeMatch::Exact(expected) if *expected != scheme => return false,
            SchemeMatch::Exact(_) => {}
        }
        let authority_end = rest.find('/').unwrap_or(rest.len());
        let (authority, path) = rest.split_at(authority_end);
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host)
            .split(':')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let host_matches = match &self.host {
            HostMatch::Any => true,
            HostMatch::Exact(expected) => host == *expected,
            HostMatch::Suffix(expected) => {
                host == *expected || host.ends_with(&format!(".{expected}"))
            }
        };
        let path = if path.is_empty() { "/" } else { path };
        host_matches && matches_glob(&self.path, path)
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The pattern as a regular expression for JavaScript, which is how a script injected into a
    /// page checks where it landed. It matches exactly what [`UrlPattern::matches`] matches.
    pub fn as_regex(&self) -> String {
        let scheme = match &self.scheme {
            SchemeMatch::Web => "https?".to_owned(),
            SchemeMatch::Exact(scheme) => escape(scheme),
        };
        let host = match &self.host {
            HostMatch::Any => "[^/]+".to_owned(),
            HostMatch::Exact(host) => format!("(?:[^/@]*@)?{}(?::\\d+)?", escape(host)),
            HostMatch::Suffix(host) => {
                format!("(?:[^/@]*@)?(?:[^/]+\\.)?{}(?::\\d+)?", escape(host))
            }
        };
        let path: String = self
            .path
            .split('*')
            .map(escape)
            .collect::<Vec<_>>()
            .join("[\\s\\S]*");
        format!("^{scheme}://{host}{path}$")
    }
}

/// Escapes everything a regular expression would read as a special character.
fn escape(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            let special = r"\^$.|?*+()[]{}/".contains(character);
            special.then_some('\\').into_iter().chain([character])
        })
        .collect()
}

impl TryFrom<String> for UrlPattern {
    type Error = PatternError;

    fn try_from(pattern: String) -> Result<Self, Self::Error> {
        Self::parse(&pattern)
    }
}

impl From<UrlPattern> for String {
    fn from(pattern: UrlPattern) -> Self {
        pattern.source
    }
}

impl fmt::Display for UrlPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.source)
    }
}

/// Whether `text` matches `pattern`, where `*` stands for any number of characters.
pub fn matches_glob(pattern: &str, text: &str) -> bool {
    let Some((start, after_first_star)) = pattern.split_once('*') else {
        return pattern == text;
    };
    let Some(mut rest) = text.strip_prefix(start) else {
        return false;
    };
    let (middle, end) = match after_first_star.rsplit_once('*') {
        Some((middle, end)) => (middle, end),
        None => ("", after_first_star),
    };
    for part in middle.split('*').filter(|part| !part.is_empty()) {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= end.len() && rest.ends_with(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(source: &str) -> UrlPattern {
        UrlPattern::parse(source).expect(source)
    }

    #[test]
    fn a_star_scheme_means_http_and_https() {
        let any = pattern("*://example.org/*");
        assert!(any.matches("http://example.org/"));
        assert!(any.matches("https://example.org/a/b?c=d"));
        assert!(!any.matches("file:///example.org/"));
        assert!(!any.matches("ftp://example.org/"));
        let secure = pattern("https://example.org/*");
        assert!(secure.matches("https://example.org/"));
        assert!(!secure.matches("http://example.org/"));
    }

    #[test]
    fn a_star_host_matches_subdomains_but_not_other_domains() {
        let site = pattern("*://*.example.org/*");
        assert!(site.matches("https://example.org/"));
        assert!(site.matches("https://www.example.org/"));
        assert!(site.matches("https://a.b.example.org/"));
        assert!(!site.matches("https://example.org.evil.test/"));
        assert!(!site.matches("https://notexample.org/"));
        let exact = pattern("*://example.org/*");
        assert!(exact.matches("https://example.org/"));
        assert!(!exact.matches("https://www.example.org/"));
        assert!(pattern("*://*/*").matches("http://anything.test/x"));
    }

    #[test]
    fn ports_and_credentials_do_not_confuse_the_host() {
        let site = pattern("*://example.org/*");
        assert!(site.matches("http://example.org:8080/a"));
        assert!(site.matches("http://kim:geheim@example.org/a"));
        assert!(!site.matches("http://evil.test/@example.org/"));
    }

    #[test]
    fn the_path_is_compared_with_stars() {
        let shop = pattern("https://example.org/shop/*");
        assert!(shop.matches("https://example.org/shop/"));
        assert!(shop.matches("https://example.org/shop/a/b"));
        assert!(!shop.matches("https://example.org/blog/"));
        let exact = pattern("https://example.org/a");
        assert!(exact.matches("https://example.org/a"));
        assert!(!exact.matches("https://example.org/ab"));
        let missing_path = pattern("https://example.org/");
        assert!(missing_path.matches("https://example.org"));
    }

    #[test]
    fn broken_patterns_are_reported() {
        assert_eq!(
            UrlPattern::parse("example.org/*"),
            Err(PatternError::MissingScheme)
        );
        assert_eq!(
            UrlPattern::parse("https://example.org"),
            Err(PatternError::MissingPath)
        );
        assert_eq!(
            UrlPattern::parse("https://ex*ample.org/*"),
            Err(PatternError::InvalidHost("ex*ample.org".to_owned()))
        );
    }

    #[test]
    fn the_regular_expression_matches_what_the_pattern_matches() {
        let addresses = [
            "https://example.org/",
            "http://example.org/a/b?c=d",
            "https://www.example.org/shop/x",
            "https://example.org.evil.test/",
            "https://example.org:8443/shop/",
            "http://kim@example.org/shop/",
            "ftp://example.org/",
            "https://other.test/shop/",
        ];
        for source in [
            "*://example.org/*",
            "*://*.example.org/*",
            "https://example.org/shop/*",
            "*://*/*",
            "https://example.org/",
        ] {
            let pattern = pattern(source);
            let expression = regex::Regex::new(&pattern.as_regex()).expect(source);
            for address in addresses {
                assert_eq!(
                    expression.is_match(address),
                    pattern.matches(address),
                    "{source} and its regular expression disagree about {address}"
                );
            }
        }
    }

    #[test]
    fn globs_match_anywhere() {
        assert!(matches_glob("*", ""));
        assert!(matches_glob("*b*", "abc"));
        assert!(matches_glob("a*c", "abbbc"));
        assert!(matches_glob("a*c*", "abcd"));
        assert!(!matches_glob("a*c", "abcd"));
        assert!(!matches_glob("*b*", "ac"));
        assert!(matches_glob("abc", "abc"));
        assert!(!matches_glob("abc", "abcd"));
        assert!(!matches_glob("a*a", "a"), "the stars may not overlap");
    }
}
