//! The network card that does not exist.
//!
//! smoltcp needs something to hand its packets to. Here that something is two queues: what came
//! out of the tunnel goes in, what the stack wants to send comes out and is encrypted before it
//! leaves the computer.

use std::collections::VecDeque;

use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

/// The two queues between the protocol stack and the tunnel.
pub(crate) struct Device {
    /// Packets that came out of the tunnel, waiting for the stack.
    pub(crate) inbox: VecDeque<Vec<u8>>,
    /// Packets the stack wants to send, waiting to be encrypted.
    pub(crate) outbox: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl Device {
    pub(crate) fn new(mtu: usize) -> Self {
        Self {
            inbox: VecDeque::new(),
            outbox: VecDeque::new(),
            mtu,
        }
    }
}

pub(crate) struct RxToken {
    packet: Vec<u8>,
}

pub(crate) struct TxToken<'a> {
    outbox: &'a mut VecDeque<Vec<u8>>,
}

impl phy::Device for Device {
    type RxToken<'a>
        = RxToken
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _now: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.inbox.pop_front()?;
        Some((
            RxToken { packet },
            TxToken {
                outbox: &mut self.outbox,
            },
        ))
    }

    fn transmit(&mut self, _now: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken {
            outbox: &mut self.outbox,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ip;
        capabilities.max_transmission_unit = self.mtu;
        capabilities
    }
}

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, read: F) -> R {
        read(&self.packet)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, write: F) -> R {
        let mut packet = vec![0u8; length];
        let result = write(&mut packet);
        self.outbox.push_back(packet);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::phy::{Device as _, RxToken as _, TxToken as _};

    #[test]
    fn what_goes_in_comes_out_at_the_other_side() {
        let mut device = Device::new(1420);
        assert!(
            device.receive(Instant::ZERO).is_none(),
            "nothing arrived yet"
        );
        device.inbox.push_back(b"ein paket".to_vec());

        let (received, _) = device.receive(Instant::ZERO).expect("a packet is there");
        assert_eq!(received.consume(<[u8]>::to_vec), b"ein paket".to_vec());

        let token = device
            .transmit(Instant::ZERO)
            .expect("sending is always allowed");
        token.consume(3, |packet| packet.copy_from_slice(b"raus"[..3].as_ref()));
        assert_eq!(device.outbox.pop_front(), Some(b"rau".to_vec()));
    }

    #[test]
    fn the_size_of_a_packet_is_the_one_the_tunnel_allows() {
        let device = Device::new(1380);
        assert_eq!(device.capabilities().max_transmission_unit, 1380);
        assert_eq!(device.capabilities().medium, Medium::Ip);
    }
}
