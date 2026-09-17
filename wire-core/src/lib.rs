use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacAddress(pub [u8; 6]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Address(pub [u8; 4]);

impl From<Ipv4Addr> for Ipv4Address {
    fn from(addr: Ipv4Addr) -> Self {
        Self(addr.octets())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub u32);

impl Seq {
    pub fn wrapping_add(self, val: u32) -> Self {
        Seq(self.0.wrapping_add(val))
    }

    pub fn wrapping_sub(self, val: Seq) -> u32 {
        self.0.wrapping_sub(val.0)
    }

    pub fn lt(self, other: Seq) -> bool {
        (self.0.wrapping_sub(other.0) as i32) < 0
    }

    pub fn lte(self, other: Seq) -> bool {
        self == other || self.lt(other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TcpFlags: u8 {
        const FIN = 0x01;
        const SYN = 0x02;
        const RST = 0x04;
        const PSH = 0x08;
        const ACK = 0x10;
        const URG = 0x20;
    }
}

pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        };
        sum += word;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn tcp_checksum(src: Ipv4Address, dst: Ipv4Address, tcp_segment: &[u8]) -> u16 {
    let mut pseudo_hdr = Vec::with_capacity(12 + tcp_segment.len());
    pseudo_hdr.extend_from_slice(&src.0);
    pseudo_hdr.extend_from_slice(&dst.0);
    pseudo_hdr.push(0);
    pseudo_hdr.push(6);
    pseudo_hdr.extend_from_slice(&(tcp_segment.len() as u16).to_be_bytes());
    pseudo_hdr.extend_from_slice(tcp_segment);
    checksum(&pseudo_hdr)
}

#[derive(Debug)]
pub struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: Seq,
    pub ack: Seq,
    pub data_offset: u8,
    pub flags: TcpFlags,
    pub window: u16,
    pub checksum: u16,
}

impl TcpHeader {
    pub fn parse(buf: &[u8]) -> Option<(Self, &[u8])> {
        if buf.len() < 20 { return None; }
        let src_port = u16::from_be_bytes([buf[0], buf[1]]);
        let dst_port = u16::from_be_bytes([buf[2], buf[3]]);
        let seq = Seq(u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]));
        let ack = Seq(u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]));
        let data_offset = (buf[12] >> 4) * 4;
        let flags = TcpFlags::from_bits_retain(buf[13]);
        let window = u16::from_be_bytes([buf[14], buf[15]]);
        let checksum = u16::from_be_bytes([buf[16], buf[17]]);

        if buf.len() < data_offset as usize { return None; }
        Some((TcpHeader {
            src_port, dst_port, seq, ack, data_offset, flags, window, checksum,
        }, &buf[data_offset as usize..]))
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = vec![0u8; 20];
        buf[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        buf[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        buf[4..8].copy_from_slice(&self.seq.0.to_be_bytes());
        buf[8..12].copy_from_slice(&self.ack.0.to_be_bytes());
        buf[12] = 5 << 4;
        buf[13] = self.flags.bits();
        buf[14..16].copy_from_slice(&self.window.to_be_bytes());
        buf
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SocketTuple {
    pub local_ip: Ipv4Address,
    pub local_port: u16,
    pub remote_ip: Ipv4Address,
    pub remote_port: u16,
}

pub struct SentSegment {
    pub seq: Seq,
    pub seq_len: u32,
    pub data: Vec<u8>,
    pub sent_at: Instant,
    pub retransmit_count: u32,
}

pub struct TcpConnection {
    pub tuple: SocketTuple,
    pub state: TcpState,
    pub local_mac: MacAddress,
    pub snd_una: Seq,
    pub snd_nxt: Seq,
    pub snd_wnd: u16,
    pub iss: Seq,
    pub rcv_nxt: Seq,
    pub rcv_wnd: u16,
    pub irs: Seq,
    pub retransmit_queue: VecDeque<SentSegment>,
    pub out_of_order: BTreeMap<u32, Vec<u8>>,
    pub send_buffer: VecDeque<u8>,
    pub receive_buffer: VecDeque<u8>,
    pub cwnd: u32,
    pub ssthresh: u32,
    pub dup_ack_count: u8,
    pub srtt: Option<Duration>,
    pub rttvar: Duration,
    pub rto: Duration,
    pub time_wait_expiry: Option<Instant>,
    pub mss: u16,
}

pub struct Stack {
    pub mac: MacAddress,
    pub ip: Ipv4Address,
    pub arp_cache: HashMap<Ipv4Address, MacAddress>,
    pub connections: HashMap<SocketTuple, TcpConnection>,
    pub listening_ports: HashMap<u16, TcpState>,
    pub tx_queue: VecDeque<Vec<u8>>,
    isn_counter: u32,
}

impl Stack {
    pub fn new(mac: MacAddress, ip: Ipv4Address) -> Self {
        Self {
            mac, ip,
            arp_cache: HashMap::new(),
            connections: HashMap::new(),
            listening_ports: HashMap::new(),
            tx_queue: VecDeque::new(),
            isn_counter: 21245643,
        }
    }

    pub fn listen(&mut self, port: u16) {
        self.listening_ports.insert(port, TcpState::Listen);
    }

    fn generate_isn(&mut self) -> Seq {
        self.isn_counter = self.isn_counter.wrapping_add(1043);
        Seq(self.isn_counter)
    }

    pub fn tcp_connect(&mut self, remote_ip: Ipv4Address, remote_port: u16, local_port: u16, now: Instant) -> SocketTuple {
        let tuple = SocketTuple {
            local_ip: self.ip,
            local_port,
            remote_ip,
            remote_port,
        };
        let iss = self.generate_isn();
        let mut conn = TcpConnection::new(tuple, TcpState::SynSent, self.mac, iss, Seq(0));
        conn.snd_nxt = iss.wrapping_add(1);
        let hdr = TcpHeader {
            src_port: local_port,
            dst_port: remote_port,
            seq: iss,
            ack: Seq(0),
            data_offset: 5,
            flags: TcpFlags::SYN,
            window: 65535,
            checksum: 0,
        };
        let mut seg = hdr.serialize();
        let csum = tcp_checksum(self.ip, remote_ip, &seg);
        seg[16..18].copy_from_slice(&csum.to_be_bytes());
        let frame = self.encapsulate_ipv4_tcp(self.ip, remote_ip, seg.clone());
        self.tx_queue.push_back(frame);
        conn.retransmit_queue.push_back(SentSegment {
            seq: iss, seq_len: 1, data: seg, sent_at: now, retransmit_count: 0,
        });
        self.connections.insert(tuple, conn);
        tuple
    }

    pub fn tcp_send(&mut self, tuple: SocketTuple, data: &[u8]) {
        if let Some(conn) = self.connections.get_mut(&tuple) {
            conn.send_buffer.extend(data);
        }
    }

    pub fn tcp_recv(&mut self, tuple: SocketTuple) -> Vec<u8> {
        if let Some(conn) = self.connections.get_mut(&tuple) {
            conn.receive_buffer.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    pub fn tcp_close(&mut self, tuple: SocketTuple, now: Instant) {
        if let Some(conn) = self.connections.get_mut(&tuple) {
            match conn.state {
                TcpState::Established => {
                    conn.state = TcpState::FinWait1;
                    conn.send_fin(&mut self.tx_queue, now);
                }
                TcpState::CloseWait => {
                    conn.state = TcpState::LastAck;
                    conn.send_fin(&mut self.tx_queue, now);
                }
                _ => {}
            }
        }
    }

    pub fn active_connections(&self) -> Vec<SocketTuple> {
        self.connections.keys().copied().collect()
    }

    pub fn connection_state(&self, tuple: SocketTuple) -> Option<TcpState> {
        self.connections.get(&tuple).map(|c| c.state)
    }

    pub fn on_packet(&mut self, frame: &[u8], now: Instant) {
        if frame.len() < 14 { return; }
        let dst_mac = MacAddress(frame[0..6].try_into().unwrap());
        let src_mac = MacAddress(frame[6..12].try_into().unwrap());
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);

        if dst_mac != self.mac && dst_mac != MacAddress([0xff; 6]) {
            return;
        }

        match ethertype {
            0x0806 => self.handle_arp(frame, src_mac),
            0x0800 => self.handle_ipv4(frame, src_mac, now),
            _ => {}
        }
    }

    pub fn on_tick(&mut self, now: Instant) {
        let tuples: Vec<_> = self.connections.keys().copied().collect();
        for tuple in tuples {
            let conn = self.connections.get_mut(&tuple).unwrap();
            conn.handle_timers(now, &mut self.tx_queue);
            conn.flush_send_buffer(&mut self.tx_queue, now);
            if conn.state == TcpState::Closed {
                self.connections.remove(&tuple);
            }
        }
    }

    fn handle_arp(&mut self, frame: &[u8], src_mac: MacAddress) {
        if frame.len() < 42 { return; }
        let target_ip = Ipv4Address(frame[38..42].try_into().unwrap());
        if target_ip != self.ip { return; }
        let sender_ip = Ipv4Address(frame[28..32].try_into().unwrap());
        self.arp_cache.insert(sender_ip, src_mac);
        let mut reply = vec![0u8; 42];
        reply[0..6].copy_from_slice(&src_mac.0);
        reply[6..12].copy_from_slice(&self.mac.0);
        reply[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        reply[14..16].copy_from_slice(&1u16.to_be_bytes());
        reply[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        reply[18] = 6;
        reply[19] = 4;
        reply[20..22].copy_from_slice(&2u16.to_be_bytes());
        reply[22..28].copy_from_slice(&self.mac.0);
        reply[28..32].copy_from_slice(&self.ip.0);
        reply[32..38].copy_from_slice(&src_mac.0);
        reply[38..42].copy_from_slice(&sender_ip.0);
        self.tx_queue.push_back(reply);
    }

    fn handle_ipv4(&mut self, frame: &[u8], src_mac: MacAddress, now: Instant) {
        if frame.len() < 34 { return; }
        let ihl = (frame[14] & 0x0F) as usize * 4;
        let total_len = u16::from_be_bytes([frame[16], frame[17]]) as usize;
        let protocol = frame[14 + 9];
        let src_ip = Ipv4Address(frame[14 + 12..14 + 16].try_into().unwrap());
        let dst_ip = Ipv4Address(frame[14 + 16..14 + 20].try_into().unwrap());
        if dst_ip != self.ip { return; }
        if checksum(&frame[14..14 + ihl]) != 0 { return; }
        self.arp_cache.insert(src_ip, src_mac);
        if protocol == 6 {
            let tcp_start = 14 + ihl;
            let tcp_end = 14 + total_len;
            if frame.len() < tcp_end { return; }
            self.handle_tcp(src_ip, dst_ip, &frame[tcp_start..tcp_end], now);
        }
    }

    fn handle_tcp(&mut self, src_ip: Ipv4Address, dst_ip: Ipv4Address, tcp_segment: &[u8], now: Instant) {
        if tcp_checksum(src_ip, dst_ip, tcp_segment) != 0 { return; }
        let (hdr, payload) = match TcpHeader::parse(tcp_segment) {
            Some(res) => res,
            None => return,
        };
        let tuple = SocketTuple {
            local_ip: dst_ip, local_port: hdr.dst_port,
            remote_ip: src_ip, remote_port: hdr.src_port,
        };
        if let Some(conn) = self.connections.get_mut(&tuple) {
            conn.process_segment(hdr, payload, &mut self.tx_queue, now);
        } else if let Some(&TcpState::Listen) = self.listening_ports.get(&hdr.dst_port) {
            if hdr.flags.contains(TcpFlags::SYN) && !hdr.flags.contains(TcpFlags::ACK) {
                let iss = self.generate_isn();
                let irs = hdr.seq;
                let mut conn = TcpConnection::new(tuple, TcpState::SynReceived, self.mac, iss, irs);
                conn.snd_nxt = iss.wrapping_add(1);
                conn.rcv_nxt = irs.wrapping_add(1);
                conn.snd_wnd = hdr.window;
                conn.send_syn_ack(&mut self.tx_queue, now);
                self.connections.insert(tuple, conn);
            }
        } else {
            self.send_rst(dst_ip, src_ip, hdr);
        }
    }

    pub fn send_arp_request(&mut self, target_ip: Ipv4Address) {
        let mut req = vec![0u8; 42];
        req[0..6].copy_from_slice(&[0xff; 6]);
        req[6..12].copy_from_slice(&self.mac.0);
        req[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        req[14..16].copy_from_slice(&1u16.to_be_bytes());
        req[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        req[18] = 6;
        req[19] = 4;
        req[20..22].copy_from_slice(&1u16.to_be_bytes());
        req[22..28].copy_from_slice(&self.mac.0);
        req[28..32].copy_from_slice(&self.ip.0);
        req[32..38].copy_from_slice(&[0x00; 6]);
        req[38..42].copy_from_slice(&target_ip.0);
        self.tx_queue.push_back(req);
    }

    pub fn resolve_and_populate_dst_mac(&mut self, packet: &mut [u8]) {
        if packet.len() >= 34 {
            let ethertype = u16::from_be_bytes([packet[12], packet[13]]);
            if ethertype == 0x0800 {
                let dst_ip = Ipv4Address(packet[30..34].try_into().unwrap());
                if let Some(mac) = self.arp_cache.get(&dst_ip) {
                    packet[0..6].copy_from_slice(&mac.0);
                } else {
                    self.send_arp_request(dst_ip);
                }
            }
        }
    }

    fn send_rst(&mut self, local_ip: Ipv4Address, remote_ip: Ipv4Address, bad_hdr: TcpHeader) {
        if bad_hdr.flags.contains(TcpFlags::RST) { return; }
        let rst_seq = if bad_hdr.flags.contains(TcpFlags::ACK) { bad_hdr.ack } else { Seq(0) };
        let rst_ack = if !bad_hdr.flags.contains(TcpFlags::ACK) {
            bad_hdr.seq.wrapping_add(if bad_hdr.flags.contains(TcpFlags::SYN) { 1 } else { 0 })
        } else { Seq(0) };
        let mut rst_flags = TcpFlags::RST;
        if !bad_hdr.flags.contains(TcpFlags::ACK) { rst_flags.insert(TcpFlags::ACK); }
        let reply_hdr = TcpHeader {
            src_port: bad_hdr.dst_port, dst_port: bad_hdr.src_port,
            seq: rst_seq, ack: rst_ack, data_offset: 5,
            flags: rst_flags, window: 0, checksum: 0,
        };
        let mut reply_bytes = reply_hdr.serialize();
        let csum = tcp_checksum(local_ip, remote_ip, &reply_bytes);
        reply_bytes[16..18].copy_from_slice(&csum.to_be_bytes());
        let frame = self.encapsulate_ipv4_tcp(local_ip, remote_ip, reply_bytes);
        self.tx_queue.push_back(frame);
    }

    fn encapsulate_ipv4_tcp(&mut self, src_ip: Ipv4Address, dst_ip: Ipv4Address, tcp_bytes: Vec<u8>) -> Vec<u8> {
        let mut frame = vec![0u8; 14 + 20 + tcp_bytes.len()];
        let dst_mac = self.arp_cache.get(&dst_ip).cloned().unwrap_or(MacAddress([0xff; 6]));
        frame[0..6].copy_from_slice(&dst_mac.0);
        frame[6..12].copy_from_slice(&self.mac.0);
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45;
        frame[15] = 0x00;
        let total_len = (20 + tcp_bytes.len()) as u16;
        frame[16..18].copy_from_slice(&total_len.to_be_bytes());
        frame[18..20].copy_from_slice(&0u16.to_be_bytes());
        frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
        frame[22] = 64;
        frame[23] = 6;
        frame[24..26].copy_from_slice(&0u16.to_be_bytes());
        frame[26..30].copy_from_slice(&src_ip.0);
        frame[30..34].copy_from_slice(&dst_ip.0);
        let ip_csum = checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&ip_csum.to_be_bytes());
        frame[34..].copy_from_slice(&tcp_bytes);
        frame
    }
}

impl TcpConnection {
    pub fn new(tuple: SocketTuple, state: TcpState, local_mac: MacAddress, iss: Seq, irs: Seq) -> Self {
        Self {
            tuple, state, local_mac,
            snd_una: iss, snd_nxt: iss, snd_wnd: 65535, iss,
            rcv_nxt: irs, rcv_wnd: 65535, irs,
            retransmit_queue: VecDeque::new(),
            out_of_order: BTreeMap::new(),
            send_buffer: VecDeque::new(),
            receive_buffer: VecDeque::new(),
            cwnd: 1460, ssthresh: 65535, dup_ack_count: 0,
            srtt: None, rttvar: Duration::from_millis(100),
            rto: Duration::from_millis(200),
            time_wait_expiry: None,
            mss: 1460,
        }
    }

    fn send_syn_ack(&mut self, tx_queue: &mut VecDeque<Vec<u8>>, now: Instant) {
        let hdr = TcpHeader {
            src_port: self.tuple.local_port, dst_port: self.tuple.remote_port,
            seq: self.iss, ack: self.rcv_nxt, data_offset: 5,
            flags: TcpFlags::SYN | TcpFlags::ACK, window: self.rcv_wnd, checksum: 0,
        };
        let mut seg = hdr.serialize();
        let csum = tcp_checksum(self.tuple.local_ip, self.tuple.remote_ip, &seg);
        seg[16..18].copy_from_slice(&csum.to_be_bytes());
        tx_queue.push_back(self.encapsulate_ipv4_tcp(seg.clone()));
        self.retransmit_queue.push_back(SentSegment {
            seq: self.iss, seq_len: 1, data: seg, sent_at: now, retransmit_count: 0,
        });
    }

    fn send_fin(&mut self, tx_queue: &mut VecDeque<Vec<u8>>, now: Instant) {
        let hdr = TcpHeader {
            src_port: self.tuple.local_port, dst_port: self.tuple.remote_port,
            seq: self.snd_nxt, ack: self.rcv_nxt, data_offset: 5,
            flags: TcpFlags::FIN | TcpFlags::ACK, window: self.rcv_wnd, checksum: 0,
        };
        let mut seg = hdr.serialize();
        let csum = tcp_checksum(self.tuple.local_ip, self.tuple.remote_ip, &seg);
        seg[16..18].copy_from_slice(&csum.to_be_bytes());
        tx_queue.push_back(self.encapsulate_ipv4_tcp(seg.clone()));
        self.retransmit_queue.push_back(SentSegment {
            seq: self.snd_nxt, seq_len: 1, data: seg, sent_at: now, retransmit_count: 0,
        });
        self.snd_nxt = self.snd_nxt.wrapping_add(1);
    }

    fn send_empty_ack(&mut self, tx_queue: &mut VecDeque<Vec<u8>>) {
        let hdr = TcpHeader {
            src_port: self.tuple.local_port, dst_port: self.tuple.remote_port,
            seq: self.snd_nxt, ack: self.rcv_nxt, data_offset: 5,
            flags: TcpFlags::ACK, window: self.rcv_wnd, checksum: 0,
        };
        let mut seg = hdr.serialize();
        let csum = tcp_checksum(self.tuple.local_ip, self.tuple.remote_ip, &seg);
        seg[16..18].copy_from_slice(&csum.to_be_bytes());
        tx_queue.push_back(self.encapsulate_ipv4_tcp(seg));
    }

    fn drain_retransmit_queue(&mut self, ack: Seq, now: Instant) {
        let mut rtt_sample = None;
        while let Some(front) = self.retransmit_queue.front() {
            let seg_end = front.seq.wrapping_add(front.seq_len);
            if seg_end.lte(ack) {
                let seg = self.retransmit_queue.pop_front().unwrap();
                if seg.retransmit_count == 0 && rtt_sample.is_none() {
                    rtt_sample = Some(now.duration_since(seg.sent_at));
                }
            } else {
                break;
            }
        }
        if let Some(rtt) = rtt_sample {
            self.update_rtt(rtt);
        }
    }

    fn compute_rto(&self) -> Duration {
        match self.srtt {
            None => Duration::from_millis(200),
            Some(srtt) => (srtt + self.rttvar * 4).max(Duration::from_millis(100)),
        }
    }

    fn update_rtt(&mut self, rtt: Duration) {
        match self.srtt {
            None => {
                self.srtt = Some(rtt);
                self.rttvar = rtt / 2;
            }
            Some(srtt) => {
                let diff = if rtt > srtt { rtt - srtt } else { srtt - rtt };
                self.rttvar = self.rttvar.mul_f64(0.75) + diff.mul_f64(0.25);
                self.srtt = Some(srtt.mul_f64(0.875) + rtt.mul_f64(0.125));
            }
        }
        self.rto = self.compute_rto();
    }

    pub fn flush_send_buffer(&mut self, tx_queue: &mut VecDeque<Vec<u8>>, now: Instant) {
        loop {
            if self.send_buffer.is_empty() { break; }
            let in_flight = self.snd_nxt.wrapping_sub(self.snd_una);
            let window = (self.cwnd.min(self.snd_wnd as u32)).saturating_sub(in_flight);
            if window == 0 { break; }
            let chunk_len = (window as usize).min(self.mss as usize).min(self.send_buffer.len());
            if chunk_len == 0 { break; }
            let chunk: Vec<u8> = self.send_buffer.drain(..chunk_len).collect();
            let hdr = TcpHeader {
                src_port: self.tuple.local_port, dst_port: self.tuple.remote_port,
                seq: self.snd_nxt, ack: self.rcv_nxt, data_offset: 5,
                flags: TcpFlags::ACK | TcpFlags::PSH, window: self.rcv_wnd, checksum: 0,
            };
            let mut seg = hdr.serialize();
            seg.extend_from_slice(&chunk);
            let csum = tcp_checksum(self.tuple.local_ip, self.tuple.remote_ip, &seg);
            seg[16..18].copy_from_slice(&csum.to_be_bytes());
            tx_queue.push_back(self.encapsulate_ipv4_tcp(seg.clone()));
            self.retransmit_queue.push_back(SentSegment {
                seq: self.snd_nxt, seq_len: chunk.len() as u32,
                data: seg, sent_at: now, retransmit_count: 0,
            });
            self.snd_nxt = self.snd_nxt.wrapping_add(chunk.len() as u32);
        }
    }

    pub fn process_segment(&mut self, hdr: TcpHeader, payload: &[u8], tx_queue: &mut VecDeque<Vec<u8>>, now: Instant) {
        if hdr.flags.contains(TcpFlags::RST) {
            self.state = TcpState::Closed;
            return;
        }

        match self.state {
            TcpState::SynSent => {
                if hdr.flags.contains(TcpFlags::SYN) && hdr.flags.contains(TcpFlags::ACK) {
                    if hdr.ack == self.snd_nxt {
                        self.irs = hdr.seq;
                        self.rcv_nxt = hdr.seq.wrapping_add(1);
                        self.snd_una = hdr.ack;
                        self.snd_wnd = hdr.window;
                        self.retransmit_queue.clear();
                        self.state = TcpState::Established;
                        self.send_empty_ack(tx_queue);
                    }
                } else if hdr.flags.contains(TcpFlags::SYN) {
                    self.irs = hdr.seq;
                    self.rcv_nxt = hdr.seq.wrapping_add(1);
                    self.state = TcpState::SynReceived;
                    self.send_syn_ack(tx_queue, now);
                }
            }
            TcpState::SynReceived => {
                if hdr.flags.contains(TcpFlags::ACK) && hdr.ack == self.snd_nxt {
                    self.state = TcpState::Established;
                    self.snd_una = hdr.ack;
                    self.snd_wnd = hdr.window;
                    self.retransmit_queue.clear();
                }
            }
            TcpState::Established => {
                self.process_ack(&hdr, tx_queue, now);
                if payload.len() > 0 {
                    if hdr.seq == self.rcv_nxt {
                        self.receive_buffer.extend(payload);
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(payload.len() as u32);
                        while let Some(stored) = self.out_of_order.remove(&self.rcv_nxt.0) {
                            let len = stored.len();
                            self.receive_buffer.extend(&stored);
                            self.rcv_nxt = self.rcv_nxt.wrapping_add(len as u32);
                        }
                        self.send_empty_ack(tx_queue);
                    } else if self.rcv_nxt.lt(hdr.seq) {
                        self.out_of_order.insert(hdr.seq.0, payload.to_vec());
                        self.send_empty_ack(tx_queue);
                    } else {
                        self.send_empty_ack(tx_queue);
                    }
                }
                if hdr.flags.contains(TcpFlags::FIN) {
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    self.state = TcpState::CloseWait;
                    self.send_empty_ack(tx_queue);
                }
            }
            TcpState::FinWait1 => {
                self.process_ack(&hdr, tx_queue, now);
                if hdr.flags.contains(TcpFlags::FIN) {
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    self.send_empty_ack(tx_queue);
                    if self.snd_una == self.snd_nxt {
                        self.state = TcpState::TimeWait;
                        self.time_wait_expiry = Some(now + Duration::from_secs(2));
                    } else {
                        self.state = TcpState::Closing;
                    }
                } else if self.snd_una == self.snd_nxt {
                    self.state = TcpState::FinWait2;
                }
            }
            TcpState::FinWait2 => {
                self.process_ack(&hdr, tx_queue, now);
                if payload.len() > 0 {
                    if hdr.seq == self.rcv_nxt {
                        self.receive_buffer.extend(payload);
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(payload.len() as u32);
                        self.send_empty_ack(tx_queue);
                    }
                }
                if hdr.flags.contains(TcpFlags::FIN) {
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    self.send_empty_ack(tx_queue);
                    self.state = TcpState::TimeWait;
                    self.time_wait_expiry = Some(now + Duration::from_secs(2));
                }
            }
            TcpState::Closing => {
                self.process_ack(&hdr, tx_queue, now);
                if self.snd_una == self.snd_nxt {
                    self.state = TcpState::TimeWait;
                    self.time_wait_expiry = Some(now + Duration::from_secs(2));
                }
            }
            TcpState::CloseWait => {
                self.process_ack(&hdr, tx_queue, now);
            }
            TcpState::LastAck => {
                if hdr.flags.contains(TcpFlags::ACK) && hdr.ack == self.snd_nxt {
                    self.state = TcpState::Closed;
                }
            }
            TcpState::TimeWait => {
                if hdr.flags.contains(TcpFlags::FIN) {
                    self.send_empty_ack(tx_queue);
                    self.time_wait_expiry = Some(now + Duration::from_secs(2));
                }
            }
            _ => {}
        }
    }

    fn process_ack(&mut self, hdr: &TcpHeader, tx_queue: &mut VecDeque<Vec<u8>>, now: Instant) {
        if !hdr.flags.contains(TcpFlags::ACK) { return; }
        if self.snd_una.lt(hdr.ack) && hdr.ack.lte(self.snd_nxt) {
            self.dup_ack_count = 0;
            self.snd_una = hdr.ack;
            self.snd_wnd = hdr.window;
            self.drain_retransmit_queue(hdr.ack, now);
            self.rto = self.compute_rto();
            if self.cwnd < self.ssthresh {
                self.cwnd = self.cwnd.wrapping_add(self.mss as u32);
            } else {
                self.cwnd = self.cwnd.wrapping_add((self.mss as u32 * self.mss as u32) / self.cwnd);
            }
        } else if hdr.ack == self.snd_una && !self.retransmit_queue.is_empty() {
            self.dup_ack_count = self.dup_ack_count.saturating_add(1);
            if self.dup_ack_count == 3 {
                self.ssthresh = (self.cwnd / 2).max(2 * self.mss as u32);
                self.cwnd = self.ssthresh + 3 * self.mss as u32;
                if let Some(seg) = self.retransmit_queue.front() {
                    let mut packet = seg.data.clone();
                    packet[16..18].copy_from_slice(&[0, 0]);
                    let csum = tcp_checksum(self.tuple.local_ip, self.tuple.remote_ip, &packet);
                    packet[16..18].copy_from_slice(&csum.to_be_bytes());
                    let frame = self.encapsulate_ipv4_tcp(packet);
                    tx_queue.push_back(frame);
                }
            }
        }
    }

    pub fn handle_timers(&mut self, now: Instant, tx_queue: &mut VecDeque<Vec<u8>>) {
        if let Some(expiry) = self.time_wait_expiry {
            if now >= expiry {
                self.state = TcpState::Closed;
                return;
            }
        }
        while let Some(front) = self.retransmit_queue.front() {
            if front.seq.wrapping_add(front.seq_len).lte(self.snd_una) {
                self.retransmit_queue.pop_front();
            } else {
                break;
            }
        }
        if let Some(seg) = self.retransmit_queue.front() {
            if now.duration_since(seg.sent_at) >= self.rto {
                self.ssthresh = (self.cwnd / 2).max(2 * self.mss as u32);
                self.cwnd = self.mss as u32;
                self.rto = (self.rto * 2).min(Duration::from_secs(60));
                let mut packet = seg.data.clone();
                packet[16..18].copy_from_slice(&[0, 0]);
                let csum = tcp_checksum(self.tuple.local_ip, self.tuple.remote_ip, &packet);
                packet[16..18].copy_from_slice(&csum.to_be_bytes());
                tx_queue.push_back(self.encapsulate_ipv4_tcp(packet));
                let seg_mut = self.retransmit_queue.front_mut().unwrap();
                seg_mut.sent_at = now;
                seg_mut.retransmit_count += 1;
            }
        }
    }

    fn encapsulate_ipv4_tcp(&mut self, tcp_bytes: Vec<u8>) -> Vec<u8> {
        let mut frame = vec![0u8; 14 + 20 + tcp_bytes.len()];
        frame[0..6].copy_from_slice(&[0x00; 6]);
        frame[6..12].copy_from_slice(&self.local_mac.0);
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45;
        frame[15] = 0x00;
        let total_len = (20 + tcp_bytes.len()) as u16;
        frame[16..18].copy_from_slice(&total_len.to_be_bytes());
        frame[18..20].copy_from_slice(&0u16.to_be_bytes());
        frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
        frame[22] = 64;
        frame[23] = 6;
        frame[24..26].copy_from_slice(&0u16.to_be_bytes());
        frame[26..30].copy_from_slice(&self.tuple.local_ip.0);
        frame[30..34].copy_from_slice(&self.tuple.remote_ip.0);
        let ip_csum = checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&ip_csum.to_be_bytes());
        frame[34..].copy_from_slice(&tcp_bytes);
        frame
    }
}
