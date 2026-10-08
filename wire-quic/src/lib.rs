pub mod varint;
pub mod crypto;
pub mod packet;
pub mod frame;
pub mod stream;
pub mod h3;
pub mod connection;

use std::collections::HashMap;
use std::time::Instant;
use connection::QuicConnection;
use packet::{parse_packet_header, ConnectionId};
use h3::{H3Frame, H3Header};

pub struct QuicEngine {
    pub connections: HashMap<ConnectionId, QuicConnection>,
    pub default_cid_len: usize,
}

impl QuicEngine {
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
            default_cid_len: 8,
        }
    }

    pub fn process_incoming_datagram(
        &mut self,
        datagram: &[u8],
        now: Instant,
        out_datagrams: &mut Vec<Vec<u8>>,
    ) -> Result<Option<ConnectionId>, ()> {
        if datagram.is_empty() {
            return Ok(None);
        }

        let header = parse_packet_header(datagram, self.default_cid_len).ok_or(())?;
        let dst_cid = header.dst_cid;

        if !self.connections.contains_key(&dst_cid) {
            let mut server_cid = ConnectionId::default();
            server_cid.len = 8;
            server_cid.bytes[0..8].copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);

            let conn = QuicConnection::new_server(server_cid, header.src_cid, now);
            self.connections.insert(dst_cid, conn);
        }

        if let Some(conn) = self.connections.get_mut(&dst_cid) {
            conn.process_packet(datagram, now, out_datagrams)?;
            return Ok(Some(dst_cid));
        }

        Ok(None)
    }

    pub fn send_h3_response(
        &mut self,
        cid: &ConnectionId,
        stream_id: u64,
        headers: Vec<H3Header>,
        body: &[u8],
        out_datagrams: &mut Vec<Vec<u8>>,
    ) {
        if let Some(conn) = self.connections.get_mut(cid) {
            let mut h3_payload = Vec::new();
            let hdr_frame = H3Frame::Headers(headers);
            hdr_frame.encode(&mut h3_payload);

            let data_frame = H3Frame::Data(body.to_vec());
            data_frame.encode(&mut h3_payload);

            conn.send_stream_data(stream_id, &h3_payload, true, out_datagrams);
        }
    }
}
