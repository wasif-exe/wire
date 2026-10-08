use crate::varint::VarInt;
use smallvec::SmallVec;

pub const MAX_CID_LEN: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConnectionId {
    pub len: u8,
    pub bytes: [u8; MAX_CID_LEN],
}

impl std::fmt::Debug for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CID({:02x?})", &self.bytes[..self.len as usize])
    }
}

impl ConnectionId {
    #[inline(always)]
    pub fn new(slice: &[u8]) -> Self {
        let len = slice.len().min(MAX_CID_LEN) as u8;
        let mut bytes = [0u8; MAX_CID_LEN];
        bytes[..len as usize].copy_from_slice(&slice[..len as usize]);
        Self { len, bytes }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Initial,
    ZeroRtt,
    Handshake,
    Retry,
    OneRtt,
}

#[derive(Debug, Clone)]
pub struct PacketHeader {
    pub packet_type: PacketType,
    pub version: u32,
    pub dst_cid: ConnectionId,
    pub src_cid: ConnectionId,
    pub token: SmallVec<[u8; 64]>,
    pub length: u64,
    pub packet_number: u64,
    pub pn_len: usize,
    pub header_len: usize,
}

pub fn decode_packet_number(largest_pn: u64, truncated_pn: u64, pn_nbits: usize) -> u64 {
    let expected_pn = largest_pn + 1;
    let pn_win = 1u64 << pn_nbits;
    let pn_hwin = pn_win / 2;
    let pn_mask = pn_win - 1;
    let candidate_pn = (expected_pn & !pn_mask) | truncated_pn;

    if candidate_pn + pn_hwin <= expected_pn && candidate_pn + pn_win < (1 << 62) {
        candidate_pn + pn_win
    } else if candidate_pn > expected_pn + pn_hwin && candidate_pn >= pn_win {
        candidate_pn - pn_win
    } else {
        candidate_pn
    }
}

pub fn parse_packet_header(raw: &[u8], local_cid_len: usize) -> Option<PacketHeader> {
    if raw.is_empty() {
        return None;
    }
    let first = raw[0];
    let is_long_header = (first & 0x80) != 0;

    if is_long_header {
        if raw.len() < 5 {
            return None;
        }
        let version = u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]);
        let pkt_type = match (first >> 4) & 0x03 {
            0x00 => PacketType::Initial,
            0x01 => PacketType::ZeroRtt,
            0x02 => PacketType::Handshake,
            0x03 => PacketType::Retry,
            _ => return None,
        };

        let mut offset = 5;
        if raw.len() < offset + 1 {
            return None;
        }
        let dst_len = raw[offset] as usize;
        offset += 1;
        if raw.len() < offset + dst_len + 1 || dst_len > MAX_CID_LEN {
            return None;
        }
        let dst_cid = ConnectionId::new(&raw[offset..offset + dst_len]);
        offset += dst_len;

        let src_len = raw[offset] as usize;
        offset += 1;
        if raw.len() < offset + src_len || src_len > MAX_CID_LEN {
            return None;
        }
        let src_cid = ConnectionId::new(&raw[offset..offset + src_len]);
        offset += src_len;

        let mut token = SmallVec::new();
        if pkt_type == PacketType::Initial {
            let (token_len_var, vlen) = VarInt::decode(&raw[offset..])?;
            offset += vlen;
            let tok_len = token_len_var.0 as usize;
            if raw.len() < offset + tok_len {
                return None;
            }
            token.extend_from_slice(&raw[offset..offset + tok_len]);
            offset += tok_len;
        }

        let (length_var, vlen) = VarInt::decode(&raw[offset..])?;
        offset += vlen;
        let length = length_var.0;

        let pn_len = ((first & 0x03) + 1) as usize;
        if raw.len() < offset + pn_len {
            return None;
        }

        let mut pn_bytes = [0u8; 8];
        pn_bytes[8 - pn_len..].copy_from_slice(&raw[offset..offset + pn_len]);
        let packet_number = u64::from_be_bytes(pn_bytes);
        offset += pn_len;

        Some(PacketHeader {
            packet_type: pkt_type,
            version,
            dst_cid,
            src_cid,
            token,
            length,
            packet_number,
            pn_len,
            header_len: offset,
        })
    } else {
        if raw.len() < 1 + local_cid_len {
            return None;
        }
        let dst_cid = ConnectionId::new(&raw[1..1 + local_cid_len]);
        let mut offset = 1 + local_cid_len;

        let pn_len = ((first & 0x03) + 1) as usize;
        if raw.len() < offset + pn_len {
            return None;
        }
        let mut pn_bytes = [0u8; 8];
        pn_bytes[8 - pn_len..].copy_from_slice(&raw[offset..offset + pn_len]);
        let packet_number = u64::from_be_bytes(pn_bytes);
        offset += pn_len;

        Some(PacketHeader {
            packet_type: PacketType::OneRtt,
            version: 1,
            dst_cid,
            src_cid: ConnectionId::default(),
            token: SmallVec::new(),
            length: (raw.len() - offset) as u64,
            packet_number,
            pn_len,
            header_len: offset,
        })
    }
}
