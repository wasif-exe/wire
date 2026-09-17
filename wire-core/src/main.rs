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

        while let Some(tx_packet) = stack.tx_queue.pop_front() {
            let mut packet = tx_packet;
            let dst_ip = Ipv4Address(packet[30..34].try_into().unwrap());
            if let Some(mac) = stack.arp_cache.get(&dst_ip) {
                packet[0..6].copy_from_slice(&mac.0);
            }
            tap.write(&packet)?;
        }
    }
}