use std::sync::Arc;
use std::thread;
use std::sync::atomic::Ordering;
use wire_core::{MacAddress, Ipv4Address};
use wire_core::shard::StackShard;
use wire_core::cacheline::{WorkerStats, cpu_relax};
use wire_xdp::{XdpSocket, XdpBpfModule, pin_thread_to_core, BATCH_SIZE};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PollMode {
    Busy,
    Hybrid,
    Sleep,
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = std::env::args().collect();
    let poll_mode = if args.iter().any(|a| a == "--poll-mode=sleep") {
        PollMode::Sleep
    } else if args.iter().any(|a| a == "--poll-mode=hybrid") {
        PollMode::Hybrid
    } else {
        PollMode::Busy
    };
    let bpf = Arc::new(XdpBpfModule::load_and_attach("veth-wire", &[])?);
    let cores = vec![0, 1];
    let num_shards = cores.len();
    let mut handles = Vec::new();
    println!(" Initializing Sharded Dataplane (Poll Mode: {:?}) across cores: {:?}", poll_mode, cores);
    for (queue_id, &core_id) in cores.iter().enumerate() {
        let bpf = Arc::clone(&bpf);
        let handle = thread::spawn(move || -> anyhow::Result<()> {
            pin_thread_to_core(core_id)?;
            println!(" Worker thread pinned to CPU core {}", core_id);
            let mut xsk = XdpSocket::new("veth-wire", queue_id as u32)?;
            bpf.update_xsk_map(queue_id as u32, xsk.fd())?;
            if xsk.is_hugepage() {
                println!(" Core {} running on 2MB HugePages", core_id);
            }
            let local_mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
            let local_ip = Ipv4Address([192, 168, 99, 2]);
            let gateway_ip = Ipv4Address([192, 168, 99, 1]);
            let shard = StackShard::new(queue_id, local_mac, local_ip, gateway_ip, 16384);
            let stats = WorkerStats::default();
            let mut batch_packets: Vec<Vec<u8>> = (0..BATCH_SIZE).map(|_| Vec::with_capacity(2048)).collect();
            let mut idle_spins = 0u64;
            loop {
                let n_reaped = xsk.poll_read_batch(&mut batch_packets);
                if n_reaped > 0 {
                    idle_spins = 0;
                    stats.rx_packets.fetch_add(n_reaped as u64, Ordering::Relaxed);
                    for i in 0..n_reaped {
                        let pkt = &batch_packets[i];
                        if pkt.len() < 14 { continue; }
                        let ethertype = u16::from_be_bytes([pkt[12], pkt[13]]);
                        if ethertype == 0x0800 && pkt.len() >= 34 {
                            let src_ip = Ipv4Address(pkt[26..30].try_into().unwrap());
                            let dst_ip = Ipv4Address(pkt[30..34].try_into().unwrap());
                            if pkt[23] == 6 && pkt.len() >= 54 {
                                let src_port = u16::from_be_bytes([pkt[34], pkt[35]]);
                                let dst_port = u16::from_be_bytes([pkt[36], pkt[37]]);
                                let tuple = wire_core::SocketTuple {
                                    local_ip: dst_ip,
                                    local_port: dst_port,
                                    remote_ip: src_ip,
                                    remote_port: src_port,
                                };
                                if !shard.handles_flow(&tuple, num_shards) {
                                    continue;
                                }
                            }
                        }
                        let mut tx_packet = pkt.clone();
                        tx_packet[0..6].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
                        tx_packet[6..12].copy_from_slice(&local_mac.0);
                        let _ = xsk.write_async(&tx_packet);
                    }
                } else {
                    idle_spins += 1;
                    stats.poll_spins.fetch_add(1, Ordering::Relaxed);
                    match poll_mode {
                        PollMode::Busy => {
                            cpu_relax();
                        }
                        PollMode::Hybrid => {
                            if idle_spins < 1000 {
                                cpu_relax();
                            } else {
                                std::thread::sleep(std::time::Duration::from_micros(10));
                            }
                        }
                        PollMode::Sleep => {
                            std::thread::sleep(std::time::Duration::from_micros(50));
                        }
                    }
                }
            }
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.join().unwrap()?;
    }
    Ok(())
}
