//! Speaking to the proxy of the user's VPN service.
//!
//! Two kinds are understood, because that is what the services hand out: a SOCKS5 proxy and a
//! proxy that takes `CONNECT`, the latter also with TLS on the way to it. Both carry the address
//! of the site as a name, so the name is looked up at the other end and not here.

use std::io;
use std::sync::Arc;

use atoll_socks::Carried;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// How the proxy of the service is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Socks5,
    /// A proxy that takes `CONNECT`, spoken to in the clear.
    Http,
    /// The same, with TLS on the way to the proxy, so the local network learns nothing.
    Https,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Socks5 => "socks5",
            Kind::Http => "http",
            Kind::Https => "https",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "socks5" => Some(Kind::Socks5),
            "http" => Some(Kind::Http),
            "https" => Some(Kind::Https),
            _ => None,
        }
    }
}

/// The proxy of the VPN service, as the user wrote it down.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Upstream {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

impl Upstream {
    /// Whether enough is known to try at all.
    pub fn is_complete(&self) -> bool {
        !self.host.trim().is_empty() && self.port != 0
    }

    fn address(&self) -> String {
        format!("{}:{}", self.host.trim(), self.port)
    }

    fn has_login(&self) -> bool {
        !self.user.is_empty()
    }
}

const VERSION: u8 = 5;
const NO_AUTHENTICATION: u8 = 0;
const LOGIN: u8 = 2;
const LOGIN_VERSION: u8 = 1;
const CONNECT: u8 = 1;
const SUCCEEDED: u8 = 0;
const ADDRESS_IPV4: u8 = 1;
const ADDRESS_NAME: u8 = 3;
const ADDRESS_IPV6: u8 = 4;

/// Opens a connection to `host`:`port` through the proxy.
pub(crate) async fn open(upstream: &Upstream, host: &str, port: u16) -> io::Result<Carried> {
    let tcp = TcpStream::connect(upstream.address()).await?;
    tcp.set_nodelay(true)?;
    match upstream.kind {
        Kind::Socks5 => {
            let mut stream = tcp;
            through_socks(upstream, &mut stream, host, port).await?;
            Ok(Box::pin(stream))
        }
        Kind::Http => {
            let mut stream = tcp;
            through_connect(upstream, &mut stream, host, port).await?;
            Ok(Box::pin(stream))
        }
        Kind::Https => {
            let mut stream = wrapped_in_tls(upstream, tcp).await?;
            through_connect(upstream, &mut stream, host, port).await?;
            Ok(Box::pin(stream))
        }
    }
}

/// Asks the proxy whether it is there and whether it knows the user, without bothering any site.
pub(crate) async fn reach(upstream: &Upstream) -> io::Result<()> {
    let tcp = TcpStream::connect(upstream.address()).await?;
    match upstream.kind {
        // The greeting already says whether the login is accepted.
        Kind::Socks5 => {
            let mut stream = tcp;
            greet(upstream, &mut stream).await
        }
        Kind::Http => Ok(()),
        Kind::Https => wrapped_in_tls(upstream, tcp).await.map(|_| ()),
    }
}

async fn wrapped_in_tls(
    upstream: &Upstream,
    tcp: TcpStream,
) -> io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(upstream.host.trim().to_owned())
        .map_err(|_| broken("the proxy has no name TLS could be checked against"))?;
    TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
}

/// The SOCKS5 greeting, with the login if there is one.
async fn greet<S: AsyncRead + AsyncWrite + Unpin>(
    upstream: &Upstream,
    stream: &mut S,
) -> io::Result<()> {
    if upstream.has_login() {
        stream
            .write_all(&[VERSION, 2, NO_AUTHENTICATION, LOGIN])
            .await?;
    } else {
        stream.write_all(&[VERSION, 1, NO_AUTHENTICATION]).await?;
    }
    let mut answer = [0u8; 2];
    stream.read_exact(&mut answer).await?;
    if answer[0] != VERSION {
        return Err(broken("the proxy does not speak SOCKS version 5"));
    }
    match answer[1] {
        NO_AUTHENTICATION => Ok(()),
        LOGIN if upstream.has_login() => log_in(upstream, stream).await,
        LOGIN => Err(broken("the proxy wants a user name and a password")),
        0xff => Err(broken("the proxy refused every way of logging in")),
        _ => Err(broken("the proxy wants a way of logging in Atoll cannot")),
    }
}

/// The user name and the password, as RFC 1929 spells them.
async fn log_in<S: AsyncRead + AsyncWrite + Unpin>(
    upstream: &Upstream,
    stream: &mut S,
) -> io::Result<()> {
    let user = upstream.user.as_bytes();
    let password = upstream.password.as_bytes();
    if user.len() > 255 || password.len() > 255 {
        return Err(broken("the user name or the password is too long"));
    }
    let mut message = Vec::with_capacity(3 + user.len() + password.len());
    message.push(LOGIN_VERSION);
    message.push(user.len() as u8);
    message.extend_from_slice(user);
    message.push(password.len() as u8);
    message.extend_from_slice(password);
    stream.write_all(&message).await?;
    let mut answer = [0u8; 2];
    stream.read_exact(&mut answer).await?;
    if answer[1] != 0 {
        return Err(broken("the proxy did not accept the user name or password"));
    }
    Ok(())
}

/// Asks a SOCKS5 proxy for the way to a site.
async fn through_socks<S: AsyncRead + AsyncWrite + Unpin>(
    upstream: &Upstream,
    stream: &mut S,
    host: &str,
    port: u16,
) -> io::Result<()> {
    greet(upstream, stream).await?;
    let name = host.as_bytes();
    if name.len() > 255 {
        return Err(broken("the address of the site is too long"));
    }
    let mut request = Vec::with_capacity(7 + name.len());
    request.extend_from_slice(&[VERSION, CONNECT, 0, ADDRESS_NAME]);
    request.push(name.len() as u8);
    request.extend_from_slice(name);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;

    let mut answer = [0u8; 4];
    stream.read_exact(&mut answer).await?;
    if answer[0] != VERSION {
        return Err(broken("the proxy does not speak SOCKS version 5"));
    }
    if answer[1] != SUCCEEDED {
        return Err(broken(socks_reason(answer[1])));
    }
    match answer[3] {
        ADDRESS_IPV4 => {
            let mut rest = [0u8; 6];
            stream.read_exact(&mut rest).await?;
        }
        ADDRESS_IPV6 => {
            let mut rest = [0u8; 18];
            stream.read_exact(&mut rest).await?;
        }
        ADDRESS_NAME => {
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).await?;
            let mut rest = vec![0u8; length[0] as usize + 2];
            stream.read_exact(&mut rest).await?;
        }
        _ => {
            return Err(broken(
                "the proxy answered with an address of no known kind",
            ));
        }
    }
    Ok(())
}

/// Asks a proxy that takes `CONNECT` for the way to a site.
async fn through_connect<S: AsyncRead + AsyncWrite + Unpin>(
    upstream: &Upstream,
    stream: &mut S,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let mut request = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if upstream.has_login() {
        let login = encode_base64(format!("{}:{}", upstream.user, upstream.password).as_bytes());
        request.push_str(&format!("Proxy-Authorization: Basic {login}\r\n"));
    }
    request.push_str("Proxy-Connection: keep-alive\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;

    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            return Err(broken("the proxy answered with more than an answer"));
        }
        if stream.read_exact(&mut byte).await.is_err() {
            return Err(broken("the proxy closed the connection without an answer"));
        }
        head.push(byte[0]);
    }
    let first = String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let code = first.split_whitespace().nth(1).unwrap_or_default();
    match code {
        "200" => Ok(()),
        "407" => Err(broken("the proxy did not accept the user name or password")),
        _ => Err(broken(&format!("the proxy refused: {first}"))),
    }
}

/// What the proxy said about a connection it did not open.
fn socks_reason(code: u8) -> &'static str {
    match code {
        2 => "the proxy does not allow this connection",
        3 => "the network cannot be reached from the proxy",
        4 => "the site cannot be reached from the proxy",
        5 => "the site refused the connection",
        6 => "the connection timed out",
        7 => "the proxy does not carry this kind of connection",
        8 => "the proxy does not know this kind of address",
        _ => "the proxy could not open the connection",
    }
}

/// Base64 as the `Proxy-Authorization` header wants it.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..group.len()].copy_from_slice(group);
        let number = u32::from(block[0]) << 16 | u32::from(block[1]) << 8 | u32::from(block[2]);
        for index in 0..4 {
            if index <= group.len() {
                let position = (number >> (18 - index * 6)) & 0x3f;
                encoded.push(ALPHABET[position as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

fn broken(message: &str) -> io::Error {
    io::Error::other(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(kind: Kind) -> Upstream {
        Upstream {
            kind,
            host: "proxy.example".to_owned(),
            port: 1080,
            user: "ich".to_owned(),
            password: "geheim".to_owned(),
        }
    }

    /// Plays the proxy: it answers what it was given, and afterwards the test reads from its
    /// end what Atoll said to it.
    async fn proxy(answers: &[&[u8]]) -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
        let (ours, mut theirs) = tokio::io::duplex(8192);
        for answer in answers {
            theirs.write_all(answer).await.unwrap();
        }
        (ours, theirs)
    }

    /// What Atoll said to the proxy.
    async fn said(stream: &mut tokio::io::DuplexStream) -> Vec<u8> {
        let mut heard = vec![0u8; 1024];
        let read = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut heard))
            .await
            .expect("the proxy heard something")
            .unwrap();
        heard.truncate(read);
        heard
    }

    #[tokio::test]
    async fn a_socks_proxy_is_greeted_and_told_the_name_of_the_site() {
        let (mut stream, mut other_end) = proxy(&[
            &[VERSION, LOGIN],
            &[LOGIN_VERSION, 0],
            &[VERSION, SUCCEEDED, 0, ADDRESS_IPV4, 0, 0, 0, 0, 0, 0],
        ])
        .await;
        through_socks(&upstream(Kind::Socks5), &mut stream, "example.org", 443)
            .await
            .expect("the way is open");
        let sent = said(&mut other_end).await;
        assert_eq!(&sent[..4], &[VERSION, 2, NO_AUTHENTICATION, LOGIN]);
        assert!(
            sent.windows(11).any(|window| window == b"example.org"),
            "the name of the site travels on, so the proxy looks it up"
        );
        assert!(
            sent.windows(6).any(|window| window == b"geheim"),
            "the password is handed over where the proxy asks for one"
        );
    }

    #[tokio::test]
    async fn a_socks_proxy_that_refuses_the_login_says_so() {
        let (mut stream, _other_end) = proxy(&[&[VERSION, LOGIN], &[LOGIN_VERSION, 1]]).await;
        let error = through_socks(&upstream(Kind::Socks5), &mut stream, "example.org", 443)
            .await
            .expect_err("the login is refused");
        assert!(error.to_string().contains("user name or password"));
    }

    #[tokio::test]
    async fn a_socks_proxy_that_cannot_reach_the_site_says_which_way_it_failed() {
        let (mut stream, _other_end) = proxy(&[
            &[VERSION, NO_AUTHENTICATION],
            &[VERSION, 4, 0, ADDRESS_IPV4, 0, 0, 0, 0, 0, 0],
        ])
        .await;
        let mut without_login = upstream(Kind::Socks5);
        without_login.user = String::new();
        let error = through_socks(&without_login, &mut stream, "example.org", 443)
            .await
            .expect_err("the site is not reachable");
        assert!(error.to_string().contains("site cannot be reached"));
    }

    #[tokio::test]
    async fn a_connect_proxy_is_asked_with_the_login_and_answers_with_a_code() {
        let (mut stream, mut other_end) =
            proxy(&[b"HTTP/1.1 200 Connection established\r\n\r\n"]).await;
        through_connect(&upstream(Kind::Http), &mut stream, "example.org", 443)
            .await
            .expect("the way is open");
        let asked = String::from_utf8_lossy(&said(&mut other_end).await).to_string();
        assert!(asked.starts_with("CONNECT example.org:443 HTTP/1.1\r\n"));
        assert!(
            asked.contains("Proxy-Authorization: Basic aWNoOmdlaGVpbQ=="),
            "the login travels as the header wants it: {asked}"
        );
    }

    #[tokio::test]
    async fn a_connect_proxy_that_wants_another_login_says_so() {
        let (mut stream, _other_end) =
            proxy(&[b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"]).await;
        let error = through_connect(&upstream(Kind::Http), &mut stream, "example.org", 443)
            .await
            .expect_err("the login is refused");
        assert!(error.to_string().contains("user name or password"));
    }

    #[test]
    fn a_login_is_written_the_way_the_header_wants_it() {
        assert_eq!(encode_base64(b"ich:geheim"), "aWNoOmdlaGVpbQ==");
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"a"), "YQ==");
        assert_eq!(encode_base64(b"ab"), "YWI=");
        assert_eq!(encode_base64(b"abc"), "YWJj");
    }

    #[test]
    fn every_kind_is_known_by_its_name() {
        for kind in [Kind::Socks5, Kind::Http, Kind::Https] {
            assert_eq!(Kind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(Kind::from_key("carrier pigeon"), None);
    }
}
