use bytes::BytesMut;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Open,
    HalfClosedRemote,
    HalfClosedLocal,
    Closed,
}

pub struct QuicStream {
    pub stream_id: u64,
    pub state: StreamState,
    pub rx_offset: u64,
    pub rx_buffered: BTreeMap<u64, Vec<u8>>,
    pub rx_consumed: u64,
    pub tx_offset: u64,
    pub tx_buffered: BytesMut,
    pub max_rx_data: u64,
    pub max_tx_data: u64,
    pub fin_received: bool,
}

impl QuicStream {
    pub fn new(stream_id: u64, initial_max_data: u64) -> Self {
        Self {
            stream_id,
            state: StreamState::Open,
            rx_offset: 0,
            rx_buffered: BTreeMap::new(),
            rx_consumed: 0,
            tx_offset: 0,
            tx_buffered: BytesMut::new(),
            max_rx_data: initial_max_data,
            max_tx_data: initial_max_data,
            fin_received: false,
        }
    }

    pub fn receive_fragment(&mut self, offset: u64, fin: bool, data: &[u8]) -> Result<(), ()> {
        if fin {
            self.fin_received = true;
        }

        if data.is_empty() {
            if self.fin_received && self.rx_offset == self.rx_consumed {
                self.transition_rx_fin();
            }
            return Ok(());
        }

        let end_offset = offset + data.len() as u64;
        if end_offset > self.max_rx_data {
            return Err(());
        }

        if offset <= self.rx_offset {
            let start = (self.rx_offset - offset) as usize;
            if start < data.len() {
                let valid_data = &data[start..];
                self.rx_buffered.insert(self.rx_offset, valid_data.to_vec());
                self.rx_offset += valid_data.len() as u64;
            }
        } else {
            self.rx_buffered.insert(offset, data.to_vec());
        }

        while let Some((&chunk_off, _)) = self.rx_buffered.range(self.rx_consumed..).next() {
            if chunk_off == self.rx_consumed {
                let chunk = self.rx_buffered.remove(&chunk_off).unwrap();
                self.rx_consumed += chunk.len() as u64;
                if chunk_off + chunk.len() as u64 > self.rx_offset {
                    self.rx_offset = chunk_off + chunk.len() as u64;
                }
            } else {
                break;
            }
        }

        if self.fin_received && self.rx_offset == self.rx_consumed {
            self.transition_rx_fin();
        }

        Ok(())
    }

    pub fn read_data(&mut self) -> Option<Vec<u8>> {
        if self.rx_buffered.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        let mut to_remove = Vec::new();

        for (&off, chunk) in &self.rx_buffered {
            out.extend_from_slice(chunk);
            to_remove.push(off);
        }

        for off in to_remove {
            self.rx_buffered.remove(&off);
        }

        Some(out)
    }

    pub fn write_data(&mut self, data: &[u8]) {
        self.tx_buffered.extend_from_slice(data);
    }

    fn transition_rx_fin(&mut self) {
        match self.state {
            StreamState::Open => self.state = StreamState::HalfClosedRemote,
            StreamState::HalfClosedLocal => self.state = StreamState::Closed,
            _ => {}
        }
    }

    pub fn close_local(&mut self) {
        match self.state {
            StreamState::Open => self.state = StreamState::HalfClosedLocal,
            StreamState::HalfClosedRemote => self.state = StreamState::Closed,
            _ => {}
        }
    }
}
