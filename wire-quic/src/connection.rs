use std::collections::HashMap;
use std::time::Instant;
use crate::crypto::derive_initial_keys;
use crate::frame::Frame;
use crate::packet::{parse_packet_header, ConnectionId, PacketType};
use crate::stream::QuicStream;
use crate::varint::VarInt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuicConnectionState {
    Initial,
    Handshake,
    Established,
    Draining,
    Closed,
}

pub struct QuicConnection {
    pub local_cid: ConnectionId,
    pub remote_cid: ConnectionId,
    pub state: QuicConnectionState,
    pub keys: Option<crate::crypto::QuicKeys>,
    pub next_packet_number: u64,
    pub largest_received_pn: u64,
    pub streams: HashMap<u64, QuicStream>,
    pub next_stream_id: u64,
    pub max_data: u64,
    pub max_stream_data: u64,
    pub last_packet_time: Instant,
    pub established_at: Option<Instant>,
}

impl QuicConnection {
    pub fn new_server(local_cid: ConnectionId, remote_cid: ConnectionId, now: Instant) -> Self {
        let keys = derive_initial_keys(local_cid.as_slice(), true);
        Self {
            local_cid,
            remote_cid,
            state: QuicConnectionState::Initial,
            keys: Some(keys),
            next_packet_number: 0,
            largest_received_pn: 0,
            streams: HashMap::new(),
            next_stream_id: 1,
            max_data: 10 * 1024 * 1024,
            max_stream_data: 2 * 1024 * 1024,
            last_packet_time: now,
            established_at: None,
        }
    }

    pub fn process_packet(
        &mut self,
        packet_slice: &[u8],
        now: Instant,
        out_packets: &mut Vec<Vec<u8>>,
    ) -> Result<(), ()> {
        self.last_packet_time = now;
        let header = match parse_packet_header(packet_slice, self.local_cid.len as usize) {
            Some(h) => h,
            None => return Err(()),
        };

        if self.remote_cid.len == 0 && header.src_cid.len > 0 {
            self.remote_cid = header.src_cid;
        }

        let mut decrypted_payload = packet_slice[header.header_len..].to_vec();

        if let Some(ref keys) = self.keys {
            let plain_len = keys
                .remote
                .decrypt_payload(
                    header.packet_number,
                    &packet_slice[..header.header_len],
                    &mut decrypted_payload,
                )
                .map_err(|_| ())?;
            decrypted_payload.truncate(plain_len);
        }

        self.largest_received_pn = self.largest_received_pn.max(header.packet_number);

        let mut offset = 0;
        let mut ack_needed = false;
        let mut handshake_done = false;

        while offset < decrypted_payload.len() {
            if let Some((frame, f_len)) = Frame::parse(&decrypted_payload[offset..]) {
                offset += f_len;
                match frame {
                    Frame::Ping => {
                        ack_needed = true;
                    }
                    Frame::Crypto { offset: _, data: _ } => {
                        ack_needed = true;
                        if self.state == QuicConnectionState::Initial {
                            self.state = QuicConnectionState::Handshake;
                            self.send_handshake_response(out_packets);
                        } else if self.state == QuicConnectionState::Handshake {
                            self.state = QuicConnectionState::Established;
                            self.established_at = Some(now);
                            handshake_done = true;
                        }
                    }
                    Frame::Stream { stream_id, offset, fin, data } => {
                        ack_needed = true;
                        let stream = self
                            .streams
                            .entry(stream_id)
                            .or_insert_with(|| QuicStream::new(stream_id, self.max_stream_data));
                        let _ = stream.receive_fragment(offset, fin, data);
                    }
                    Frame::ConnectionClose { .. } => {
                        self.state = QuicConnectionState::Closed;
                        return Ok(());
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }

        if handshake_done {
            self.send_handshake_done(out_packets);
        }

        if ack_needed && self.state == QuicConnectionState::Established {
            self.send_ack(out_packets);
        }

        Ok(())
    }

    fn send_handshake_response(&mut self, out_packets: &mut Vec<Vec<u8>>) {
        let mut frames_buf = Vec::new();
        let crypto_frame = Frame::Crypto {
            offset: 0,
            data: b"SERVER_HELLO_0RTT_TLS13",
        };
        crypto_frame.encode(&mut frames_buf);

        let pkt = self.build_long_packet(PacketType::Initial, &frames_buf);
        out_packets.push(pkt);
    }

    fn send_handshake_done(&mut self, out_packets: &mut Vec<Vec<u8>>) {
        let mut frames_buf = Vec::new();
        Frame::HandshakeDone.encode(&mut frames_buf);
        let pkt = self.build_short_packet(&frames_buf);
        out_packets.push(pkt);
    }

    fn send_ack(&mut self, out_packets: &mut Vec<Vec<u8>>) {
        let mut frames_buf = Vec::new();
        let ack_frame = Frame::Ack {
            largest_acknowledged: self.largest_received_pn,
            ack_delay: 0,
            first_ack_range: 0,
            ranges: smallvec::SmallVec::new(),
        };
        ack_frame.encode(&mut frames_buf);
        let pkt = self.build_short_packet(&frames_buf);
        out_packets.push(pkt);
    }

    pub fn send_stream_data(
        &mut self,
        stream_id: u64,
        data: &[u8],
        fin: bool,
        out_packets: &mut Vec<Vec<u8>>,
    ) {
        let stream = self
            .streams
            .entry(stream_id)
            .or_insert_with(|| QuicStream::new(stream_id, self.max_stream_data));

        let offset = stream.tx_offset;
        stream.tx_offset += data.len() as u64;

        let frame = Frame::Stream {
            stream_id,
            offset,
            fin,
            data,
        };
        let mut frames_buf = Vec::new();
        frame.encode(&mut frames_buf);

        let pkt = self.build_short_packet(&frames_buf);
        out_packets.push(pkt);
    }

    fn build_long_packet(&mut self, pkt_type: PacketType, payload: &[u8]) -> Vec<u8> {
        let pn = self.next_packet_number;
        self.next_packet_number += 1;

        let mut out = Vec::new();
        let type_flag = match pkt_type {
            PacketType::Initial => 0x00,
            PacketType::ZeroRtt => 0x01,
            PacketType::Handshake => 0x02,
            PacketType::Retry => 0x03,
            _ => 0x00,
        };
        out.push(0xc0 | (type_flag << 4) | 0x03);
        out.extend_from_slice(&1u32.to_be_bytes());

        out.push(self.remote_cid.len);
        out.extend_from_slice(self.remote_cid.as_slice());

        out.push(self.local_cid.len);
        out.extend_from_slice(self.local_cid.as_slice());

        if pkt_type == PacketType::Initial {
            VarInt(0).encode_vec(&mut out);
        }

        let encrypted_len = payload.len() + 16 + 4;
        VarInt(encrypted_len as u64).encode_vec(&mut out);

        let pn_bytes = (pn as u32).to_be_bytes();
        out.extend_from_slice(&pn_bytes);

        let mut cipher_payload = payload.to_vec();
        if let Some(ref keys) = self.keys {
            keys.local.encrypt_payload(pn, &out, &mut cipher_payload);
        }

        out.extend_from_slice(&cipher_payload);
        out
    }

    fn build_short_packet(&mut self, payload: &[u8]) -> Vec<u8> {
        let pn = self.next_packet_number;
        self.next_packet_number += 1;

        let mut out = Vec::new();
        out.push(0x40 | 0x03);
        out.extend_from_slice(self.remote_cid.as_slice());

        let pn_bytes = (pn as u32).to_be_bytes();
        out.extend_from_slice(&pn_bytes);

        let mut cipher_payload = payload.to_vec();
        if let Some(ref keys) = self.keys {
            keys.local.encrypt_payload(pn, &out, &mut cipher_payload);
        }

        out.extend_from_slice(&cipher_payload);
        out
    }
}
