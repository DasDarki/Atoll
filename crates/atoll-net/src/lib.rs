//! Where the requests Atoll makes itself go.
//!
//! Besides the pages, Atoll fetches a few things on its own: the filter lists, the question to
//! the Internet Archive, what extensions ask for, the videos yt-dlp loads. While the user's VPN
//! carries the browser, those have to take the same way, or the browser would keep a side door
//! open that gives away the address the VPN is there to hide.
//!
//! The way is a single port for the whole program, so it lives here as one value every part of
//! Atoll reads, instead of being threaded through every call that might one day fetch something.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The port of the loopback proxy, or `0` while requests go out directly.
static ROUTE: AtomicU32 = AtomicU32::new(0);

/// Sends everything Atoll fetches through the proxy on that port, or directly again.
pub fn route_through(port: Option<u16>) {
    ROUTE.store(u32::from(port.unwrap_or_default()), Ordering::SeqCst);
}

/// The port everything goes through, if there is one.
pub fn route() -> Option<u16> {
    match ROUTE.load(Ordering::SeqCst) {
        0 => None,
        port => u16::try_from(port).ok(),
    }
}

/// The proxy as a program like yt-dlp wants it written.
pub fn proxy_address() -> Option<String> {
    route().map(|port| format!("socks5h://127.0.0.1:{port}"))
}

/// An agent for one request, which takes the way everything else takes. Names are looked up at
/// the other end of the proxy, never here, because a lookup would give the address away.
pub fn agent(timeout: Duration) -> ureq::Agent {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("Atoll/", env!("CARGO_PKG_VERSION")));
    if let Some(address) = proxy_address()
        && let Ok(proxy) = ureq::Proxy::new(&address)
    {
        config = config.proxy(Some(proxy));
    }
    config.build().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The way is one value for the whole program, so this is one test and not two: two would
    /// set it against each other while they run side by side.
    #[test]
    fn the_way_out_is_remembered_as_other_programs_read_it() {
        route_through(None);
        assert_eq!(route(), None);
        assert_eq!(proxy_address(), None);

        route_through(Some(4711));
        assert_eq!(route(), Some(4711));
        assert_eq!(
            proxy_address().as_deref(),
            Some("socks5h://127.0.0.1:4711"),
            "the h says the proxy looks the name up, not this computer"
        );

        route_through(None);
        assert_eq!(proxy_address(), None);
    }
}
