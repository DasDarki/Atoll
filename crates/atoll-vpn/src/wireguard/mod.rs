//! A WireGuard tunnel Atoll builds itself.
//!
//! The user hands Atoll the configuration their service gave them, and the tunnel is built inside
//! the browser: boringtun speaks the protocol, smoltcp is the network stack that lives in the
//! tunnel, and a UDP socket carries the encrypted packets to the server. Nothing of this touches
//! the system, so no root is needed and nothing changes for the other programs on the computer.
//!
//! Everything happens in one task. The connections of the SOCKS server reach it as orders and
//! leave as a pair of pipes, so no lock is needed anywhere and the protocol stack is only ever
//! touched from one place.

mod config;
mod device;

pub use config::Config;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet};
use smoltcp::socket::{dns, tcp};
use smoltcp::time::Instant as Tick;
use smoltcp::wire::{DnsQueryType, HardwareAddress, IpAddress, IpCidr};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

use atoll_i18n::t;

use crate::{Inner, Status};
use device::Device;

/// The largest packet the tunnel ever handles.
const MAX_PACKET: usize = 65_536;

/// How often the timers of the protocol are driven: handshakes, keepalives, retries.
const TICK: Duration = Duration::from_millis(150);

/// How long the tunnel may stay without a handshake before Atoll calls it broken.
const PATIENCE: Duration = Duration::from_secs(20);

/// A handshake older than this means the tunnel is not standing any more.
const HANDSHAKE_LASTS: Duration = Duration::from_secs(180);

/// How long one connection may take to come up before the tunnel is called broken. A tunnel
/// whose other side went away answers nothing at all, and the browser must not wait for that.
const OPENING_LASTS: Duration = Duration::from_secs(12);

/// How much of a connection is held in the pipes to the browser.
const PIPE: usize = 64 * 1024;

/// What one connection may hold in the stack in each direction.
const WINDOW: usize = 32 * 1024;

/// What the SOCKS side asks of the tunnel.
pub(crate) enum Command {
    Open {
        host: String,
        port: u16,
        answer: oneshot::Sender<io::Result<DuplexStream>>,
    },
}

/// The tunnel, as the rest of the VPN sees it.
pub(crate) struct Handle {
    /// What it was built from, so a changed configuration is noticed.
    pub(crate) config: Config,
    orders: mpsc::Sender<Command>,
}

impl Handle {
    /// Whether the tunnel behind this gave up, so it has to be built again.
    pub(crate) fn is_gone(&self) -> bool {
        self.orders.is_closed()
    }

    /// Opens a connection to `host`:`port` inside the tunnel.
    pub(crate) async fn open(&self, host: String, port: u16) -> io::Result<DuplexStream> {
        let (answer, answered) = oneshot::channel();
        self.orders
            .send(Command::Open { host, port, answer })
            .await
            .map_err(|_| io::Error::other(t!("Der Tunnel läuft nicht mehr")))?;
        answered
            .await
            .map_err(|_| io::Error::other(t!("Der Tunnel hat nicht geantwortet")))?
    }
}

/// Builds the tunnel and keeps it running until it is dropped.
pub(crate) fn start(config: Config, inner: Arc<Inner>, port: u16) -> Handle {
    let (orders, taken) = mpsc::channel(32);
    let handle = Handle {
        config: config.clone(),
        orders,
    };
    tokio::spawn(async move {
        if let Err(trouble) = run(config, inner.clone(), taken, port).await {
            tracing::warn!(trouble, "the tunnel ended");
            inner.set(Status::Down(trouble));
        }
    });
    handle
}

/// What one connection through the tunnel holds while it lives.
struct Connection {
    socket: SocketHandle,
    /// Bytes on their way to the browser.
    to_browser: mpsc::Sender<Vec<u8>>,
    /// Bytes from the browser the stack has not taken yet.
    waiting: VecDeque<Vec<u8>>,
    /// Whether the browser side said it is finished.
    browser_done: bool,
}

/// A connection that is not open yet.
enum Waiting {
    /// The name is being looked up, inside the tunnel.
    Name {
        query: dns::QueryHandle,
        host: String,
        port: u16,
        since: Instant,
        answer: oneshot::Sender<io::Result<DuplexStream>>,
    },
    /// The stack is opening the connection.
    Opening {
        socket: SocketHandle,
        since: Instant,
        answer: oneshot::Sender<io::Result<DuplexStream>>,
    },
}

impl Waiting {
    /// Whether it has been waiting longer than anything inside a working tunnel would.
    fn too_long(&self) -> bool {
        let since = match self {
            Waiting::Name { since, .. } | Waiting::Opening { since, .. } => since,
        };
        since.elapsed() > OPENING_LASTS
    }
}

/// What the tasks of a connection tell the tunnel.
enum Note {
    FromBrowser(usize, Vec<u8>),
    BrowserDone(usize),
}

async fn run(
    config: Config,
    inner: Arc<Inner>,
    mut orders: mpsc::Receiver<Command>,
    port: u16,
) -> Result<(), String> {
    let endpoint = where_the_server_is(&config.endpoint).await?;
    let udp = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .map_err(|error| t!("Kein Platz für den Tunnel: {}", error))?;
    udp.connect(endpoint)
        .await
        .map_err(|error| t!("Der VPN-Server ist nicht erreichbar: {}", error))?;

    let mut tunn = Tunn::new(
        StaticSecret::from(config.private_key),
        PublicKey::from(config.peer_key),
        config.preshared_key,
        config.keepalive,
        0,
        None,
    );
    let mut device = Device::new(config.mtu);
    let mut iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ip),
        &mut device,
        Tick::now(),
    );
    iface.update_ip_addrs(|addresses| {
        let _ = addresses.push(IpCidr::new(IpAddress::Ipv4(config.address), 32));
    });
    // Everything the browser asks for lies outside the tunnel's own address, so it all goes to
    // the server. With a tunnel there is no hardware below, so the address only picks the way.
    iface
        .routes_mut()
        .add_default_ipv4_route(config.dns)
        .map_err(|_| t!("Der Weg durch den Tunnel ließ sich nicht eintragen").to_owned())?;

    let mut sockets = SocketSet::new(Vec::new());
    let resolver = sockets.add(dns::Socket::new(
        &[IpAddress::Ipv4(config.dns)],
        vec![None, None, None, None],
    ));

    let (notes, mut noted) = mpsc::channel::<Note>(64);
    let mut connections: HashMap<usize, Connection> = HashMap::new();
    let mut waiting: Vec<Waiting> = Vec::new();
    let mut next_connection = 0usize;
    let mut next_port = 49_152u16;
    let mut datagram = vec![0u8; MAX_PACKET];
    let mut scratch = vec![0u8; MAX_PACKET];
    let started = Instant::now();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // The first handshake is asked for at once, so the way is open before the user types.
    if let TunnResult::WriteToNetwork(packet) =
        tunn.format_handshake_initiation(&mut scratch, false)
        && udp.send(packet).await.is_err()
    {
        return Err(t!("Der erste Gruß an den VPN-Server kam nicht an").to_owned());
    }

    loop {
        tokio::select! {
            received = udp.recv(&mut datagram) => {
                let Ok(size) = received else {
                    return Err(t!("Die Verbindung zum VPN-Server ist abgerissen").to_owned());
                };
                let mut read = &datagram[..size];
                loop {
                    match tunn.decapsulate(Some(endpoint.ip()), read, &mut scratch) {
                        TunnResult::WriteToNetwork(packet) => {
                            let _ = udp.send(packet).await;
                            // Repeating with nothing hands out what is still queued.
                            read = &[];
                        }
                        TunnResult::WriteToTunnelV4(packet, _) => {
                            device.inbox.push_back(packet.to_vec());
                            break;
                        }
                        TunnResult::WriteToTunnelV6(_, _) | TunnResult::Done => break,
                        TunnResult::Err(error) => {
                            tracing::debug!(?error, "a packet of the VPN server was not understood");
                            break;
                        }
                    }
                }
            }
            order = orders.recv() => {
                let Some(Command::Open { host, port: wanted, answer }) = order else {
                    return Ok(());
                };
                match host.parse::<Ipv4Addr>() {
                    Ok(address) => {
                        let socket = open_socket(
                            &mut sockets, &mut iface, address, wanted, &mut next_port,
                        );
                        match socket {
                            Ok(socket) => waiting.push(Waiting::Opening {
                                socket,
                                since: Instant::now(),
                                answer,
                            }),
                            Err(trouble) => { let _ = answer.send(Err(trouble)); }
                        }
                    }
                    Err(_) => {
                        let query = sockets
                            .get_mut::<dns::Socket>(resolver)
                            .start_query(iface.context(), &host, DnsQueryType::A);
                        match query {
                            Ok(query) => waiting.push(Waiting::Name {
                                query,
                                host,
                                port: wanted,
                                since: Instant::now(),
                                answer,
                            }),
                            Err(_) => {
                                let _ = answer.send(Err(io::Error::other(t!(
                                    "Der Name konnte im Tunnel nicht nachgeschlagen werden"
                                ))));
                            }
                        }
                    }
                }
            }
            note = noted.recv() => {
                match note {
                    Some(Note::FromBrowser(id, chunk)) => {
                        if let Some(connection) = connections.get_mut(&id) {
                            connection.waiting.push_back(chunk);
                        }
                    }
                    Some(Note::BrowserDone(id)) => {
                        if let Some(connection) = connections.get_mut(&id) {
                            connection.browser_done = true;
                        }
                    }
                    None => {}
                }
            }
            _ = ticker.tick() => {
                let mut result = tunn.update_timers(&mut scratch);
                loop {
                    match result {
                        TunnResult::WriteToNetwork(packet) => {
                            let _ = udp.send(packet).await;
                            break;
                        }
                        TunnResult::Err(boringtun::noise::errors::WireGuardError::ConnectionExpired) => {
                            result = tunn.format_handshake_initiation(&mut scratch, false);
                        }
                        _ => break,
                    }
                }
            }
        }

        iface.poll(Tick::now(), &mut device, &mut sockets);
        let gave_up = answer_the_waiting(
            &mut waiting,
            &mut sockets,
            &mut iface,
            resolver,
            &mut connections,
            &mut next_connection,
            &mut next_port,
            &notes,
        );
        if gave_up {
            // Nothing comes through any more, whatever the last handshake says.
            inner.set(Status::Down(
                t!("Der VPN-Server antwortet nicht mehr").to_owned(),
            ));
        }
        carry_bytes(&mut connections, &mut sockets);
        iface.poll(Tick::now(), &mut device, &mut sockets);

        while let Some(packet) = device.outbox.pop_front() {
            match tunn.encapsulate(&packet, &mut scratch) {
                TunnResult::WriteToNetwork(encrypted) => {
                    let _ = udp.send(encrypted).await;
                }
                TunnResult::Err(error) => {
                    tracing::debug!(?error, "a packet could not be sent through the tunnel");
                }
                _ => {}
            }
        }

        let standing = !gave_up
            && tunn
                .time_since_last_handshake()
                .is_some_and(|since| since < HANDSHAKE_LASTS);
        if standing {
            if !inner.status().is_up() {
                tracing::info!(port, "the tunnel is up");
                inner.set(Status::Up(port));
            }
        } else if started.elapsed() > PATIENCE && inner.status().is_up() {
            inner.set(Status::Down(
                t!("Der VPN-Server antwortet nicht mehr").to_owned(),
            ));
        } else if started.elapsed() > PATIENCE {
            inner.set(Status::Down(
                t!("Der VPN-Server antwortet nicht").to_owned(),
            ));
        }
    }
}

/// Where the server is, asked outside the tunnel because there is no tunnel yet. That is the one
/// lookup a VPN cannot hide, and every WireGuard client makes it.
async fn where_the_server_is(endpoint: &str) -> Result<SocketAddr, String> {
    tokio::net::lookup_host(endpoint)
        .await
        .map_err(|error| t!("Der VPN-Server {} ist nicht zu finden: {}", endpoint, error))?
        .find(|address| address.is_ipv4())
        .ok_or_else(|| t!("Der VPN-Server {} hat keine IPv4-Adresse", endpoint))
}

/// Opens a connection inside the tunnel.
fn open_socket(
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    address: Ipv4Addr,
    port: u16,
    next_port: &mut u16,
) -> io::Result<SocketHandle> {
    let socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; WINDOW]),
        tcp::SocketBuffer::new(vec![0u8; WINDOW]),
    );
    let handle = sockets.add(socket);
    *next_port = if *next_port == u16::MAX {
        49_152
    } else {
        *next_port + 1
    };
    let opened = sockets.get_mut::<tcp::Socket>(handle).connect(
        iface.context(),
        (IpAddress::Ipv4(address), port),
        *next_port,
    );
    match opened {
        Ok(()) => Ok(handle),
        Err(error) => {
            sockets.remove(handle);
            Err(io::Error::other(t!(
                "Der Tunnel konnte die Verbindung nicht öffnen: {}",
                error
            )))
        }
    }
}

/// Looks after the connections that are not open yet: names being looked up and sockets on their
/// way, and hands the browser its pipe once a connection stands.
/// Answers with whether something was given up on, which says the tunnel is not carrying.
#[allow(clippy::too_many_arguments)]
fn answer_the_waiting(
    waiting: &mut Vec<Waiting>,
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    resolver: SocketHandle,
    connections: &mut HashMap<usize, Connection>,
    next_connection: &mut usize,
    next_port: &mut u16,
    notes: &mpsc::Sender<Note>,
) -> bool {
    let mut still_waiting = Vec::with_capacity(waiting.len());
    let mut gave_up = false;
    for entry in waiting.drain(..) {
        if entry.too_long() {
            gave_up = true;
            match entry {
                Waiting::Name { host, answer, .. } => {
                    let _ = answer.send(Err(io::Error::other(t!(
                        "{} ist im Tunnel nicht aufzulösen",
                        host
                    ))));
                }
                Waiting::Opening { socket, answer, .. } => {
                    sockets.get_mut::<tcp::Socket>(socket).abort();
                    sockets.remove(socket);
                    let _ = answer.send(Err(io::Error::other(t!(
                        "Der VPN-Server antwortet nicht mehr"
                    ))));
                }
            }
            continue;
        }
        match entry {
            Waiting::Name {
                query,
                host,
                port,
                since,
                answer,
            } => {
                let found = sockets
                    .get_mut::<dns::Socket>(resolver)
                    .get_query_result(query);
                match found {
                    Err(dns::GetQueryResultError::Pending) => still_waiting.push(Waiting::Name {
                        query,
                        host,
                        port,
                        since,
                        answer,
                    }),
                    Err(dns::GetQueryResultError::Failed) => {
                        let _ = answer.send(Err(io::Error::other(t!(
                            "{} ist im Tunnel nicht aufzulösen",
                            host
                        ))));
                    }
                    Ok(addresses) => {
                        let address = addresses.iter().find_map(|address| match address {
                            IpAddress::Ipv4(address) => Some(*address),
                            #[allow(unreachable_patterns)]
                            _ => None,
                        });
                        match address {
                            Some(address) => {
                                match open_socket(sockets, iface, address, port, next_port) {
                                    Ok(socket) => {
                                        still_waiting.push(Waiting::Opening {
                                            socket,
                                            since: Instant::now(),
                                            answer,
                                        });
                                    }
                                    Err(trouble) => {
                                        let _ = answer.send(Err(trouble));
                                    }
                                }
                            }
                            None => {
                                let _ = answer.send(Err(io::Error::other(t!(
                                    "{} hat keine IPv4-Adresse",
                                    host
                                ))));
                            }
                        }
                    }
                }
            }
            Waiting::Opening {
                socket,
                since,
                answer,
            } => {
                let state = sockets.get::<tcp::Socket>(socket).state();
                match state {
                    tcp::State::Established => {
                        let id = *next_connection;
                        *next_connection += 1;
                        let (browser_end, tunnel_end) = tokio::io::duplex(PIPE);
                        let (to_browser, from_tunnel) = mpsc::channel::<Vec<u8>>(8);
                        spawn_pipes(id, tunnel_end, from_tunnel, notes.clone());
                        connections.insert(
                            id,
                            Connection {
                                socket,
                                to_browser,
                                waiting: VecDeque::new(),
                                browser_done: false,
                            },
                        );
                        let _ = answer.send(Ok(browser_end));
                    }
                    tcp::State::Closed | tcp::State::TimeWait => {
                        sockets.remove(socket);
                        let _ = answer.send(Err(io::Error::other(
                            "die Gegenstelle hat die Verbindung abgelehnt",
                        )));
                    }
                    _ => still_waiting.push(Waiting::Opening {
                        socket,
                        since,
                        answer,
                    }),
                }
            }
        }
    }
    *waiting = still_waiting;
    gave_up
}

/// The two tasks of one connection: what the browser sends becomes a note to the tunnel, and what
/// the tunnel answers is written back to the browser.
fn spawn_pipes(
    id: usize,
    tunnel_end: DuplexStream,
    mut from_tunnel: mpsc::Receiver<Vec<u8>>,
    notes: mpsc::Sender<Note>,
) {
    let (mut reading, mut writing) = tokio::io::split(tunnel_end);
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match reading.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if notes
                        .send(Note::FromBrowser(id, buffer[..read].to_vec()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let _ = notes.send(Note::BrowserDone(id)).await;
    });
    tokio::spawn(async move {
        while let Some(chunk) = from_tunnel.recv().await {
            if writing.write_all(&chunk).await.is_err() {
                return;
            }
        }
        let _ = writing.shutdown().await;
    });
}

/// Moves what is there between the stack and the pipes of the browser, and clears away what is
/// finished. Nothing is forced: a full pipe simply stays full, which slows the other side down.
fn carry_bytes(connections: &mut HashMap<usize, Connection>, sockets: &mut SocketSet<'static>) {
    connections.retain(|_, connection| {
        let socket = sockets.get_mut::<tcp::Socket>(connection.socket);

        while let Some(chunk) = connection.waiting.front() {
            if !socket.can_send() {
                break;
            }
            match socket.send_slice(chunk) {
                Ok(0) => break,
                Ok(sent) if sent == chunk.len() => {
                    connection.waiting.pop_front();
                }
                Ok(sent) => {
                    connection.waiting[0].drain(..sent);
                    break;
                }
                Err(_) => break,
            }
        }

        while socket.can_recv() && connection.to_browser.capacity() > 0 {
            let mut chunk = Vec::new();
            let read = socket.recv(|waiting| {
                let take = waiting.len().min(16 * 1024);
                chunk.extend_from_slice(&waiting[..take]);
                (take, take)
            });
            match read {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if connection.to_browser.try_send(chunk).is_err() {
                        break;
                    }
                }
            }
        }

        if connection.browser_done && connection.waiting.is_empty() && socket.may_send() {
            socket.close();
        }
        if socket.state() == tcp::State::Closed {
            sockets.remove(connection.socket);
            return false;
        }
        true
    });
}
