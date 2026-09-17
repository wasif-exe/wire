use std::time::Instant;
use wire_core::{Stack, MacAddress, Ipv4Address};
use wire_tap::TapDevice;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let mut tap = TapDevice::new("tap0")?;
    let local_mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let local_ip = Ipv4Address([192, 168, 99, 2]);
    let mut stack = Stack::new(local_mac, local_ip);
    stack.listen(8080);
    let mut buf = [0u8; 2048];
    let mut last_tick = Instant::now();
    loop {
        let now = Instant::now();
        if now.duration_since(last_tick) >= std::time::Duration::from_millis(10) {
            stack.on_tick(now);
            last_tick = now;
        }
        let n = tap.read(&mut buf)?;
        stack.on_packet(&buf[..n], now);
        for tuple in stack.active_connections() {
            let data = stack.tcp_recv(tuple);
            if !data.is_empty() {
                stack.tcp_send(tuple, &data);
            }
            if let Some(wire_core::TcpState::CloseWait) = stack.connection_state(tuple) {
                stack.tcp_close(tuple, now);
            }
        }
        while let Some(mut tx_packet) = stack.tx_queue.pop_front() {
            if tx_packet.len() >= 34 {
                let dst_ip = Ipv4Address(tx_packet[30..34].try_into().unwrap());
                if let Some(mac) = stack.arp_cache.get(&dst_ip) {
                    tx_packet[0..6].copy_from_slice(&mac.0);
                }
            }
            tap.write(&tx_packet)?;
        }
    }
}
