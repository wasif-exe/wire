use wire_core::types::PackedTuple;
use crate::ParsedL4;

#[inline(always)]
pub fn parse_one_scalar(frame: &[u8]) -> ParsedL4 {
    if frame.len() < 54 {
        return ParsedL4::default();
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != 0x0800 {
        return ParsedL4::default();
    }
    let version_ihl = frame[14];
    let version = version_ihl >> 4;
    let ihl = version_ihl & 0x0F;
    if version != 4 || ihl < 5 {
        return ParsedL4::default();
    }
    let ihl_bytes = (ihl * 4) as usize;
    let total_len = u16::from_be_bytes([frame[16], frame[17]]) as usize;
    let proto = frame[23];
    if proto != 6 && proto != 17 {
        return ParsedL4::default();
    }
    if frame.len() < 14 + total_len {
        return ParsedL4::default();
    }
    let ip_hdr = &frame[14..14 + ihl_bytes];
    let mut sum: u32 = 0;
    for chunk in ip_hdr.chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    if !sum as u16 != 0 {
        return ParsedL4::default();
    }
    let src_ip: [u8; 4] = frame[26..30].try_into().unwrap();
    let dst_ip: [u8; 4] = frame[30..34].try_into().unwrap();
    let l4_offset = 14 + ihl_bytes;
    if frame.len() < l4_offset + 8 {
        return ParsedL4::default();
    }
    let src_port = u16::from_be_bytes([frame[l4_offset], frame[l4_offset + 1]]);
    let dst_port = u16::from_be_bytes([frame[l4_offset + 2], frame[l4_offset + 3]]);
    if proto == 6 {
        if frame.len() < l4_offset + 20 {
            return ParsedL4::default();
        }
        let tcp_seq = u32::from_be_bytes([
            frame[l4_offset + 4],
            frame[l4_offset + 5],
            frame[l4_offset + 6],
            frame[l4_offset + 7],
        ]);
        let tcp_ack = u32::from_be_bytes([
            frame[l4_offset + 8],
            frame[l4_offset + 9],
            frame[l4_offset + 10],
            frame[l4_offset + 11],
        ]);
        let data_offset = frame[l4_offset + 12] >> 4;
        let tcp_flags = frame[l4_offset + 13];
        let tcp_window = u16::from_be_bytes([frame[l4_offset + 14], frame[l4_offset + 15]]);
        let tcp_hdr_len = (data_offset * 4) as usize;
        if total_len < ihl_bytes + tcp_hdr_len {
            return ParsedL4::default();
        }
        let payload_offset = l4_offset + tcp_hdr_len;
        let payload_len = total_len - ihl_bytes - tcp_hdr_len;
        ParsedL4 {
            tuple: PackedTuple {
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                proto,
                _pad: [0; 3],
            },
            payload_offset: payload_offset as u16,
            payload_len: payload_len as u16,
            tcp_seq,
            tcp_ack,
            tcp_flags,
            tcp_window,
            valid: true,
        }
    } else {
        let udp_len = u16::from_be_bytes([frame[l4_offset + 4], frame[l4_offset + 5]]) as usize;
        if udp_len < 8 || total_len < ihl_bytes + udp_len {
            return ParsedL4::default();
        }
        let payload_offset = l4_offset + 8;
        let payload_len = udp_len - 8;
        ParsedL4 {
            tuple: PackedTuple {
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                proto,
                _pad: [0; 3],
            },
            payload_offset: payload_offset as u16,
            payload_len: payload_len as u16,
            tcp_seq: 0,
            tcp_ack: 0,
            tcp_flags: 0,
            tcp_window: 0,
            valid: true,
        }
    }
}
