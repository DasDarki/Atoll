//! The VPN of the user, and the killswitch that comes with it.
//!
//! Atoll brings no VPN service of its own. The user names the proxy of the service they pay for,
//! and Atoll puts a SOCKS server on the loopback address in front of it: the engine only speaks
//! to proxies, Chromium cannot hand a password to a SOCKS proxy, and a single point every
//! connection passes through is the only honest place for a killswitch.
//!
//! The killswitch is not a setting, it is how this works: while the way out is not known to be
//! open, every connection is refused here. Nothing falls back to the ordinary connection, because
//! a fallback is exactly the leak the user wanted to rule out.

mod upstream;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atoll_socks::{Carried, Connect, Opening};

pub use upstream::{Kind, Upstream};

/// How often the way out is tried again while it stands, and while it does not.
const WHILE_UP: Duration = Duration::from_secs(30);
const WHILE_DOWN: Duration = Duration::from_secs(5);

/// How far the VPN is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// Nobody asked for a VPN.
    #[default]
    Off,
    /// The proxy is being tried.
    Connecting,
    /// The way out stands, with the port the engine sends its requests to.
    Up(u16),
    /// It does not work, in the words the user gets to see. Nothing leaves the browser.
    Down(String),
}

impl Status {
    pub fn port(&self) -> Option<u16> {
        match self {
            Status::Up(port) => Some(*port),
            _ => None,
        }
    }

    pub fn is_up(&self) -> bool {
        matches!(self, Status::Up(_))
    }
}

/// The way out through the user's VPN. Asking for it is done once, the rest takes care of itself.
#[derive(Debug, Clone, Default)]
pub struct Vpn {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    status: Mutex<Status>,
    upstream: Mutex<Upstream>,
    /// Set while a bridge thread is running, cleared when it is asked to stop.
    running: AtomicBool,
    wanted: AtomicBool,
    /// The port of the bridge, known as soon as the VPN is asked for, so the engine can be
    /// pointed at it before anything is loaded.
    port: AtomicU32,
}

impl Inner {
    fn set(&self, status: Status) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }

    fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn upstream(&self) -> Upstream {
        self.upstream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Vpn {
    pub fn status(&self) -> Status {
        self.inner.status()
    }

    /// The port of the bridge, from the moment the VPN is asked for. It answers before the way
    /// out stands, and refuses everything until it does, which is the killswitch.
    pub fn port(&self) -> Option<u16> {
        match self.inner.port.load(Ordering::SeqCst) {
            0 => None,
            port => u16::try_from(port).ok(),
        }
    }

    /// Whether the user wants their connections carried through the VPN, whatever state it is in.
    pub fn is_wanted(&self) -> bool {
        self.inner.wanted.load(Ordering::SeqCst)
    }

    /// Starts carrying connections through `upstream`. The bridge has its port before this
    /// answers, so the engine can be pointed at it at once; whether the way out stands is what
    /// [`Vpn::status`] then says.
    pub fn start(&self, upstream: Upstream) {
        *self
            .inner
            .upstream
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = upstream;
        self.inner.wanted.store(true, Ordering::SeqCst);
        self.inner.set(Status::Connecting);
        if self.inner.running.swap(true, Ordering::SeqCst) {
            // The bridge is already up; it picks up the new proxy with the next check.
            return;
        }
        let listener = match bind() {
            Ok(listener) => listener,
            Err(error) => {
                self.inner.set(Status::Down(error.to_string()));
                self.inner.running.store(false, Ordering::SeqCst);
                return;
            }
        };
        let port = listener
            .local_addr()
            .map(|address| address.port())
            .unwrap_or_default();
        self.inner.port.store(u32::from(port), Ordering::SeqCst);
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name("atoll-vpn".to_owned())
            .spawn(move || run(&inner, listener, port));
        if let Err(error) = spawned {
            self.inner.set(Status::Down(error.to_string()));
            self.inner.running.store(false, Ordering::SeqCst);
        }
    }

    /// Stops carrying connections. The bridge stays on its port but refuses everything, so a
    /// request that is still on its way cannot slip out unprotected.
    pub fn stop(&self) {
        self.inner.wanted.store(false, Ordering::SeqCst);
        self.inner.set(Status::Off);
    }
}

/// Takes a port on the loopback address for the bridge.
fn bind() -> std::io::Result<TcpListener> {
    let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// The life of the VPN thread: serve what the engine sends and keep an eye on the way out.
fn run(inner: &Arc<Inner>, listener: TcpListener, port: u16) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            inner.set(Status::Down(error.to_string()));
            return;
        }
    };
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                inner.set(Status::Down(error.to_string()));
                return;
            }
        };
        let watching = tokio::spawn(watch(inner.clone(), port));
        atoll_socks::serve(
            listener,
            Arc::new(ThroughVpn {
                inner: inner.clone(),
            }),
        )
        .await;
        watching.abort();
    });
}

/// Asks the proxy now and then whether it is still there, so a way out that broke is noticed
/// before the user types the next address.
async fn watch(inner: Arc<Inner>, port: u16) {
    loop {
        if !inner.wanted.load(Ordering::SeqCst) {
            inner.set(Status::Off);
            tokio::time::sleep(WHILE_DOWN).await;
            continue;
        }
        let upstream = inner.upstream();
        let settled = if upstream.is_complete() {
            match upstream::reach(&upstream).await {
                Ok(()) => Status::Up(port),
                Err(error) => Status::Down(error.to_string()),
            }
        } else {
            Status::Down("no proxy is written down".to_owned())
        };
        let was = inner.status();
        if was != settled {
            match &settled {
                Status::Up(_) => tracing::info!(port, "the VPN is up"),
                Status::Down(reason) => tracing::warn!(reason, "the VPN is down"),
                _ => {}
            }
            inner.set(settled.clone());
        }
        tokio::time::sleep(if settled.is_up() {
            WHILE_UP
        } else {
            WHILE_DOWN
        })
        .await;
    }
}

/// Carries a connection of the SOCKS server through the proxy of the user's service, or refuses
/// it: that refusal is the killswitch.
struct ThroughVpn {
    inner: Arc<Inner>,
}

impl Connect for ThroughVpn {
    fn connect(&self, host: String, port: u16) -> Opening {
        let inner = self.inner.clone();
        Box::pin(async move {
            if !inner.wanted.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("the VPN was switched off"));
            }
            let upstream = inner.upstream();
            if !upstream.is_complete() {
                return Err(std::io::Error::other("no proxy is written down"));
            }
            match upstream::open(&upstream, &host, port).await {
                Ok(stream) => {
                    if !inner.status().is_up() {
                        inner.set(Status::Up(inner.status().port().unwrap_or_default()));
                    }
                    Ok(stream as Carried)
                }
                Err(error) => {
                    tracing::debug!(%error, host, port, "the VPN could not carry a connection");
                    inner.set(Status::Down(error.to_string()));
                    Err(error)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vpn_nobody_asked_for_carries_nothing() {
        let vpn = Vpn::default();
        assert_eq!(vpn.status(), Status::Off);
        assert_eq!(vpn.port(), None);
        assert!(!vpn.is_wanted());
    }

    #[test]
    fn asking_for_it_remembers_the_proxy_and_that_it_is_wanted() {
        let vpn = Vpn::default();
        let upstream = Upstream {
            kind: Kind::Socks5,
            host: "proxy.example".to_owned(),
            port: 1080,
            ..Upstream::default()
        };
        vpn.start(upstream.clone());
        assert!(vpn.is_wanted());
        assert_eq!(vpn.inner.upstream(), upstream);
        vpn.stop();
        assert!(!vpn.is_wanted());
        assert_eq!(vpn.status(), Status::Off);
    }

    #[tokio::test]
    async fn nothing_is_carried_while_the_way_out_is_not_wanted() {
        let inner = Arc::new(Inner::default());
        let through = ThroughVpn {
            inner: inner.clone(),
        };
        let refused = through.connect("example.org".to_owned(), 443).await;
        assert!(refused.is_err(), "the killswitch holds everything back");

        inner.wanted.store(true, Ordering::SeqCst);
        let without_proxy = through.connect("example.org".to_owned(), 443).await;
        assert!(
            without_proxy.is_err(),
            "and it holds while no proxy is written down"
        );
    }
}
