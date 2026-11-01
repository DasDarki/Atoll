//! Whether a page sending the user to another site is doing them a favour or robbing them of
//! the page they were reading.
//!
//! Stopping every such hop is easy and wrong. Half the web gets you where you want to go through
//! another address: a search result that counts the click, a short address, the sign-in of one
//! site held by another, a payment that walks through three hosts before it comes back. Those
//! hops carry no click of their own, because the click was two hops ago, and a browser that only
//! asks "did the user click right now?" stops them all.
//!
//! So the question is asked properly, and asked here, on this computer and nowhere else: what is
//! known about the address, about the page it comes from and about what the user was doing. Each
//! thing noticed is a mark for or against, the marks are added up and read as a chance, and only
//! an address that is more likely advertising than not is stopped. Nothing is sent anywhere to
//! answer this, nothing is learned about the user, and the same address always gets the same
//! answer.
//!
//! The weights are the judgement, and they are written out below so they can be read, argued
//! with and changed. What carries the most is not a clever guess but the filter lists: millions
//! of people maintain them, and when they know an address, that settles it.

use std::time::Duration;

/// Where the line is drawn: an address that reaches this is stopped.
pub const LIMIT: f32 = 0.5;

/// What a page sending the user elsewhere by itself starts with. It is above the line, so an
/// address nothing is known about is stopped - that is what the protection is for.
const START: f32 = 0.6;

/// What is known about an address a page wants to send the user to.
#[derive(Debug, Clone, Copy)]
pub struct Attempt<'a> {
    /// The page the user is on.
    pub page_url: &'a str,
    /// Where it wants to send them.
    pub target_url: &'a str,
    /// How long ago the user last did something in that page themselves, if they did anything.
    pub since_acted: Option<Duration>,
    /// How long ago the browser sent that page somewhere because the user asked for the
    /// address. They wanted the page, which is not the same as wanting to be sent on from it.
    pub since_asked: Option<Duration>,
    /// How many times in a row the page has sent itself somewhere without being asked.
    pub hops: u32,
    /// Whether the filter lists know the address as advertising or a tracker.
    pub on_a_list: bool,
}

/// One thing the judgement noticed, and how much it says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    /// What was noticed, written for the log.
    pub about: &'static str,
    /// Above zero it speaks against the address, below zero for it.
    pub weight: f32,
}

/// What the judgement came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgement {
    /// How likely it is that the address is advertising or spam, between zero and one.
    pub chance: f32,
    /// Everything that was noticed, in the order it was noticed.
    pub marks: Vec<Mark>,
}

impl Judgement {
    /// Whether the hop is stopped.
    pub fn stops_it(&self) -> bool {
        self.chance >= LIMIT
    }

    /// The marks as one line for the log.
    pub fn reasons(&self) -> String {
        self.marks
            .iter()
            .map(|mark| format!("{} ({:+.1})", mark.about, mark.weight))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Judges one hop. Runs on the thread the engine asks on and touches nothing outside itself.
pub fn judge(attempt: &Attempt<'_>) -> Judgement {
    let marks = marks(attempt);
    let sum = START + marks.iter().map(|mark| mark.weight).sum::<f32>();
    Judgement {
        chance: 1.0 / (1.0 + (-sum).exp()),
        marks,
    }
}

/// Everything the judgement notices about one hop.
fn marks(attempt: &Attempt<'_>) -> Vec<Mark> {
    let mut marks = Vec::new();
    let mut mark = |about, weight| marks.push(Mark { about, weight });

    if attempt.on_a_list {
        // The lists know what advertising looks like better than any guess made here.
        mark("the filter lists know the address", 4.0);
    }
    match attempt.since_acted {
        Some(since) if since <= Duration::from_secs(3) => {
            mark("the user has just done something", -2.6);
        }
        Some(since) if since <= Duration::from_secs(10) => {
            mark("the user did something a moment ago", -1.0);
        }
        Some(since) if since <= Duration::from_secs(30) => {
            mark("the user did something a while ago", -0.3);
        }
        Some(_) => mark("the user has not touched the page for a long time", 0.4),
        None => {}
    }
    if attempt
        .since_asked
        .is_some_and(|since| since <= Duration::from_secs(5))
    {
        // The address was wanted, so where it leads by itself is probably part of it: a site
        // that has moved, a country of its own, a gate that asks something first.
        mark("the page was opened a moment ago", -1.4);
    }
    if attempt.hops > HOPS_WITHOUT_DOUBT {
        let over = (attempt.hops - HOPS_WITHOUT_DOUBT) as f32;
        mark("the page keeps sending itself on", (over * 0.4).min(2.6));
    }

    let page = Address::of(attempt.page_url);
    let target = Address::of(attempt.target_url);
    let (Some(page), Some(target)) = (page, target) else {
        return marks;
    };

    if page.domain == target.domain && !target.domain.is_empty() {
        mark("it stays with the same site", -3.0);
    } else if page.name() == target.name() && !target.name().is_empty() {
        mark("the same name under another ending", -2.0);
    }
    if page.carries(&target) {
        mark("the page says in its own address where it leads", -2.4);
    }
    if SHORTENERS.contains(&page.host.as_str()) {
        mark("the page is an address that stands for another", -1.8);
    } else if page.sends_on() {
        mark("sending people on is what the page is for", -1.2);
    }
    if target.has_any(&SIGNING_IN) {
        mark("it leads to signing in", -1.8);
    }
    if target.has_any(&PAYING) {
        mark("it leads to paying", -1.2);
    }
    if CHEAP_ENDINGS.contains(&target.ending()) {
        mark("the ending is one advertising is cheaply at home in", 1.3);
    }
    if target.has_any(&COUNTING_CLICKS) {
        mark("the address counts a click", 0.9);
    }
    if target.is_a_number() {
        mark("the host is a number, not a name", 1.2);
    }
    if target.looks_made_up() {
        mark("the name of the host looks made up", 0.9);
    }
    if target.host.split('.').count() >= 5 {
        mark("the host is buried under its own names", 0.4);
    }
    if target.host.contains("xn--") {
        mark("the host is written in letters that look like others", 0.7);
    }
    marks
}

/// How many hops a page may make before being sent on again counts against it.
const HOPS_WITHOUT_DOUBT: u32 = 6;

/// Addresses that stand for another one, whose whole purpose is to send people on.
const SHORTENERS: [&str; 24] = [
    "t.co",
    "bit.ly",
    "tinyurl.com",
    "goo.gl",
    "ow.ly",
    "buff.ly",
    "lnkd.in",
    "rb.gy",
    "cutt.ly",
    "is.gd",
    "shorturl.at",
    "t.ly",
    "v.gd",
    "s.id",
    "trib.al",
    "dlvr.it",
    "href.li",
    "l.facebook.com",
    "lm.facebook.com",
    "l.instagram.com",
    "l.messenger.com",
    "out.reddit.com",
    "away.vk.com",
    "t.umblr.com",
];

/// What the first part of a path says when the page is there to send people on.
const SENDING_ON: [&str; 11] = [
    "url", "redirect", "redir", "out", "outgoing", "go", "away", "exit", "jump", "visit", "link",
];

/// What an address says when it leads to signing in, which walks between sites by nature.
const SIGNING_IN: [&str; 16] = [
    "oauth",
    "openid",
    "authorize",
    "/auth",
    "signin",
    "sign-in",
    "/login",
    "logout",
    "/sso",
    "saml",
    "client_id=",
    "redirect_uri=",
    "response_type=",
    "id_token",
    "/callback",
    "consent",
];

/// The same for paying, which walks between the shop, the bank and back.
const PAYING: [&str; 9] = [
    "checkout",
    "/pay",
    "payment",
    "paypal",
    "stripe",
    "klarna",
    "3ds",
    "acs",
    "secure-redirect",
];

/// What an address says when it is there to count a click for money.
const COUNTING_CLICKS: [&str; 12] = [
    "aff_id",
    "affiliate",
    "clickid",
    "click_id",
    "subid",
    "sub_id",
    "offer_id",
    "zoneid",
    "campaign_id",
    "smartlink",
    "popunder",
    "trackid",
];

/// Endings that cost almost nothing, which is why so much advertising lives under them.
const CHEAP_ENDINGS: [&str; 22] = [
    "top", "xyz", "click", "buzz", "icu", "cfd", "sbs", "rest", "monster", "quest", "casa", "loan",
    "gdn", "stream", "date", "review", "country", "men", "mom", "lol", "cyou", "bar",
];

/// An address taken apart, as far as the judgement needs it.
struct Address {
    host: String,
    /// The site the host belongs to, such as `example.co.uk` for `shop.example.co.uk`.
    domain: String,
    path: String,
    /// Path and query together, which is where the words of an address stand.
    rest: String,
}

impl Address {
    fn of(url: &str) -> Option<Self> {
        let parsed = url::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return None;
        }
        let host = parsed.host_str()?.to_ascii_lowercase();
        let domain = psl::domain_str(&host).unwrap_or(&host).to_owned();
        let path = parsed.path().to_ascii_lowercase();
        let rest = match parsed.query() {
            Some(query) => format!("{path}?{}", query.to_ascii_lowercase()),
            None => path.clone(),
        };
        Some(Self {
            host,
            domain,
            path,
            rest,
        })
    }

    /// The name of the site without its ending, such as `example` for `example.co.uk`.
    fn name(&self) -> &str {
        self.domain.split('.').next().unwrap_or_default()
    }

    /// The ending of the site, such as `org` or `uk`.
    fn ending(&self) -> &str {
        self.domain.rsplit('.').next().unwrap_or_default()
    }

    /// Whether this address names the other one in its own path or query, as a search result or
    /// a click counter does.
    fn carries(&self, other: &Self) -> bool {
        !other.domain.is_empty() && self.rest.contains(&other.domain)
    }

    /// Whether the first part of the path says that the page sends people on.
    fn sends_on(&self) -> bool {
        let first = self.path.trim_start_matches('/');
        let first = first.split('/').next().unwrap_or_default();
        let first = first.split('.').next().unwrap_or_default();
        SENDING_ON.contains(&first)
    }

    fn has_any(&self, words: &[&str]) -> bool {
        words.iter().any(|word| self.rest.contains(word))
    }

    /// Whether the host is written as a number instead of a name.
    fn is_a_number(&self) -> bool {
        self.host.parse::<std::net::IpAddr>().is_ok()
    }

    /// Whether the name of the site reads like something nobody chose to be read: a long row of
    /// letters with digits in it, or without the vowels a word needs.
    fn looks_made_up(&self) -> bool {
        let name = self.name();
        let letters = name.chars().count();
        if letters < 8 {
            return false;
        }
        let digits = name.chars().filter(char::is_ascii_digit).count();
        if digits >= 4 {
            return true;
        }
        let vowels = name
            .chars()
            .filter(|letter| VOWELS.contains(letter))
            .count();
        letters >= 12 && (vowels as f32) / (letters as f32) < 0.25
    }
}

const VOWELS: [char; 6] = ['a', 'e', 'i', 'o', 'u', 'y'];

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt<'a>(page: &'a str, target: &'a str) -> Attempt<'a> {
        Attempt {
            page_url: page,
            target_url: target,
            since_acted: None,
            since_asked: None,
            hops: 1,
            on_a_list: false,
        }
    }

    fn just_clicked<'a>(page: &'a str, target: &'a str) -> Attempt<'a> {
        Attempt {
            since_acted: Some(Duration::from_millis(400)),
            ..attempt(page, target)
        }
    }

    fn judged(attempt: &Attempt<'_>) -> (bool, String) {
        let judgement = judge(attempt);
        (judgement.stops_it(), judgement.reasons())
    }

    #[test]
    fn what_the_user_set_off_themselves_goes_through() {
        let (stopped, why) = judged(&just_clicked(
            "https://forum.example.org/thread/5",
            "https://downloads.other.test/file",
        ));
        assert!(!stopped, "{why}");
    }

    #[test]
    fn a_while_after_a_key_was_pressed_advertising_is_still_stopped() {
        let attempt = Attempt {
            since_acted: Some(Duration::from_secs(9)),
            ..attempt(
                "https://news.example.org/artikel",
                "https://anderswo.test/landing?aff_id=9",
            )
        };
        let (stopped, why) = judged(&attempt);
        assert!(stopped, "{why}");
    }

    #[test]
    fn a_while_after_a_key_was_pressed_an_ordinary_address_goes_through() {
        let attempt = Attempt {
            since_acted: Some(Duration::from_secs(9)),
            ..attempt(
                "https://news.example.org/artikel",
                "https://anderswo.test/mehr",
            )
        };
        let (stopped, why) = judged(&attempt);
        assert!(!stopped, "{why}");
    }

    #[test]
    fn a_page_left_alone_for_a_long_time_may_not_send_anyone_anywhere() {
        let attempt = Attempt {
            since_acted: Some(Duration::from_secs(120)),
            ..attempt(
                "https://news.example.org/artikel",
                "https://winner.top/go?aff_id=77",
            )
        };
        let (stopped, why) = judged(&attempt);
        assert!(stopped, "{why}");
    }

    #[test]
    fn a_page_just_opened_may_hand_over_to_where_it_has_moved() {
        let attempt = Attempt {
            since_asked: Some(Duration::from_millis(800)),
            ..attempt("https://example.org/", "https://example.test/")
        };
        let (stopped, why) = judged(&attempt);
        assert!(!stopped, "{why}");
    }

    #[test]
    fn a_page_just_opened_may_still_not_throw_anyone_at_advertising() {
        let attempt = Attempt {
            since_asked: Some(Duration::from_millis(800)),
            ..attempt(
                "https://stream.example.org/film",
                "https://winner.top/go?aff_id=9",
            )
        };
        let (stopped, why) = judged(&attempt);
        assert!(stopped, "{why}");
    }

    #[test]
    fn a_page_nobody_has_touched_keeps_to_itself() {
        let (stopped, why) = judged(&attempt(
            "https://news.example.org/artikel",
            "https://somewhere.test/else",
        ));
        assert!(stopped, "{why}");
    }

    #[test]
    fn the_filter_lists_settle_it() {
        let attempt = Attempt {
            on_a_list: true,
            ..just_clicked(
                "https://video.example.org/watch",
                "https://ads.test/landing",
            )
        };
        let (stopped, why) = judged(&attempt);
        assert!(
            stopped,
            "even a click does not carry a known address: {why}"
        );
    }

    #[test]
    fn another_host_of_the_same_site_is_not_elsewhere() {
        let (stopped, why) = judged(&attempt(
            "https://example.org/start",
            "https://accounts.example.org/anmelden",
        ));
        assert!(!stopped, "{why}");
    }

    #[test]
    fn signing_in_walks_between_sites() {
        let attempt = Attempt {
            since_acted: Some(Duration::from_secs(6)),
            hops: 3,
            ..attempt(
                "https://shop.example.org/kasse",
                "https://login.identity.test/oauth/authorize?client_id=7&redirect_uri=x",
            )
        };
        let (stopped, why) = judged(&attempt);
        assert!(!stopped, "{why}");
    }

    #[test]
    fn a_search_result_leads_where_it_says_it_does() {
        let attempt = attempt(
            "https://www.google.com/url?q=https%3A%2F%2Fexample.org%2Fartikel",
            "https://example.org/artikel",
        );
        let (stopped, why) = judged(&attempt);
        assert!(!stopped, "{why}");
    }

    #[test]
    fn a_short_address_is_there_to_send_people_on() {
        let (stopped, why) = judged(&attempt(
            "https://t.co/abcdef",
            "https://example.org/artikel",
        ));
        assert!(!stopped, "{why}");
    }

    #[test]
    fn a_chain_that_never_ends_is_stopped() {
        let attempt = Attempt {
            hops: 12,
            ..just_clicked("https://a.test/x", "https://b.test/y")
        };
        let (stopped, why) = judged(&attempt);
        assert!(stopped, "{why}");
    }

    #[test]
    fn made_up_names_are_recognised() {
        let made_up = Address::of("https://x7k2m9qp4z.test/a").unwrap();
        assert!(made_up.looks_made_up());
        let ordinary = Address::of("https://tagesschau.de/a").unwrap();
        assert!(!ordinary.looks_made_up());
        let short = Address::of("https://ab12.test/a").unwrap();
        assert!(!short.looks_made_up(), "a short name is nobody's business");
    }

    #[test]
    fn a_site_is_read_with_its_whole_ending() {
        let address = Address::of("https://shop.example.co.uk/a?b=c").unwrap();
        assert_eq!(address.domain, "example.co.uk");
        assert_eq!(address.name(), "example");
        assert_eq!(address.ending(), "uk");
    }

    #[test]
    fn a_page_that_sends_people_on_says_so_in_its_path() {
        assert!(Address::of("https://a.test/go/1234").unwrap().sends_on());
        assert!(Address::of("https://a.test/out?u=x").unwrap().sends_on());
        assert!(
            Address::of("https://a.test/url.php?u=x")
                .unwrap()
                .sends_on()
        );
        assert!(!Address::of("https://a.test/golf/1").unwrap().sends_on());
        assert!(!Address::of("https://a.test/a/go").unwrap().sends_on());
    }
}
