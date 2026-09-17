use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::time::{Duration, Instant};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use wire_core::{Stack, MacAddress, Ipv4Address, TcpState};

struct ScheduledPacket {
    deliver_at: Instant,
    data: Vec<u8>,
    direction: Direction,
}

impl PartialEq for ScheduledPacket {
    fn eq(&self, other: &Self) -> bool {
        self.deliver_at == other.deliver_at
    }
}

impl Eq for ScheduledPacket {}

impl PartialOrd for ScheduledPacket {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledPacket {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deliver_at.cmp(&other.deliver_at)
    }
}

#[derive(Clone, Copy)]
enum Direction {
    AtoB,
    BtoA,
}

pub struct SimConfig {
    pub loss_rate: f64,
    pub dup_rate: f64,
    pub min_delay: Duration,
    pub max_delay: Duration,
    pub data_size: usize,
    pub max_ticks: u64,
}

pub struct SimResult {
    pub seed: u64,
    pub bytes_sent: usize,
    pub bytes_received: usize,
    pub data_match: bool,
    pub connection_closed: bool,
    pub ticks: u64,
}

pub fn run_simulation(seed: u64, config: &SimConfig) -> SimResult {
    let mut rng = SmallRng::seed_from_u64(seed);
    let base = Instant::now();
    let mut now = base;

    let mac_a = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let mac_b = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
    let ip_a = Ipv4Address([10, 0, 0, 1]);
    let ip_b = Ipv4Address([10, 0, 0, 2]);

    let mut stack_a = Stack::new(mac_a, ip_a);
    let mut stack_b = Stack::new(mac_b, ip_b);

    stack_a.arp_cache.insert(ip_b, mac_b);
    stack_b.arp_cache.insert(ip_a, mac_a);

    stack_a.listen(8080);
    let tuple_b = stack_b.tcp_connect(ip_a, 8080, 40000, now);

    let mut wire: BinaryHeap<Reverse<ScheduledPacket>> = BinaryHeap::new();

    let test_data: Vec<u8> = (0..config.data_size).map(|i| (i % 256) as u8).collect();
    let mut data_sent = false;
    let mut total_received: Vec<u8> = Vec::new();

    let mut ticks: u64 = 0;

    loop {
        ticks += 1;
        if ticks > config.max_ticks { break; }

        let mut next_tick = now + Duration::from_millis(1);

        if let Some(Reverse(pkt)) = wire.peek() {
            if pkt.deliver_at < next_tick {
                next_tick = pkt.deliver_at;
            }
        }

        now = next_tick;

        while let Some(Reverse(pkt)) = wire.peek() {
            if pkt.deliver_at > now { break; }
            let pkt = wire.pop().unwrap().0;
            match pkt.direction {
                Direction::AtoB => stack_b.on_packet(&pkt.data, now),
                Direction::BtoA => stack_a.on_packet(&pkt.data, now),
            }
        }

        stack_a.on_tick(now);
        stack_b.on_tick(now);

        if !data_sent {
            if let Some(TcpState::Established) = stack_b.connection_state(tuple_b) {
                stack_b.tcp_send(tuple_b, &test_data);
                data_sent = true;
            }
        }

        if let Some(&tuple_a) = stack_a.active_connections().first() {
            let received_a = stack_a.tcp_recv(tuple_a);
            if !received_a.is_empty() {
                stack_a.tcp_send(tuple_a, &received_a);
            }
            if let Some(TcpState::CloseWait) = stack_a.connection_state(tuple_a) {
                stack_a.tcp_close(tuple_a, now);
            }
        }

        let received_b = stack_b.tcp_recv(tuple_b);
        if !received_b.is_empty() {
            total_received.extend(&received_b);
        }

        if data_sent && total_received.len() >= test_data.len() {
            stack_b.tcp_close(tuple_b, now);
        }

        for mut frame in stack_a.tx_queue.drain(..) {
            if frame.len() >= 34 {
                let dst_ip = Ipv4Address(frame[30..34].try_into().unwrap());
                if let Some(mac) = stack_a.arp_cache.get(&dst_ip) {
                    frame[0..6].copy_from_slice(&mac.0);
                }
            }
            if rng.gen::<f64>() < config.loss_rate { continue; }
            
            let min_ns = config.min_delay.as_nanos() as u64;
            let max_ns = config.max_delay.as_nanos() as u64;
            let delay_ns = if min_ns >= max_ns { min_ns } else { rng.gen_range(min_ns..=max_ns) };
            let delay = Duration::from_nanos(delay_ns);

            wire.push(Reverse(ScheduledPacket {
                deliver_at: now + delay,
                data: frame.clone(),
                direction: Direction::AtoB,
            }));
            if rng.gen::<f64>() < config.dup_rate {
                wire.push(Reverse(ScheduledPacket {
                    deliver_at: now + delay + Duration::from_micros(rng.gen::<u64>() % 100),
                    data: frame,
                    direction: Direction::AtoB,
                }));
            }
        }

        for mut frame in stack_b.tx_queue.drain(..) {
            if frame.len() >= 34 {
                let dst_ip = Ipv4Address(frame[30..34].try_into().unwrap());
                if let Some(mac) = stack_b.arp_cache.get(&dst_ip) {
                    frame[0..6].copy_from_slice(&mac.0);
                }
            }
            if rng.gen::<f64>() < config.loss_rate { continue; }

            let min_ns = config.min_delay.as_nanos() as u64;
            let max_ns = config.max_delay.as_nanos() as u64;
            let delay_ns = if min_ns >= max_ns { min_ns } else { rng.gen_range(min_ns..=max_ns) };
            let delay = Duration::from_nanos(delay_ns);

            wire.push(Reverse(ScheduledPacket {
                deliver_at: now + delay,
                data: frame.clone(),
                direction: Direction::BtoA,
            }));
            if rng.gen::<f64>() < config.dup_rate {
                wire.push(Reverse(ScheduledPacket {
                    deliver_at: now + delay + Duration::from_micros(rng.gen::<u64>() % 100),
                    data: frame,
                    direction: Direction::BtoA,
                }));
            }
        }

        let a_closed = stack_a.active_connections().is_empty();
        let b_closed = stack_b.connection_state(tuple_b).map_or(true, |s| s == TcpState::Closed);
        if a_closed && b_closed && data_sent { break; }
    }

    let data_match = total_received.len() >= test_data.len()
        && total_received[..test_data.len()] == test_data[..];

    SimResult {
        seed,
        bytes_sent: test_data.len(),
        bytes_received: total_received.len().min(test_data.len()),
        data_match,
        connection_closed: stack_a.active_connections().is_empty(),
        ticks,
    }
}
