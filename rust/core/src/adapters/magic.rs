//! The direct magic packet: UDP from this computer to a configured IPv4 address and port.
//!
//! The address is whatever the configuration says: a broadcast address on the same network,
//! or the PC's own IP for a routed setup (see the architecture document). `SO_BROADCAST` is
//! set only when the system refuses to send without it, so unicast packets never carry it.

use std::io;
use std::net::{Ipv4Addr, UdpSocket};

use crate::domain::{MacAddr, WakeMethod};
use crate::ports::{WakeError, WakeRequest, WakeSender};

/// Length of a magic packet: 6 bytes of `FF` and the MAC address 16 times.
pub const MAGIC_PACKET_LEN: usize = 6 + 16 * 6;

/// The magic packet for a MAC address.
pub fn magic_packet(mac: &MacAddr) -> [u8; MAGIC_PACKET_LEN] {
    let mut packet = [0xFF; MAGIC_PACKET_LEN];
    let octets = mac.octets();
    for (index, byte) in packet.iter_mut().enumerate().skip(6) {
        *byte = octets.get((index - 6) % 6).copied().unwrap_or(0);
    }
    packet
}

/// Sends the magic packet for `mac` to `address:port` over UDP.
///
/// If the system refuses the send with a permission error, which is what sending to a
/// broadcast address without `SO_BROADCAST` gives, the socket is switched to broadcast and
/// the send is repeated once.
pub fn send_magic_packet(address: Ipv4Addr, port: u16, mac: &MacAddr) -> io::Result<()> {
    let packet = magic_packet(mac);
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let sent = match socket.send_to(&packet, (address, port)) {
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            socket.set_broadcast(true)?;
            socket.send_to(&packet, (address, port))?
        }
        other => other?,
    };
    if sent == packet.len() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            format!("sent {sent} of {} bytes", packet.len()),
        ))
    }
}

/// Wakes a PC with [`WakeMethod::MagicPacket`].
#[derive(Debug, Default, Clone, Copy)]
pub struct MagicPacketWake;

impl WakeSender for MagicPacketWake {
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError> {
        let WakeMethod::MagicPacket { mac, address, port } = request.method else {
            return Err(WakeError::Unsupported);
        };
        log::info!(
            "sending a magic packet for {} to {address}:{port}",
            request.pc.id
        );
        send_magic_packet(*address, *port, mac).map_err(|err| {
            WakeError::Failed(format!(
                "cannot send the magic packet to {address}:{port}: {err}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A known vector: the packet for AA:BB:CC:DD:EE:FF is six FF bytes followed by the MAC
    /// sixteen times.
    #[test]
    fn the_packet_matches_a_known_vector() {
        let mac = MacAddr::parse("aa:bb:cc:dd:ee:ff").unwrap();
        let packet = magic_packet(&mac);
        assert_eq!(packet.len(), 102);
        let expected = format!("{}{}", "ff".repeat(6), "aabbccddeeff".repeat(16));
        assert_eq!(hex(&packet), expected);
    }

    #[test]
    fn the_packet_follows_the_octet_order_of_the_address() {
        let mac = MacAddr::parse("01-23-45-67-89-ab").unwrap();
        let packet = magic_packet(&mac);
        assert_eq!(hex(packet.get(..12).unwrap()), "ffffffffffff0123456789ab");
        assert_eq!(hex(packet.get(96..).unwrap()), "0123456789ab");
    }
}
