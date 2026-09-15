//! The SOCKS server the browser engine talks to.
//!
//! Only what a browser needs: version 5, no authentication, and CONNECT. The address is passed
//! on as it arrives, so names are looked up inside the Tor network and never by this computer.

use std::sync::Arc;

use arti_client::TorClient;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tor_rtcompat::Runtime;

/// The answers of the handshake, as the protocol spells them.
const VERSION: u8 = 5;
const NO_AUTHENTICATION: u8 = 0;
const CONNECT: u8 = 1;
const SUCCEEDED: u8 = 0;
const GENERAL_FAILURE: u8 = 1;
const ADDRESS_IPV4: u8 = 1;
const ADDRESS_NAME: u8 = 3;
const ADDRESS_IPV6: u8 = 4;

/// Serves every connection until the listener gives up.
pub(crate) async fn serve<R: Runtime>(listener: TcpListener, client: Arc<TorClient<R>>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let client = client.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(stream, client).await {
                tracing::debug!(%error, "a Tor connection ended early");
            }
        });
    }
}

/// One connection: greet, read where it wants to go, open that through Tor and then only carry
/// bytes in both directions.
async fn handle<R: Runtime>(
    mut stream: TcpStream,
    client: Arc<TorClient<R>>,
) -> Result<(), std::io::Error> {
    greet(&mut stream).await?;
    let (host, port) = destination(&mut stream).await?;
    let opened = client.connect((host.as_str(), port)).await;
    let tor = match opened {
        Ok(tor) => tor,
        Err(error) => {
            tracing::debug!(%error, host, port, "Tor could not reach the site");
            refuse(&mut stream).await?;
            return Ok(());
        }
    };
    accept(&mut stream).await?;
    let (mut from_browser, mut to_browser) = stream.into_split();
    let (mut from_site, mut to_site) = tokio::io::split(tor);
    let up = async {
        tokio::io::copy(&mut from_browser, &mut to_site).await?;
        to_site.shutdown().await
    };
    let down = async {
        tokio::io::copy(&mut from_site, &mut to_browser).await?;
        to_browser.shutdown().await
    };
    let _ = tokio::join!(up, down);
    Ok(())
}

/// The opening handshake, which only offers going without authentication.
async fn greet<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) -> Result<(), std::io::Error> {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != VERSION {
        return Err(broken("only SOCKS version 5 is spoken here"));
    }
    let mut methods = vec![0u8; header[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&NO_AUTHENTICATION) {
        stream.write_all(&[VERSION, 0xff]).await?;
        return Err(broken("the browser asked for an authentication"));
    }
    stream.write_all(&[VERSION, NO_AUTHENTICATION]).await?;
    Ok(())
}

/// Where the browser wants to go.
async fn destination<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
) -> Result<(String, u16), std::io::Error> {
    let mut request = [0u8; 4];
    stream.read_exact(&mut request).await?;
    if request[0] != VERSION {
        return Err(broken("only SOCKS version 5 is spoken here"));
    }
    if request[1] != CONNECT {
        refuse(stream).await?;
        return Err(broken("only connections are carried, nothing else"));
    }
    let host = match request[3] {
        ADDRESS_IPV4 => {
            let mut address = [0u8; 4];
            stream.read_exact(&mut address).await?;
            std::net::Ipv4Addr::from(address).to_string()
        }
        ADDRESS_IPV6 => {
            let mut address = [0u8; 16];
            stream.read_exact(&mut address).await?;
            std::net::Ipv6Addr::from(address).to_string()
        }
        ADDRESS_NAME => {
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).await?;
            let mut name = vec![0u8; length[0] as usize];
            stream.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| broken("the name is not readable"))?
        }
        _ => {
            refuse(stream).await?;
            return Err(broken("this kind of address is not known"));
        }
    };
    let mut port = [0u8; 2];
    stream.read_exact(&mut port).await?;
    Ok((host, u16::from_be_bytes(port)))
}

/// Tells the browser the way is open. The address of this end is of no interest to it, so it
/// gets zeros, which the protocol allows.
async fn accept<S: AsyncWrite + Unpin>(stream: &mut S) -> Result<(), std::io::Error> {
    stream
        .write_all(&[VERSION, SUCCEEDED, 0, ADDRESS_IPV4, 0, 0, 0, 0, 0, 0])
        .await
}

async fn refuse<S: AsyncWrite + Unpin>(stream: &mut S) -> Result<(), std::io::Error> {
    stream
        .write_all(&[VERSION, GENERAL_FAILURE, 0, ADDRESS_IPV4, 0, 0, 0, 0, 0, 0])
        .await
}

fn broken(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays the browser side of the handshake and answers with what the server said.
    async fn talk(request: &[u8]) -> (Vec<u8>, Option<(String, u16)>) {
        let (mut browser, mut server) = tokio::io::duplex(1024);
        let spoken = tokio::spawn(async move {
            greet(&mut server).await?;
            let destination = destination(&mut server).await?;
            accept(&mut server).await?;
            Ok::<_, std::io::Error>(destination)
        });
        browser.write_all(request).await.unwrap();
        let mut answer = Vec::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            browser.read_to_end(&mut answer),
        )
        .await;
        let _ = read;
        (answer, spoken.await.unwrap().ok())
    }

    #[tokio::test]
    async fn a_browser_asking_for_a_name_is_understood() {
        let mut request = vec![VERSION, 1, NO_AUTHENTICATION];
        request.extend([VERSION, CONNECT, 0, ADDRESS_NAME, 11]);
        request.extend(b"example.org");
        request.extend(443u16.to_be_bytes());
        let (answer, destination) = talk(&request).await;
        assert_eq!(
            destination,
            Some(("example.org".to_owned(), 443)),
            "the name travels on untouched, so Tor looks it up and not this computer"
        );
        assert_eq!(answer[0..2], [VERSION, NO_AUTHENTICATION]);
        assert_eq!(answer[2..4], [VERSION, SUCCEEDED]);
    }

    #[tokio::test]
    async fn an_address_of_numbers_works_too() {
        let mut request = vec![VERSION, 1, NO_AUTHENTICATION];
        request.extend([VERSION, CONNECT, 0, ADDRESS_IPV4, 10, 0, 0, 7]);
        request.extend(80u16.to_be_bytes());
        let (_, destination) = talk(&request).await;
        assert_eq!(destination, Some(("10.0.0.7".to_owned(), 80)));
    }

    #[tokio::test]
    async fn anything_but_connecting_is_refused() {
        let mut request = vec![VERSION, 1, NO_AUTHENTICATION];
        request.extend([VERSION, 2, 0, ADDRESS_IPV4, 10, 0, 0, 7]);
        request.extend(80u16.to_be_bytes());
        let (answer, destination) = talk(&request).await;
        assert_eq!(destination, None);
        assert_eq!(answer[2..4], [VERSION, GENERAL_FAILURE]);
    }
}
