//! Reading the configuration a VPN service hands out for WireGuard.
//!
//! The file is what `wg-quick` reads, and services all write the same handful of lines. Atoll
//! takes what it needs to build the tunnel and passes over the rest: `AllowedIPs` for instance
//! decides nothing here, because everything of the browser goes through the tunnel anyway.

use std::net::Ipv4Addr;

use atoll_i18n::t;

/// What the configuration of a tunnel says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The secret of this computer, 32 bytes.
    pub private_key: [u8; 32],
    /// The key of the server.
    pub peer_key: [u8; 32],
    /// The additional key some services use.
    pub preshared_key: Option<[u8; 32]>,
    /// The address this computer has inside the tunnel.
    pub address: Ipv4Addr,
    /// Where names are looked up, inside the tunnel. Without it a lookup would give the user away.
    pub dns: Ipv4Addr,
    /// The server, as it is written down: a name or an address, with a port.
    pub endpoint: String,
    /// How large a packet may be inside the tunnel.
    pub mtu: usize,
    /// How often an empty packet keeps the way open through a router at home.
    pub keepalive: Option<u16>,
}

/// What a configuration without this is missing.
const NEEDED: &str = "PrivateKey, Address, DNS, PublicKey and Endpoint";

impl Config {
    /// Reads a configuration in the form `wg-quick` uses.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut section = String::new();
        let mut interface: Vec<(String, String)> = Vec::new();
        let mut peer: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_ascii_lowercase();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let entry = (key.trim().to_ascii_lowercase(), value.trim().to_owned());
            match section.as_str() {
                "interface" => interface.push(entry),
                "peer" => peer.push(entry),
                _ => {}
            }
        }
        let find = |entries: &[(String, String)], key: &str| {
            entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };

        let private_key = key_of(&find(&interface, "privatekey").ok_or(missing("PrivateKey"))?)?;
        let peer_key = key_of(&find(&peer, "publickey").ok_or(missing("PublicKey"))?)?;
        let preshared_key = match find(&peer, "presharedkey") {
            Some(text) => Some(key_of(&text)?),
            None => None,
        };
        let address = first_address(&find(&interface, "address").ok_or(missing("Address"))?)
            .ok_or_else(|| t!("Die Adresse im Tunnel ist keine IPv4-Adresse").to_owned())?;
        let dns = find(&interface, "dns")
            .as_deref()
            .and_then(first_address)
            .ok_or_else(|| {
                t!(
                    "Ohne DNS im Tunnel könnte Atoll Namen nur außerhalb nachschlagen, und genau das würde dich verraten. Trag einen DNS-Server in die Konfiguration ein."
                )
                .to_owned()
            })?;
        let endpoint = find(&peer, "endpoint").ok_or(missing("Endpoint"))?;
        if !endpoint.contains(':') {
            return Err(
                t!("Der Endpoint braucht einen Port, etwa server.beispiel:51820").to_owned(),
            );
        }
        let mtu = find(&interface, "mtu")
            .and_then(|mtu| mtu.parse().ok())
            .filter(|mtu| (576..=1500).contains(mtu))
            .unwrap_or(1420);
        let keepalive = find(&peer, "persistentkeepalive")
            .and_then(|seconds| seconds.parse().ok())
            .filter(|seconds| *seconds > 0);

        Ok(Config {
            private_key,
            peer_key,
            preshared_key,
            address,
            dns,
            endpoint,
            mtu,
            keepalive,
        })
    }
}

fn missing(what: &str) -> String {
    t!(
        "Der Konfiguration fehlt {}. Gebraucht werden {}.",
        what,
        NEEDED
    )
}

/// The first IPv4 address of a line that may hold several, with or without a prefix length.
fn first_address(line: &str) -> Option<Ipv4Addr> {
    line.split(',')
        .map(str::trim)
        .filter_map(|entry| entry.split('/').next())
        .find_map(|entry| entry.parse().ok())
}

/// A key as WireGuard writes it: 32 bytes in base64.
fn key_of(text: &str) -> Result<[u8; 32], String> {
    let bytes = decode_base64(text.trim())
        .ok_or_else(|| t!("Ein Schlüssel in der Konfiguration ist kein Base64").to_owned())?;
    bytes
        .try_into()
        .map_err(|_| t!("Ein Schlüssel in der Konfiguration ist nicht 32 Byte lang").to_owned())
}

/// Base64 as the configuration writes it, without the characters of the other alphabet.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut held = 0;
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);
    for character in text.chars() {
        if character == '=' {
            break;
        }
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | value;
        held += 6;
        if held >= 8 {
            held -= 8;
            bytes.push((bits >> held) as u8);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
[Interface]
PrivateKey = YHRjU9MVpzFCdBHJkDHFKwJSSAU2noqgOLUf5gyFCGI=
Address = 10.64.0.2/32, fc00:bbbb::1/128
DNS = 10.64.0.1
MTU = 1380

[Peer]
PublicKey = /Y4sHXmM9hMtXK4tKPJBSHEcdHpYdYRbPCxBLOlqIxs=
PresharedKey = tGlmcm5kZ2hqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = de-ber-wg-001.example:51820
PersistentKeepalive = 25
";

    #[test]
    fn a_configuration_of_a_service_is_read() {
        let config = Config::parse(CONFIG).expect("the configuration is readable");
        assert_eq!(config.address, Ipv4Addr::new(10, 64, 0, 2));
        assert_eq!(config.dns, Ipv4Addr::new(10, 64, 0, 1));
        assert_eq!(config.endpoint, "de-ber-wg-001.example:51820");
        assert_eq!(config.mtu, 1380);
        assert_eq!(config.keepalive, Some(25));
        assert_eq!(config.private_key.len(), 32);
        assert!(config.preshared_key.is_some());
    }

    #[test]
    fn what_is_not_written_down_falls_back_or_is_said_plainly() {
        let plain = CONFIG
            .replace("MTU = 1380\n", "")
            .replace("PersistentKeepalive = 25\n", "")
            .replace(
                "PresharedKey = tGlmcm5kZ2hqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY=\n",
                "",
            );
        let config = Config::parse(&plain).expect("the configuration is readable");
        assert_eq!(config.mtu, 1420, "the usual size inside a tunnel");
        assert_eq!(config.keepalive, None);
        assert_eq!(config.preshared_key, None);

        let without_dns = CONFIG.replace("DNS = 10.64.0.1\n", "");
        let complaint = Config::parse(&without_dns).expect_err("without DNS it cannot work");
        assert!(complaint.contains("DNS"), "{complaint}");

        let without_key = CONFIG.replace("PrivateKey", "NichtDerSchlüssel");
        assert!(
            Config::parse(&without_key)
                .expect_err("without a key it cannot work")
                .contains("PrivateKey")
        );
    }

    #[test]
    fn comments_and_the_order_of_the_lines_do_not_matter() {
        let shuffled = "\
# Was der Dienst geschickt hat
[Peer]
Endpoint = 198.51.100.7:51820   # der Server
PublicKey = /Y4sHXmM9hMtXK4tKPJBSHEcdHpYdYRbPCxBLOlqIxs=

[Interface]
DNS=10.64.0.1
Address=10.64.0.2/32
PrivateKey=YHRjU9MVpzFCdBHJkDHFKwJSSAU2noqgOLUf5gyFCGI=
";
        let config = Config::parse(shuffled).expect("the configuration is readable");
        assert_eq!(config.endpoint, "198.51.100.7:51820");
        assert_eq!(config.address, Ipv4Addr::new(10, 64, 0, 2));
    }

    #[test]
    fn a_key_that_is_not_a_key_is_refused() {
        let broken = CONFIG.replace(
            "YHRjU9MVpzFCdBHJkDHFKwJSSAU2noqgOLUf5gyFCGI=",
            "kein schlüssel",
        );
        assert!(Config::parse(&broken).is_err());
        let short = CONFIG.replace("YHRjU9MVpzFCdBHJkDHFKwJSSAU2noqgOLUf5gyFCGI=", "YWJj");
        assert!(
            Config::parse(&short)
                .expect_err("a key of three bytes is no key")
                .contains("32")
        );
    }

    #[test]
    fn base64_is_read_the_way_the_keys_are_written() {
        assert_eq!(decode_base64("YWJj"), Some(b"abc".to_vec()));
        assert_eq!(decode_base64("YWI="), Some(b"ab".to_vec()));
        assert_eq!(decode_base64("YQ=="), Some(b"a".to_vec()));
        assert_eq!(decode_base64("!!"), None);
    }
}
