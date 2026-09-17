use std::time::{Instant, Duration};
use wire_core::{Stack, MacAddress, Ipv4Address, TcpState};
use wire_tap::TapDevice;

fn main() -> anyhow::Result<()> {
    let mut tap = TapDevice::new("tap0")?;
    let local_mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let local_ip = Ipv4Address([192, 168, 99, 2]);
    let remote_ip = Ipv4Address([192, 168, 99, 1]);
    let remote_port = 8000;
    let local_port = 49152;

    let mut stack = Stack::new(local_mac, local_ip);
    let mut buf = [0u8; 2048];
    let mut last_tick = Instant::now();

    println!("⚡ Starting Active Connection to 192.168.99.1:8000");

    let now = Instant::now();
    let tuple = stack.tcp_connect(remote_ip, remote_port, local_port, now);

    let mut request_sent = false;
    let mut response_buffer = Vec::new();

    loop {
        let now = Instant::now();
        if now.duration_since(last_tick) >= Duration::from_millis(10) {
            stack.on_tick(now);
            last_tick = now;
        }

        let n = tap.read(&mut buf)?;
        stack.on_packet(&buf[..n], now);

        if let Some(state) = stack.connection_state(tuple) {
            if state == TcpState::Established && !request_sent {
                println!("✅ Connection established! Sending HTTP GET...");
                let http_req = b"GET /index.html HTTP/1.1\r\nHost: 192.168.99.1\r\nConnection: close\r\n\r\n";
                stack.tcp_send(tuple, http_req);
                request_sent = true;
            }

            let data = stack.tcp_recv(tuple);
            if !data.is_empty() {
                response_buffer.extend(&data);
            }

            if state == TcpState::CloseWait {
                println!("📥 Server closed connection (FIN received). Closing our side.");
                stack.tcp_close(tuple, now);
            }
        } else {
            if request_sent {
                println!("🛑 Connection closed cleanly by stack.");
                break;
            }
        }

        while let Some(mut tx_packet) = stack.tx_queue.pop_front() {
            stack.resolve_and_populate_dst_mac(&mut tx_packet);
            tap.write(&tx_packet)?;
        }
    }

    println!("\n=== HTTP RESPONSE RECEIVED ===");
    println!("{}", String::from_utf8_lossy(&response_buffer));
    println!("==============================");

    Ok(())
}
