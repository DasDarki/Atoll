//! Asking a tracker for peers, through the way out Atoll uses.
//!
//! While the torrents go through Atoll's own VPN, the client may not ask the trackers itself: it
//! would look the tracker's name up on this computer and reach it on whatever address came back,
//! past the bridge that only carries TCP over IPv4. Atoll therefore asks for it, with the name
//! resolved at the far end of the tunnel, and hands the peers it got to the client.
//!
//! Only trackers that speak HTTP are asked. A tracker spoken to over UDP cannot be reached
//! through the bridge at all, and asking it outside would be the leak this is here to avoid.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

/// How long one tracker gets to answer.
const TIMEOUT: Duration = Duration::from_secs(20);

/// How many peers are asked for.
const WANTED: usize = 100;

/// Asks every tracker that speaks HTTP and collects what they answer. Blocking, so it belongs on
/// a thread of its own.
pub(crate) fn peers_of(
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    trackers: &[String],
) -> Vec<SocketAddr> {
    let mut found = Vec::new();
    for tracker in trackers.iter().filter(|url| speaks_http(url)) {
        match ask(tracker, info_hash, peer_id) {
            Ok(peers) => {
                tracing::debug!(tracker, peers = peers.len(), "a tracker answered");
                found.extend(peers);
            }
            Err(trouble) => tracing::debug!(tracker, trouble, "a tracker did not answer"),
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Whether Atoll can reach this tracker at all on the way it is using.
pub(crate) fn speaks_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// One question to one tracker.
fn ask(tracker: &str, info_hash: &[u8; 20], peer_id: &[u8; 20]) -> Result<Vec<SocketAddr>, String> {
    let separator = if tracker.contains('?') { '&' } else { '?' };
    let address = format!(
        "{tracker}{separator}info_hash={}&peer_id={}&port=6881&uploaded=0&downloaded=0\
         &left=1&compact=1&numwant={WANTED}&event=started",
        escaped(info_hash),
        escaped(peer_id),
    );
    let mut answer = atoll_net::agent(TIMEOUT)
        .get(&address)
        .call()
        .map_err(|error| error.to_string())?;
    let body = answer
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_vec()
        .map_err(|error| error.to_string())?;
    Ok(peers_in(&body))
}

/// Every byte that is not plain is written out, as a tracker expects its parameters.
fn escaped(bytes: &[u8; 20]) -> String {
    let mut written = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                written.push(*byte as char);
            }
            other => written.push_str(&format!("%{other:02X}")),
        }
    }
    written
}

/// The peers in a tracker's answer, which is bencoded. Only the compact form is read, which is
/// what every tracker has answered with for twenty years.
fn peers_in(answer: &[u8]) -> Vec<SocketAddr> {
    let Some(peers) = value_of(answer, b"peers") else {
        return Vec::new();
    };
    peers
        .chunks_exact(6)
        .map(|peer| {
            let address = Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]);
            let port = u16::from_be_bytes([peer[4], peer[5]]);
            SocketAddr::V4(SocketAddrV4::new(address, port))
        })
        .filter(|peer| peer.port() != 0 && !peer.ip().is_unspecified())
        .collect()
}

/// The bytes a key holds in a bencoded dictionary, without reading the whole thing: the answer of
/// a tracker is flat, and everything Atoll wants from it is one string.
fn value_of<'a>(bencoded: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let needle = format!("{}:", key.len())
        .into_bytes()
        .into_iter()
        .chain(key.iter().copied())
        .collect::<Vec<u8>>();
    let at = bencoded
        .windows(needle.len())
        .position(|window| window == needle)?;
    let rest = &bencoded[at + needle.len()..];
    let colon = rest.iter().position(|byte| *byte == b':')?;
    let length: usize = std::str::from_utf8(&rest[..colon]).ok()?.parse().ok()?;
    rest.get(colon + 1..colon + 1 + length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_trackers_that_speak_http_are_asked() {
        assert!(speaks_http("https://torrent.example/announce"));
        assert!(speaks_http("http://torrent.example/announce"));
        assert!(
            !speaks_http("udp://tracker.example:1337/announce"),
            "a tracker spoken to over UDP cannot be reached through the bridge"
        );
    }

    #[test]
    fn what_a_tracker_answers_is_read_as_peers() {
        let mut answer = b"d8:intervali1800e5:peers12:".to_vec();
        answer.extend_from_slice(&[93, 158, 213, 92, 0x1a, 0xe1]);
        answer.extend_from_slice(&[10, 0, 0, 7, 0x1a, 0xe2]);
        answer.extend_from_slice(b"e");
        let peers = peers_in(&answer);
        assert_eq!(
            peers,
            vec![
                "93.158.213.92:6881".parse::<SocketAddr>().unwrap(),
                "10.0.0.7:6882".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn an_answer_without_peers_is_no_trouble() {
        assert!(peers_in(b"d14:failure reason9:not founde").is_empty());
        assert!(peers_in(b"").is_empty());
        assert!(peers_in(b"d5:peers0:e").is_empty());
    }

    #[test]
    fn what_a_tracker_is_told_is_written_the_way_it_reads_it() {
        let hash = [0u8; 20];
        assert_eq!(escaped(&hash), "%00".repeat(20));
        let mixed = *b"abcDEF0123456789-_.~";
        assert_eq!(escaped(&mixed), "abcDEF0123456789-_.~");
    }
}
