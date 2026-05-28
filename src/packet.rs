#![allow(dead_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketKind {
    Tcp,
    Udp,
    Dns,
    Unsupported(u8),
    Malformed,
}

pub fn classify_ipv4(packet: &[u8]) -> PacketKind {
    if packet.len() < 20 {
        return PacketKind::Malformed;
    }

    let version = packet[0] >> 4;
    if version != 4 {
        return PacketKind::Malformed;
    }

    let ihl = usize::from(packet[0] & 0x0f) * 4;
    if ihl < 20 || packet.len() < ihl {
        return PacketKind::Malformed;
    }

    match packet[9] {
        6 => PacketKind::Tcp,
        17 => classify_udp(packet, ihl),
        protocol => PacketKind::Unsupported(protocol),
    }
}

fn classify_udp(packet: &[u8], ihl: usize) -> PacketKind {
    if packet.len() < ihl + 8 {
        return PacketKind::Malformed;
    }

    let src_port = u16::from_be_bytes([packet[ihl], packet[ihl + 1]]);
    let dst_port = u16::from_be_bytes([packet[ihl + 2], packet[ihl + 3]]);
    if src_port == 53 || dst_port == 53 {
        PacketKind::Dns
    } else {
        PacketKind::Udp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4_packet(protocol: u8, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0u8; 20 + payload.len()];
        let packet_len = packet.len() as u16;
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&packet_len.to_be_bytes());
        packet[8] = 64;
        packet[9] = protocol;
        packet[12..16].copy_from_slice(&[10, 0, 0, 1]);
        packet[16..20].copy_from_slice(&[10, 0, 0, 2]);
        packet[20..].copy_from_slice(payload);
        packet
    }

    #[test]
    fn classifies_tcp() {
        assert_eq!(classify_ipv4(&ipv4_packet(6, &[0; 20])), PacketKind::Tcp);
    }

    #[test]
    fn classifies_udp_dns_by_port() {
        let mut udp = [0u8; 8];
        udp[2..4].copy_from_slice(&53u16.to_be_bytes());
        assert_eq!(classify_ipv4(&ipv4_packet(17, &udp)), PacketKind::Dns);
    }

    #[test]
    fn classifies_udp_non_dns() {
        let mut udp = [0u8; 8];
        udp[0..2].copy_from_slice(&443u16.to_be_bytes());
        udp[2..4].copy_from_slice(&443u16.to_be_bytes());
        assert_eq!(classify_ipv4(&ipv4_packet(17, &udp)), PacketKind::Udp);
    }

    #[test]
    fn classifies_unsupported_protocol() {
        assert_eq!(
            classify_ipv4(&ipv4_packet(1, &[0; 8])),
            PacketKind::Unsupported(1)
        );
    }

    #[test]
    fn classifies_malformed_packet() {
        assert_eq!(classify_ipv4(&[0x45, 0]), PacketKind::Malformed);
    }
}
