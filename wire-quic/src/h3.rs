use crate::varint::VarInt;
use std::collections::HashMap;

pub const H3_FRAME_DATA: u64 = 0x00;
pub const H3_FRAME_HEADERS: u64 = 0x01;
pub const H3_FRAME_CANCEL_PUSH: u64 = 0x03;
pub const H3_FRAME_SETTINGS: u64 = 0x04;
pub const H3_FRAME_PUSH_PROMISE: u64 = 0x05;
pub const H3_FRAME_GOAWAY: u64 = 0x07;
pub const H3_FRAME_MAX_PUSH_ID: u64 = 0x0d;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H3Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H3Frame {
    Data(Vec<u8>),
    Headers(Vec<H3Header>),
    Settings(HashMap<u64, u64>),
    Goaway(u64),
}

pub struct QpackCodec;

const QPACK_STATIC_TABLE: &[(&str, &str)] = &[
    (":authority", ""),
    (":path", "/"),
    (":method", "GET"),
    (":method", "POST"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-encoding", "gzip, deflate, br"),
    ("content-length", "0"),
    ("content-type", "application/octet-stream"),
    ("content-type", "application/json"),
    ("content-type", "text/plain; charset=utf-8"),
    ("server", "wire-v6"),
];

impl QpackCodec {
    pub fn encode_headers(headers: &[H3Header], out: &mut Vec<u8>) {
        out.push(0x00);
        out.push(0x00);

        for h in headers {
            let mut matched = false;
            for (idx, &(s_name, s_val)) in QPACK_STATIC_TABLE.iter().enumerate() {
                if h.name == s_name && h.value == s_val {
                    out.push(0xc0 | (idx as u8));
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }

            let mut name_idx = None;
            for (idx, &(s_name, _)) in QPACK_STATIC_TABLE.iter().enumerate() {
                if h.name == s_name {
                    name_idx = Some(idx);
                    break;
                }
            }

            if let Some(idx) = name_idx {
                out.push(0x50 | (idx as u8));
                Self::encode_string(&h.value, out);
            } else {
                out.push(0x20);
                Self::encode_string(&h.name, out);
                Self::encode_string(&h.value, out);
            }
        }
    }

    pub fn decode_headers(raw: &[u8]) -> Option<Vec<H3Header>> {
        if raw.len() < 2 {
            return None;
        }
        let mut offset = 2;
        let mut headers = Vec::new();

        while offset < raw.len() {
            let b = raw[offset];
            if (b & 0xc0) == 0xc0 {
                let idx = (b & 0x3f) as usize;
                offset += 1;
                if idx < QPACK_STATIC_TABLE.len() {
                    headers.push(H3Header {
                        name: QPACK_STATIC_TABLE[idx].0.to_string(),
                        value: QPACK_STATIC_TABLE[idx].1.to_string(),
                    });
                }
            } else if (b & 0xf0) == 0x50 {
                let idx = (b & 0x0f) as usize;
                offset += 1;
                let (val, v_len) = Self::decode_string(&raw[offset..])?;
                offset += v_len;
                if idx < QPACK_STATIC_TABLE.len() {
                    headers.push(H3Header {
                        name: QPACK_STATIC_TABLE[idx].0.to_string(),
                        value: val,
                    });
                }
            } else if (b & 0xe0) == 0x20 {
                offset += 1;
                let (name, n_len) = Self::decode_string(&raw[offset..])?;
                offset += n_len;
                let (value, v_len) = Self::decode_string(&raw[offset..])?;
                offset += v_len;
                headers.push(H3Header { name, value });
            } else {
                offset += 1;
            }
        }
        Some(headers)
    }

    fn encode_string(s: &str, out: &mut Vec<u8>) {
        let bytes = s.as_bytes();
        VarInt(bytes.len() as u64).encode_vec(out);
        out.extend_from_slice(bytes);
    }

    fn decode_string(raw: &[u8]) -> Option<(String, usize)> {
        let (len_var, vlen) = VarInt::decode(raw)?;
        let len = len_var.0 as usize;
        let start = vlen;
        let end = start + len;
        if raw.len() < end {
            return None;
        }
        let s = String::from_utf8_lossy(&raw[start..end]).into_owned();
        Some((s, end))
    }
}

impl H3Frame {
    pub fn parse(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.is_empty() {
            return None;
        }
        let (frame_type, f_len) = VarInt::decode(buf)?;
        let mut offset = f_len;
        let (payload_len, l_len) = VarInt::decode(&buf[offset..])?;
        offset += l_len;
        let plen = payload_len.0 as usize;

        if buf.len() < offset + plen {
            return None;
        }

        let payload = &buf[offset..offset + plen];
        offset += plen;

        match frame_type.0 {
            H3_FRAME_DATA => Some((H3Frame::Data(payload.to_vec()), offset)),
            H3_FRAME_HEADERS => {
                let hdrs = QpackCodec::decode_headers(payload)?;
                Some((H3Frame::Headers(hdrs), offset))
            }
            H3_FRAME_SETTINGS => {
                let mut settings = HashMap::new();
                let mut s_off = 0;
                while s_off < payload.len() {
                    let (k, k_len) = VarInt::decode(&payload[s_off..])?;
                    s_off += k_len;
                    let (v, v_len) = VarInt::decode(&payload[s_off..])?;
                    s_off += v_len;
                    settings.insert(k.0, v.0);
                }
                Some((H3Frame::Settings(settings), offset))
            }
            H3_FRAME_GOAWAY => {
                let (id, _) = VarInt::decode(payload)?;
                Some((H3Frame::Goaway(id.0), offset))
            }
            _ => None,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            H3Frame::Data(data) => {
                VarInt(H3_FRAME_DATA).encode_vec(out);
                VarInt(data.len() as u64).encode_vec(out);
                out.extend_from_slice(data);
            }
            H3Frame::Headers(headers) => {
                let mut h_buf = Vec::new();
                QpackCodec::encode_headers(headers, &mut h_buf);
                VarInt(H3_FRAME_HEADERS).encode_vec(out);
                VarInt(h_buf.len() as u64).encode_vec(out);
                out.extend_from_slice(&h_buf);
            }
            H3Frame::Settings(settings) => {
                let mut s_buf = Vec::new();
                for (&k, &v) in settings {
                    VarInt(k).encode_vec(&mut s_buf);
                    VarInt(v).encode_vec(&mut s_buf);
                }
                VarInt(H3_FRAME_SETTINGS).encode_vec(out);
                VarInt(s_buf.len() as u64).encode_vec(out);
                out.extend_from_slice(&s_buf);
            }
            H3Frame::Goaway(id) => {
                let mut g_buf = Vec::new();
                VarInt(*id).encode_vec(&mut g_buf);
                VarInt(H3_FRAME_GOAWAY).encode_vec(out);
                VarInt(g_buf.len() as u64).encode_vec(out);
                out.extend_from_slice(&g_buf);
            }
        }
    }
}
