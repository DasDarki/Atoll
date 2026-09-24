//! Tor for Atoll, through Arti.
//!
//! The browser engine only speaks to proxies, so this runs a small SOCKS server on the loopback
//! address that hands every connection to Arti, which carries it through the Tor network. The
//! whole thing lives on a thread of its own with its own runtime: starting Tor takes a while and
//! must not hold up the window, so the browser asks for the [`Status`] whenever it wants to know
//! how far it is.
//!
//! This hides the address a site sees. It is not the Tor Browser: nothing here changes what a
//! page can read about fonts, screen or canvas, so a site can still recognise the browser itself.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arti_client::{TorClient, TorClientConfig};
use atoll_socks::{Carried, Connect, Opening};
use tor_rtcompat::Runtime as TorRuntime;

/// How far Tor is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nobody asked for Tor yet.
    Idle,
    /// Arti is finding its way into the network.
    Starting,
    /// Ready, with the port the engine sends its requests to.
    Ready(u16),
    /// It did not work, in the words the user gets to see.
    Failed(String),
}

impl Status {
    pub fn port(&self) -> Option<u16> {
        match self {
            Status::Ready(port) => Some(*port),
            _ => None,
        }
    }
}

/// The way into the Tor network. Starting it is asked for once and then takes care of itself.
#[derive(Debug, Clone, Default)]
pub struct Tor {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    status: Mutex<Option<Status>>,
    started: AtomicBool,
}

impl Inner {
    fn set(&self, status: Status) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = Some(status);
    }
}

impl Tor {
    /// What Tor is doing right now.
    pub fn status(&self) -> Status {
        self.inner
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or(Status::Idle)
    }

    /// Starts Tor unless it is already running or on its way. Answers right away, the work
    /// happens on a thread of its own.
    pub fn start(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = self.inner.clone();
        inner.set(Status::Starting);
        let spawned = std::thread::Builder::new()
            .name("atoll-tor".to_owned())
            .spawn(move || run(&inner));
        if let Err(error) = spawned {
            self.inner.set(Status::Failed(error.to_string()));
            self.inner.started.store(false, Ordering::SeqCst);
        }
    }

    /// Lets Tor be started again after it failed.
    pub fn forget_failure(&self) {
        if matches!(self.status(), Status::Failed(_)) {
            self.inner.started.store(false, Ordering::SeqCst);
            self.inner.set(Status::Idle);
        }
    }
}

/// The life of the Tor thread: take a port, start Arti, then serve what the engine sends.
fn run(inner: &Arc<Inner>) {
    let listener =
        match TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))) {
            Ok(listener) => listener,
            Err(error) => {
                inner.set(Status::Failed(error.to_string()));
                return;
            }
        };
    if let Err(error) = listener.set_nonblocking(true) {
        inner.set(Status::Failed(error.to_string()));
        return;
    }
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(error) => {
            inner.set(Status::Failed(error.to_string()));
            return;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            inner.set(Status::Failed(error.to_string()));
            return;
        }
    };
    runtime.block_on(async move {
        let client = match TorClient::create_bootstrapped(TorClientConfig::default()).await {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "Tor could not be started");
                inner.set(Status::Failed(error.to_string()));
                return;
            }
        };
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                inner.set(Status::Failed(error.to_string()));
                return;
            }
        };
        tracing::info!(port, "Tor is ready");
        inner.set(Status::Ready(port));
        atoll_socks::serve(listener, Arc::new(ThroughTor { client })).await;
    });
}

/// Carries a connection of the SOCKS server through the Tor network.
struct ThroughTor<R: TorRuntime> {
    client: Arc<TorClient<R>>,
}

impl<R: TorRuntime> Connect for ThroughTor<R> {
    fn connect(&self, host: String, port: u16) -> Opening {
        let client = self.client.clone();
        Box::pin(async move {
            let stream = client
                .connect((host.as_str(), port))
                .await
                .map_err(std::io::Error::other)?;
            Ok(Box::pin(stream) as Carried)
        })
    }
}
