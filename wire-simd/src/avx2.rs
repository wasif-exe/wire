use wire_core::types::PackedTuple;
use crate::ParsedL4;
use crate::masks::ETH_TCP_SHUFFLE_OCTETS;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[inline(always)]
pub unsafe fn parse_one_avx2(frame: &[u8]) -> ParsedL4 {
    let len = frame.len();
    if len < 54 {
        return crate::scalar::parse_one_scalar(frame);
    }
    let ptr = frame.as_ptr();
    
    // SAFETY: Verified frame length is >= 64 bytes to allow full 256-bit vector load.
    if len < 64 {
        return crate::scalar::parse_one_scalar(frame);
    }

    let chunk0 = _mm256_loadu_si256(ptr as *const __m256i);

    let ethertype = _mm256_extract_epi16_unwrap(chunk0, 6);
    if ethertype != 0x0008 {
        return ParsedL4::default();
    }

    let ver_ihl = _mm256_extract_epi8_unwrap(chunk0, 14);
    if (ver_ihl >> 4) != 4 {
        return ParsedL4::default();
    }
    let ihl = ver_ihl & 0x0F;
    if ihl != 5 {
        return crate::scalar::parse_one_scalar(frame);
    }

    let proto = _mm256_extract_epi8_unwrap(chunk0, 23);
    if proto != 6 && proto != 17 {
        return ParsedL4::default();
    }

    let mut sum: u32 = 0;
    let ip_bytes_ptr = &frame[14..34];
    for chunk in ip_bytes_ptr.chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    if !sum as u16 != 0 {
        return ParsedL4::default();
    }

    let shuffle_mask = _mm256_loadu_si256(ETH_TCP_SHUFFLE_OCTETS.as_ptr() as *const __m256i);
    let shuffled = _mm256_shuffle_epi8(chunk0, shuffle_mask);
    
    let mut tuple_storage = [0u8; 32];
    _mm256_storeu_si256(tuple_storage.as_mut_ptr() as *mut __m256i, shuffled);

    let mut src_ip = [0u8; 4];
    let mut dst_ip = [0u8; 4];
    src_ip.copy_from_slice(&tuple_storage[0..4]);
    dst_ip.copy_from_slice(&tuple_storage[4..8]);

    let src_port = u16::from_be_bytes([tuple_storage[8], tuple_storage[9]]);
    let dst_port = u16::from_be_bytes([tuple_storage[10], tuple_storage[11]]);

    let total_len = u16::from_be_bytes([frame[16], frame[17]]) as usize;
    if len < 14 + total_len {
        return ParsedL4::default();
    }

    if proto == 6 {
        let tcp_seq = u32::from_be_bytes([frame[38], frame[39], frame[40], frame[41]]);
        let tcp_ack = u32::from_be_bytes([frame[42], frame[43], frame[44], frame[45]]);
        let data_offset = frame[46] >> 4;
        let tcp_flags = frame[47];
        let tcp_window = u16::from_be_bytes([frame[48], frame[49]]);
        
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
        let udp_len = u16::from_be_bytes([frame[38], frame[39]]) as usize;
        if udp_len < 8 || total_len < 20 + udp_len {
            return ParsedL4::default();
        }
        let payload_offset = 34 + 8;
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

#[inline(always)]
unsafe fn _mm256_extract_epi16_unwrap(val: __m256i, idx: i32) -> u16 {
    let mut arr = [0u16; 16];
    _mm256_storeu_si256(arr.as_mut_ptr() as *mut __m256i, val);
    arr[idx as usize]
}

#[inline(always)]
unsafe fn _mm256_extract_epi8_unwrap(val: __m256i, idx: i32) -> u8 {
    let mut arr = [0u8; 32];
    _mm256_storeu_si256(arr.as_mut_ptr() as *mut __m256i, val);
    arr[idx as usize]
}
