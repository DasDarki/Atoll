use url::Url;

use crate::items::Login;

/// Whether `login` was saved for the site of `page_url`: the same host, or a subdomain of it.
/// `www.` is ignored on both sides.
pub fn matches(login: &Login, page_url: &str) -> bool {
    let Some(page) = host_of(page_url) else {
        return false;
    };
    login
        .urls
        .iter()
        .filter_map(|url| host_of(url))
        .any(|saved| {
            page == saved
                || page
                    .strip_suffix(saved.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
}

/// The logins saved for the site of `page_url`, sorted by title.
pub fn logins_for<'a>(logins: &'a [Login], page_url: &str) -> Vec<&'a Login> {
    let mut matching: Vec<&Login> = logins
        .iter()
        .filter(|login| matches(login, page_url))
        .collect();
    matching.sort_by_key(|login| login.title.to_lowercase());
    matching
}

fn host_of(address: &str) -> Option<String> {
    let parsed = Url::parse(address)
        .ok()
        .filter(|url| url.has_host())
        .or_else(|| Url::parse(&format!("https://{address}")).ok())?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(title: &str, urls: &[&str]) -> Login {
        Login {
            id: title.to_lowercase(),
            title: title.to_owned(),
            username: String::new(),
            vault: String::new(),
            urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        }
    }

    #[test]
    fn hosts_and_their_subdomains_match() {
        let github = login("GitHub", &["https://github.com/login"]);
        assert!(matches(&github, "https://github.com/session"));
        assert!(matches(&github, "https://www.github.com/"));
        assert!(matches(&github, "https://gist.github.com/"));
        assert!(!matches(&github, "https://notgithub.com/"));
        assert!(!matches(&github, "https://github.com.evil.example/"));
        assert!(!matches(&github, "data:text/html,github.com"));
    }

    #[test]
    fn saved_addresses_without_scheme_work() {
        let bank = login("Bank", &["meine-bank.de"]);
        assert!(matches(&bank, "https://login.meine-bank.de/"));
        assert!(!matches(&login("Leer", &[]), "https://meine-bank.de/"));
    }

    #[test]
    fn a_subdomain_login_does_not_match_the_parent_site() {
        let mail = login("Mail", &["https://mail.example.org"]);
        assert!(!matches(&mail, "https://example.org/"));
        assert!(!matches(&mail, "https://news.example.org/"));
    }

    #[test]
    fn matching_logins_are_sorted_by_title() {
        let logins = [
            login("zeta", &["https://example.org"]),
            login("Alpha", &["https://example.org"]),
            login("Other", &["https://other.example"]),
        ];
        let titles: Vec<&str> = logins_for(&logins, "https://example.org/")
            .iter()
            .map(|login| login.title.as_str())
            .collect();
        assert_eq!(titles, ["Alpha", "zeta"]);
    }
}
