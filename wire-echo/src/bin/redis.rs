use std::collections::HashMap;
use std::time::{Duration, Instant};
use wire_core::{Ipv4Address, MacAddress, SocketTuple, Stack, TcpState};
use wire_core::kv::ShardKvStore;
use wire_core::resp::RespParser;
use wire_tap::TapDevice;
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let mut tap = TapDevice::new("tap0")?;
    let local_mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let local_ip = Ipv4Address([192, 168, 99, 2]);
    let gateway_ip = Ipv4Address([192, 168, 99, 1]);
    let mut stack = Stack::new(local_mac, local_ip, gateway_ip);
    stack.listen(6379);
    let mut kv_store = ShardKvStore::new();
    let mut rx_buffers: HashMap<SocketTuple, Vec<u8>> = HashMap::new();
    let mut buf = [0u8; 2048];
    let mut last_tick = Instant::now();
    println!(" Wire-Redis Server listening on 192.168.99.2:6379 (Zero-Syscall Dataplane)");
    loop {
        let now = Instant::now();
        if now.duration_since(last_tick) >= Duration::from_millis(10) {
            stack.on_tick(now);
            last_tick = now;
        }
        let mut read_any = false;
        while let Some(n) = tap.poll_read(&mut buf)? {
            stack.on_packet(&buf[..n], now);
            read_any = true;
        }
        for tuple in stack.active_connections() {
            let data = stack.tcp_recv(tuple);
            if !data.is_empty() {
                let conn_buf = rx_buffers.entry(tuple).or_default();
                conn_buf.extend_from_slice(&data);
                let mut resp_out = Vec::with_capacity(512);
                let consumed = {
                    let (cmds, consumed) = RespParser::parse_commands(conn_buf);
                    for cmd in &cmds {
                        kv_store.execute(cmd, &mut resp_out);
                    }
                    consumed
                };
                if consumed > 0 {
                    conn_buf.drain(..consumed);
                }
                if !resp_out.is_empty() {
                    stack.tcp_send(tuple, &resp_out);
                    stack.flush(now);
                }
            }
            if let Some(TcpState::CloseWait) = stack.connection_state(tuple) {
                stack.tcp_close(tuple, now);
                rx_buffers.remove(&tuple);
            }
        }
        while let Some(mut tx_packet) = stack.tx_queue.pop_front() {
            stack.resolve_and_populate_dst_mac(&mut tx_packet);
            match tap.write_async(&tx_packet)? {
                Some(_) => {}
                None => {
                    stack.tx_queue.push_front(tx_packet);
                    break;
                }
            }
        }
        if !read_any {
            std::thread::sleep(Duration::from_micros(20));
        }
    }
}
