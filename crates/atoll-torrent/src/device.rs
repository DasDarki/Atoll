//! The network devices of the system, so a VPN set up there can carry the torrents.
//!
//! Atoll does not build that VPN and cannot check what it does. What it can do is read which
//! devices exist, which of them look like a tunnel, and whether one is still up - and tie every
//! socket of a torrent to it, after which the kernel lets nothing past it.

use std::fs;
use std::path::Path;

/// Where the system writes down its network devices.
const DEVICES: &str = "/sys/class/net";

/// The names that usually belong to a tunnel.
const TUNNELS: [&str; 6] = ["wg", "tun", "proton", "nordlynx", "mullvad", "tailscale"];

/// A network device, as the settings offer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    /// Whether it looks like the device of a VPN rather than a cable or a radio.
    pub looks_like_a_tunnel: bool,
    pub up: bool,
}

/// Every device of the system, the ones that look like a tunnel first.
pub fn devices() -> Vec<Device> {
    let Ok(entries) = fs::read_dir(DEVICES) else {
        return Vec::new();
    };
    let mut found: Vec<Device> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != "lo")
        .map(|name| Device {
            looks_like_a_tunnel: looks_like_a_tunnel(&name),
            up: is_up(&name),
            name,
        })
        .collect();
    found.sort_by(|one, other| {
        other
            .looks_like_a_tunnel
            .cmp(&one.looks_like_a_tunnel)
            .then_with(|| one.name.cmp(&other.name))
    });
    found
}

/// Whether the device is there and carrying.
pub fn is_up(name: &str) -> bool {
    if name.is_empty() || name.contains('/') {
        return false;
    }
    let state = fs::read_to_string(Path::new(DEVICES).join(name).join("operstate"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    // A tunnel usually says "unknown" instead of "up", because it has no cable to speak of.
    matches!(state.as_str(), "up" | "unknown") && carries(name)
}

/// Whether the device is switched on, read out of the flags the system keeps.
fn carries(name: &str) -> bool {
    let flags = fs::read_to_string(Path::new(DEVICES).join(name).join("flags")).unwrap_or_default();
    let flags = flags.trim().trim_start_matches("0x");
    u32::from_str_radix(flags, 16).is_ok_and(|flags| flags & 0x1 != 0)
}

fn looks_like_a_tunnel(name: &str) -> bool {
    TUNNELS.iter().any(|known| name.starts_with(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_of_tunnels_are_known() {
        assert!(looks_like_a_tunnel("wg0"));
        assert!(looks_like_a_tunnel("tun0"));
        assert!(looks_like_a_tunnel("proton0"));
        assert!(!looks_like_a_tunnel("enp5s0"));
        assert!(!looks_like_a_tunnel("wlan0"));
    }

    #[test]
    fn a_device_that_is_not_there_is_not_up() {
        assert!(!is_up("gibtesnicht0"));
        assert!(!is_up(""), "an empty name is no device");
        assert!(!is_up("../etc"), "and no way out of the folder either");
    }

    #[test]
    fn the_devices_of_this_computer_are_read() {
        let found = devices();
        assert!(!found.is_empty(), "every computer has a network device");
        assert!(
            found.iter().all(|device| device.name != "lo"),
            "the one that only talks to itself is left out"
        );
        let tunnels_first = found
            .iter()
            .position(|device| !device.looks_like_a_tunnel)
            .is_none_or(|first_plain| {
                found
                    .iter()
                    .skip(first_plain)
                    .all(|device| !device.looks_like_a_tunnel)
            });
        assert!(tunnels_first, "what looks like a tunnel is offered first");
    }
}
