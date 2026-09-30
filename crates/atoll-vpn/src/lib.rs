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
mod wireguard;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use atoll_i18n::t;
use atoll_socks::{Carried, Connect, Opening};

pub use upstream::{Kind, Upstream};
pub use wireguard::Config as TunnelConfig;

/// How the browser gets out: through the proxy of a service, or through a tunnel Atoll builds
/// itself from the configuration the service handed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Way {
    Proxy(Upstream),
    Tunnel(Box<TunnelConfig>),
}

impl Default for Way {
    fn default() -> Self {
        Way::Proxy(Upstream::default())
    }
}

impl Way {
    /// Whether enough is known to try at all.
    pub fn is_complete(&self) -> bool {
        match self {
            Way::Proxy(upstream) => upstream.is_complete(),
            Way::Tunnel(_) => true,
        }
    }
}

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
    way: Mutex<Way>,
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

    fn way(&self) -> Way {
        self.way
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

    /// Starts carrying connections the given way. The bridge has its port before this answers,
    /// so the engine can be pointed at it at once; whether the way out stands is what
    /// [`Vpn::status`] then says.
    pub fn start(&self, way: Way) {
        *self
            .inner
            .way
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = way;
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
        let carrier = Arc::new(ThroughVpn {
            inner: inner.clone(),
            tunnel: Arc::default(),
        });
        let watching = tokio::spawn(watch(inner.clone(), port, carrier.clone()));
        atoll_socks::serve(listener, carrier).await;
        watching.abort();
    });
}

/// Looks after the way out: a proxy is asked now and then whether it is still there, and a
/// tunnel is built, or built again once its configuration changed. A way that broke is noticed
/// before the user types the next address.
async fn watch(inner: Arc<Inner>, port: u16, carrier: Arc<ThroughVpn>) {
    loop {
        if !inner.wanted.load(Ordering::SeqCst) {
            carrier.forget_tunnel().await;
            inner.set(Status::Off);
            tokio::time::sleep(WHILE_DOWN).await;
            continue;
        }
        let settled = match inner.way() {
            Way::Proxy(upstream) => {
                carrier.forget_tunnel().await;
                if upstream.is_complete() {
                    match upstream::reach(&upstream).await {
                        Ok(()) => Status::Up(port),
                        Err(error) => Status::Down(error.to_string()),
                    }
                } else {
                    Status::Down(t!("Kein Proxy eingetragen").to_owned())
                }
            }
            // The tunnel says itself how it is doing, because only it knows whether the other
            // side still answers.
            Way::Tunnel(config) => {
                carrier.keep_tunnel(*config, inner.clone(), port).await;
                inner.status()
            }
        };
        if inner.status() != settled {
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

/// Carries a connection of the SOCKS server out through the way the user chose, or refuses it:
/// that refusal is the killswitch.
struct ThroughVpn {
    inner: Arc<Inner>,
    /// The tunnel, while one is wanted. It is built by [`watch`] and used from here.
    tunnel: Arc<tokio::sync::Mutex<Option<wireguard::Handle>>>,
}

impl ThroughVpn {
    /// Builds the tunnel, or builds it again after its configuration changed.
    async fn keep_tunnel(&self, config: wireguard::Config, inner: Arc<Inner>, port: u16) {
        let mut held = self.tunnel.lock().await;
        let standing = held
            .as_ref()
            .is_some_and(|tunnel| tunnel.config == config && !tunnel.is_gone());
        if standing {
            return;
        }
        inner.set(Status::Connecting);
        *held = Some(wireguard::start(config, inner, port));
    }

    /// Takes the tunnel down, because the user chose another way or switched the VPN off.
    async fn forget_tunnel(&self) {
        self.tunnel.lock().await.take();
    }
}

impl Connect for ThroughVpn {
    fn connect(&self, host: String, port: u16) -> Opening {
        let inner = self.inner.clone();
        let tunnel = self.tunnel.clone();
        Box::pin(async move {
            if !inner.wanted.load(Ordering::SeqCst) {
                return Err(std::io::Error::other(t!("Das VPN ist ausgeschaltet")));
            }
            let carried = match inner.way() {
                Way::Proxy(upstream) if upstream.is_complete() => {
                    upstream::open(&upstream, &host, port).await
                }
                Way::Proxy(_) => Err(std::io::Error::other(t!("Kein Proxy eingetragen"))),
                Way::Tunnel(_) => match tunnel.lock().await.as_ref() {
                    Some(tunnel) => tunnel
                        .open(host.clone(), port)
                        .await
                        .map(|stream| Box::pin(stream) as Carried),
                    None => Err(std::io::Error::other(t!("Der Tunnel steht noch nicht"))),
                },
            };
            match carried {
                Ok(stream) => {
                    if !inner.status().is_up()
                        && let Some(port) = inner.status().port()
                    {
                        inner.set(Status::Up(port));
                    }
                    Ok(stream)
                }
                Err(error) => {
                    tracing::debug!(%error, host, port, "the VPN could not carry a connection");
                    if matches!(inner.way(), Way::Proxy(_)) {
                        inner.set(Status::Down(error.to_string()));
                    }
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
    fn asking_for_it_remembers_the_way_and_that_it_is_wanted() {
        let vpn = Vpn::default();
        let way = Way::Proxy(Upstream {
            kind: Kind::Socks5,
            host: "proxy.example".to_owned(),
            port: 1080,
            ..Upstream::default()
        });
        vpn.start(way.clone());
        assert!(vpn.is_wanted());
        assert_eq!(vpn.inner.way(), way);
        vpn.stop();
        assert!(!vpn.is_wanted());
        assert_eq!(vpn.status(), Status::Off);
    }

    #[tokio::test]
    async fn nothing_is_carried_while_the_way_out_is_not_wanted() {
        let inner = Arc::new(Inner::default());
        let through = ThroughVpn {
            inner: inner.clone(),
            tunnel: Arc::default(),
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
