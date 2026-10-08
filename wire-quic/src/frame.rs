use crate::varint::VarInt;
use smallvec::SmallVec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame<'a> {
    Padding { len: usize },
    Ping,
    Ack {
        largest_acknowledged: u64,
        ack_delay: u64,
        first_ack_range: u64,
        ranges: SmallVec<[(u64, u64); 8]>,
    },
    ResetStream {
        stream_id: u64,
        error_code: u64,
        final_size: u64,
    },
    StopSending {
        stream_id: u64,
        error_code: u64,
    },
    Crypto {
        offset: u64,
        data: &'a [u8],
    },
    NewToken {
        token: &'a [u8],
    },
    Stream {
        stream_id: u64,
        offset: u64,
        fin: bool,
        data: &'a [u8],
    },
    MaxData {
        max_data: u64,
    },
    MaxStreamData {
        stream_id: u64,
        max_stream_data: u64,
    },
    MaxStreams {
        is_bidi: bool,
        max_streams: u64,
    },
    DataBlocked {
        max_data: u64,
    },
    StreamDataBlocked {
        stream_id: u64,
        max_stream_data: u64,
    },
    StreamsBlocked {
        is_bidi: bool,
        max_streams: u64,
    },
    NewConnectionId {
        sequence_number: u64,
        retire_prior_to: u64,
        connection_id: &'a [u8],
        stateless_reset_token: [u8; 16],
    },
    RetireConnectionId {
        sequence_number: u64,
    },
    PathChallenge {
        data: [u8; 8],
    },
    PathResponse {
        data: [u8; 8],
    },
    ConnectionClose {
        error_code: u64,
        frame_type: Option<u64>,
        reason: &'a str,
    },
    HandshakeDone,
}

impl<'a> Frame<'a> {
    pub fn parse(buf: &'a [u8]) -> Option<(Self, usize)> {
        if buf.is_empty() {
            return None;
        }
        let (frame_type, mut offset) = VarInt::decode(buf)?;
        let ftype = frame_type.0;

        match ftype {
            0x00 => {
                let mut p_len = 1;
                while offset + p_len - 1 < buf.len() && buf[offset + p_len - 1] == 0 {
                    p_len += 1;
                }
                Some((Frame::Padding { len: p_len }, p_len))
            }
            0x01 => Some((Frame::Ping, offset)),
            0x02 | 0x03 => {
                let (largest_ack, l_len) = VarInt::decode(&buf[offset..])?;
                offset += l_len;
                let (ack_delay, d_len) = VarInt::decode(&buf[offset..])?;
                offset += d_len;
                let (range_count, rc_len) = VarInt::decode(&buf[offset..])?;
                offset += rc_len;
                let (first_range, f_len) = VarInt::decode(&buf[offset..])?;
                offset += f_len;

                let mut ranges = SmallVec::new();
                let mut current_smallest = largest_ack.0.saturating_sub(first_range.0);

                for _ in 0..range_count.0 {
                    let (gap, g_len) = VarInt::decode(&buf[offset..])?;
                    offset += g_len;
                    let (ack_range, r_len) = VarInt::decode(&buf[offset..])?;
                    offset += r_len;

                    let range_largest = current_smallest.saturating_sub(gap.0 + 2);
                    let range_smallest = range_largest.saturating_sub(ack_range.0);
                    ranges.push((range_smallest, range_largest));
                    current_smallest = range_smallest;
                }

                Some((
                    Frame::Ack {
                        largest_acknowledged: largest_ack.0,
                        ack_delay: ack_delay.0,
                        first_ack_range: first_range.0,
                        ranges,
                    },
                    offset,
                ))
            }
            0x04 => {
                let (stream_id, s_len) = VarInt::decode(&buf[offset..])?;
                offset += s_len;
                let (error_code, e_len) = VarInt::decode(&buf[offset..])?;
                offset += e_len;
                let (final_size, f_len) = VarInt::decode(&buf[offset..])?;
                offset += f_len;
                Some((
                    Frame::ResetStream {
                        stream_id: stream_id.0,
                        error_code: error_code.0,
                        final_size: final_size.0,
                    },
                    offset,
                ))
            }
            0x05 => {
                let (stream_id, s_len) = VarInt::decode(&buf[offset..])?;
                offset += s_len;
                let (error_code, e_len) = VarInt::decode(&buf[offset..])?;
                offset += e_len;
                Some((
                    Frame::StopSending {
                        stream_id: stream_id.0,
                        error_code: error_code.0,
                    },
                    offset,
                ))
            }
            0x06 => {
                let (crypto_off, c_len) = VarInt::decode(&buf[offset..])?;
                offset += c_len;
                let (length_var, l_len) = VarInt::decode(&buf[offset..])?;
                offset += l_len;
                let d_len = length_var.0 as usize;
                if buf.len() < offset + d_len {
                    return None;
                }
                let data = &buf[offset..offset + d_len];
                offset += d_len;
                Some((Frame::Crypto { offset: crypto_off.0, data }, offset))
            }
            0x07 => {
                let (len_var, l_len) = VarInt::decode(&buf[offset..])?;
                offset += l_len;
                let t_len = len_var.0 as usize;
                if buf.len() < offset + t_len {
                    return None;
                }
                let token = &buf[offset..offset + t_len];
                offset += t_len;
                Some((Frame::NewToken { token }, offset))
            }
            0x08..=0x0f => {
                let has_off = (ftype & 0x04) != 0;
                let has_len = (ftype & 0x02) != 0;
                let fin = (ftype & 0x01) != 0;

                let (stream_id, s_len) = VarInt::decode(&buf[offset..])?;
                offset += s_len;

                let stream_off = if has_off {
                    let (o, o_len) = VarInt::decode(&buf[offset..])?;
                    offset += o_len;
                    o.0
                } else {
                    0
                };

                let data_len = if has_len {
                    let (l, l_len) = VarInt::decode(&buf[offset..])?;
                    offset += l_len;
                    l.0 as usize
                } else {
                    buf.len() - offset
                };

                if buf.len() < offset + data_len {
                    return None;
                }
                let data = &buf[offset..offset + data_len];
                offset += data_len;

                Some((
                    Frame::Stream {
                        stream_id: stream_id.0,
                        offset: stream_off,
                        fin,
                        data,
                    },
                    offset,
                ))
            }
            0x10 => {
                let (max_data, m_len) = VarInt::decode(&buf[offset..])?;
                offset += m_len;
                Some((Frame::MaxData { max_data: max_data.0 }, offset))
            }
            0x11 => {
                let (stream_id, s_len) = VarInt::decode(&buf[offset..])?;
                offset += s_len;
                let (max_stream_data, m_len) = VarInt::decode(&buf[offset..])?;
                offset += m_len;
                Some((
                    Frame::MaxStreamData {
                        stream_id: stream_id.0,
                        max_stream_data: max_stream_data.0,
                    },
                    offset,
                ))
            }
            0x12 => {
                let (max_streams, m_len) = VarInt::decode(&buf[offset..])?;
                offset += m_len;
                Some((Frame::MaxStreams { is_bidi: true, max_streams: max_streams.0 }, offset))
            }
            0x13 => {
                let (max_streams, m_len) = VarInt::decode(&buf[offset..])?;
                offset += m_len;
                Some((Frame::MaxStreams { is_bidi: false, max_streams: max_streams.0 }, offset))
            }
            0x1a => {
                if buf.len() < offset + 8 {
                    return None;
                }
                let mut data = [0u8; 8];
                data.copy_from_slice(&buf[offset..offset + 8]);
                offset += 8;
                Some((Frame::PathChallenge { data }, offset))
            }
            0x1b => {
                if buf.len() < offset + 8 {
                    return None;
                }
                let mut data = [0u8; 8];
                data.copy_from_slice(&buf[offset..offset + 8]);
                offset += 8;
                Some((Frame::PathResponse { data }, offset))
            }
            0x1c | 0x1d => {
                let (error_code, e_len) = VarInt::decode(&buf[offset..])?;
                offset += e_len;
                let frame_type = if ftype == 0x1c {
                    let (ft, f_len) = VarInt::decode(&buf[offset..])?;
                    offset += f_len;
                    Some(ft.0)
                } else {
                    None
                };
                let (reason_len, r_len) = VarInt::decode(&buf[offset..])?;
                offset += r_len;
                let rlen = reason_len.0 as usize;
                if buf.len() < offset + rlen {
                    return None;
                }
                let reason = std::str::from_utf8(&buf[offset..offset + rlen]).unwrap_or("");
                offset += rlen;
                Some((Frame::ConnectionClose { error_code: error_code.0, frame_type, reason }, offset))
            }
            0x1e => Some((Frame::HandshakeDone, offset)),
            _ => None,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Frame::Padding { len } => {
                out.resize(out.len() + *len, 0);
            }
            Frame::Ping => {
                out.push(0x01);
            }
            Frame::Ack {
                largest_acknowledged,
                ack_delay,
                first_ack_range,
                ranges,
            } => {
                out.push(0x02);
                VarInt(*largest_acknowledged).encode_vec(out);
                VarInt(*ack_delay).encode_vec(out);
                VarInt(ranges.len() as u64).encode_vec(out);
                VarInt(*first_ack_range).encode_vec(out);

                let mut current_smallest = largest_acknowledged.saturating_sub(*first_ack_range);
                for &(range_smallest, range_largest) in ranges {
                    let gap = current_smallest.saturating_sub(range_largest + 2);
                    let range_len = range_largest.saturating_sub(range_smallest);
                    VarInt(gap).encode_vec(out);
                    VarInt(range_len).encode_vec(out);
                    current_smallest = range_smallest;
                }
            }
            Frame::Crypto { offset, data } => {
                out.push(0x06);
                VarInt(*offset).encode_vec(out);
                VarInt(data.len() as u64).encode_vec(out);
                out.extend_from_slice(data);
            }
            Frame::Stream { stream_id, offset, fin, data } => {
                let mut type_byte = 0x08;
                if *offset > 0 {
                    type_byte |= 0x04;
                }
                type_byte |= 0x02;
                if *fin {
                    type_byte |= 0x01;
                }
                out.push(type_byte);
                VarInt(*stream_id).encode_vec(out);
                if *offset > 0 {
                    VarInt(*offset).encode_vec(out);
                }
                VarInt(data.len() as u64).encode_vec(out);
                out.extend_from_slice(data);
            }
            Frame::MaxData { max_data } => {
                out.push(0x10);
                VarInt(*max_data).encode_vec(out);
            }
            Frame::MaxStreamData { stream_id, max_stream_data } => {
                out.push(0x11);
                VarInt(*stream_id).encode_vec(out);
                VarInt(*max_stream_data).encode_vec(out);
            }
            Frame::MaxStreams { is_bidi, max_streams } => {
                out.push(if *is_bidi { 0x12 } else { 0x13 });
                VarInt(*max_streams).encode_vec(out);
            }
            Frame::PathChallenge { data } => {
                out.push(0x1a);
                out.extend_from_slice(data);
            }
            Frame::PathResponse { data } => {
                out.push(0x1b);
                out.extend_from_slice(data);
            }
            Frame::ConnectionClose { error_code, frame_type, reason } => {
                if let Some(ft) = frame_type {
                    out.push(0x1c);
                    VarInt(*error_code).encode_vec(out);
                    VarInt(*ft).encode_vec(out);
                } else {
                    out.push(0x1d);
                    VarInt(*error_code).encode_vec(out);
                }
                VarInt(reason.len() as u64).encode_vec(out);
                out.extend_from_slice(reason.as_bytes());
            }
            Frame::HandshakeDone => {
                out.push(0x1e);
            }
            _ => {}
        }
    }
}
