use wire_core::types::PackedTuple;
use crate::ParsedL4;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f", enable = "avx512bw", enable = "avx512dq", enable = "avx512vl")]
pub unsafe fn parse_one_avx512(frame: &[u8]) -> ParsedL4 {
    let len = frame.len();
    if len < 64 {
        return crate::scalar::parse_one_scalar(frame);
    }
    let ptr = frame.as_ptr();

    let chunk = _mm512_loadu_si512(ptr as *const __m512i);

    let mut buf = [0u8; 64];
    _mm512_storeu_si512(buf.as_mut_ptr() as *mut __m512i, chunk);

    let ethertype = u16::from_be_bytes([buf[12], buf[13]]);
    if ethertype != 0x0800 && ethertype != 0x86dd {
        return ParsedL4::default();
    }

    if ethertype == 0x0800 {
        let ver_ihl = buf[14];
        if (ver_ihl >> 4) != 4 || (ver_ihl & 0x0f) != 5 {
            return ParsedL4::default();
        }

        let proto = buf[23];
        if proto != 6 && proto != 17 {
            return ParsedL4::default();
        }

        let src_ip = [buf[26], buf[27], buf[28], buf[29]];
        let dst_ip = [buf[30], buf[31], buf[32], buf[33]];

        let src_port = u16::from_be_bytes([buf[34], buf[35]]);
        let dst_port = u16::from_be_bytes([buf[36], buf[37]]);

        let total_len = u16::from_be_bytes([buf[16], buf[17]]) as usize;
        if len < 14 + total_len {
            return ParsedL4::default();
        }

        if proto == 6 {
            let tcp_seq = u32::from_be_bytes([buf[38], buf[39], buf[40], buf[41]]);
            let tcp_ack = u32::from_be_bytes([buf[42], buf[43], buf[44], buf[45]]);
            let data_offset = buf[46] >> 4;
            let tcp_flags = buf[47];
            let tcp_window = u16::from_be_bytes([buf[48], buf[49]]);

            let tcp_hdr_len = (data_offset * 4) as usize;
            let payload_offset = 34 + tcp_hdr_len;
            if total_len < 20 + tcp_hdr_len {
                return ParsedL4::default();
            }
            let payload_len = total_len - 20 - tcp_hdr_len;

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
            let udp_len = u16::from_be_bytes([buf[38], buf[39]]) as usize;
            if udp_len < 8 || total_len < 20 + udp_len {
                return ParsedL4::default();
            }
            ParsedL4 {
                tuple: PackedTuple {
                    src_ip,
                    dst_ip,
                    src_port,
                    dst_port,
                    proto,
                    _pad: [0; 3],
                },
                payload_offset: (34 + 8) as u16,
                payload_len: (udp_len - 8) as u16,
                tcp_seq: 0,
                tcp_ack: 0,
                tcp_flags: 0,
                tcp_window: 0,
                valid: true,
            }
        }
    } else {
        let ip_version = buf[14] >> 4;
        if ip_version != 6 {
            return ParsedL4::default();
        }

        let mut next_header = buf[20];
        let payload_len = u16::from_be_bytes([buf[18], buf[19]]) as usize;

        if len < 54 + payload_len {
            return ParsedL4::default();
        }

        let src_ip = [buf[22], buf[23], buf[24], buf[25]];
        let dst_ip = [buf[38], buf[39], buf[40], buf[41]];

        let mut header_offset = 54;
        let mut ext_count = 0;

        while ext_count < 4 {
            if next_header == 0 || next_header == 43 || next_header == 60 {
                if len < header_offset + 8 {
                    return ParsedL4::default();
                }
                next_header = frame[header_offset];
                let ext_len = ((frame[header_offset + 1] as usize) + 1) * 8;
                header_offset += ext_len;
                ext_count += 1;
            } else {
                break;
            }
        }

        if next_header != 6 && next_header != 17 {
            return ParsedL4::default();
        }

        if len < header_offset + 20 {
            return ParsedL4::default();
        }

        let src_port = u16::from_be_bytes([frame[header_offset], frame[header_offset + 1]]);
        let dst_port = u16::from_be_bytes([frame[header_offset + 2], frame[header_offset + 3]]);

        if next_header == 6 {
            let tcp_seq = u32::from_be_bytes([
                frame[header_offset + 4],
                frame[header_offset + 5],
                frame[header_offset + 6],
                frame[header_offset + 7],
            ]);
            let tcp_ack = u32::from_be_bytes([
                frame[header_offset + 8],
                frame[header_offset + 9],
                frame[header_offset + 10],
                frame[header_offset + 11],
            ]);
            let data_offset = frame[header_offset + 12] >> 4;
            let tcp_flags = frame[header_offset + 13];
            let tcp_window = u16::from_be_bytes([frame[header_offset + 14], frame[header_offset + 15]]);

            let tcp_hdr_len = (data_offset * 4) as usize;
            let payload_offset = header_offset + tcp_hdr_len;
            let actual_payload_len = payload_len.saturating_sub(tcp_hdr_len);

            ParsedL4 {
                tuple: PackedTuple {
                    src_ip,
                    dst_ip,
                    src_port,
                    dst_port,
                    proto: 6,
                    _pad: [0; 3],
                },
                payload_offset: payload_offset as u16,
                payload_len: actual_payload_len as u16,
                tcp_seq,
                tcp_ack,
                tcp_flags,
                tcp_window,
                valid: true,
            }
        } else {
            let udp_len = u16::from_be_bytes([frame[header_offset + 4], frame[header_offset + 5]]) as usize;
            ParsedL4 {
                tuple: PackedTuple {
                    src_ip,
                    dst_ip,
                    src_port,
                    dst_port,
                    proto: 17,
                    _pad: [0; 3],
                },
                payload_offset: (header_offset + 8) as u16,
                payload_len: udp_len.saturating_sub(8) as u16,
                tcp_seq: 0,
                tcp_ack: 0,
                tcp_flags: 0,
                tcp_window: 0,
                valid: true,
            }
        }
    }
}
