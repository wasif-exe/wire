use std::time::{Instant, Duration};
use std::thread;
use wire_core::{Stack, MacAddress, Ipv4Address, TcpHeader, TcpFlags, Seq, checksum, tcp_checksum};
use wire_core::shard::StackShard;
use wire_core::resp::RespParser;
use wire_core::kv::ShardKvStore;
use wire_core::probes::{self, STAGE_NAMES, NUM_STAGES};
use wire_sim::{SimConfig, run_simulation};
fn calibrate_tsc() -> f64 {
    let start_time = Instant::now();
    let start_tsc = probes::rdtsc();
    std::thread::sleep(Duration::from_millis(50));
    let elapsed = start_time.elapsed().as_secs_f64();
    let tsc_delta = probes::rdtsc().wrapping_sub(start_tsc) as f64;
    tsc_delta / elapsed / 1_000_000_000.0
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let profile_mode = args.iter().any(|a| a.starts_with("--profile"));
    let sample_rate = args.iter()
        .find(|a| a.starts_with("--profile=sample="))
        .and_then(|a| a.strip_prefix("--profile=sample="))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);
    probes::set_sample_rate(sample_rate);
    println!("\x1b[1;36m================================================================================\x1b[0m");
    println!("\x1b[1;37m                       WIRE V4 PIPELINE PERFORMANCE SUITE                       \x1b[0m");
    println!("\x1b[1;36m================================================================================\x1b[0m\n");
    let tsc_ghz = calibrate_tsc();
    println!("  TSC Clock Frequency:   \x1b[1;32m{:.3} GHz\x1b[0m", tsc_ghz);
    if profile_mode {
        println!("  Profiling Mode:        \x1b[1;33mActive (Sample 1/{})\x1b[0m", sample_rate);
    }
    println!();
    bench_checksum();
    println!();
    bench_fsm_throughput(tsc_ghz);
    println!();
    bench_multi_core_sharding();
    println!();
    bench_redis_l7_engine();
    println!();
    bench_bbr_pacing_evaluation();
    println!();
    bench_simulation_chaos();
    println!();
    if profile_mode {
        print_profile_report(tsc_ghz, &args);
    }
    println!("\x1b[1;36m================================================================================\x1b[0m");
    println!("\x1b[1;32m                          ALL BENCHMARKS COMPLETED                              \x1b[0m");
    println!("\x1b[1;36m================================================================================\x1b[0m");
}
fn bench_checksum() {
    println!("\x1b[1;33m[1/6] Internet Checksum (RFC 1071) Computation Performance\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    let chunk_size = 1460;
    let iterations = 1_000_000;
    let data = vec![0x5au8; chunk_size];
    let total_bytes = chunk_size as u64 * iterations as u64;
    let start = Instant::now();
    let mut dummy_sum = 0u16;
    for _ in 0..iterations {
        dummy_sum = dummy_sum.wrapping_add(checksum(&data));
    }
    let elapsed = start.elapsed();
    std::hint::black_box(dummy_sum);
    let mb_processed = (total_bytes as f64) / (1024.0 * 1024.0);
    let gbps = (total_bytes as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000_000.0);
    let ns_per_op = elapsed.as_nanos() as f64 / iterations as f64;
    println!("  Payload Size:          {} bytes (MSS)", chunk_size);
    println!("  Iterations:            {}", iterations);
    println!("  Total Processed:       {:.2} MB", mb_processed);
    println!("  Elapsed Time:          {:.3} ms", elapsed.as_secs_f64() * 1000.0);
    println!("  Latency:               \x1b[1;32m{:.2} ns/chunk\x1b[0m", ns_per_op);
    println!("  Throughput:            \x1b[1;32m{:.2} Gbps\x1b[0m", gbps);
}
fn bench_fsm_throughput(tsc_ghz: f64) {
    println!("\x1b[1;33m[2/6] Single-Core Ingress Packet Throughput\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    let mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let ip = Ipv4Address([192, 168, 99, 2]);
    let gw = Ipv4Address([192, 168, 99, 1]);
    let mut stack = Stack::new(mac, ip, gw);
    stack.listen(8080);
    let client_mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
    let client_ip = Ipv4Address([192, 168, 99, 1]);
    stack.arp_cache.insert(client_ip, client_mac);
    let now = Instant::now();
    let syn_hdr = TcpHeader {
        src_port: 50000,
        dst_port: 8080,
        seq: Seq(100),
        ack: Seq(0),
        data_offset: 5,
        flags: TcpFlags::SYN,
        window: 65535,
        checksum: 0,
        options: Vec::new(),
    };
    let mut syn_bytes = syn_hdr.serialize();
    let csum = tcp_checksum(client_ip, ip, &syn_bytes);
    syn_bytes[16..18].copy_from_slice(&csum.to_be_bytes());
    let mut syn_frame = vec![0u8; 14 + 20 + syn_bytes.len()];
    syn_frame[0..6].copy_from_slice(&mac.0);
    syn_frame[6..12].copy_from_slice(&client_mac.0);
    syn_frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    syn_frame[14] = 0x45;
    let total_len = (20 + syn_bytes.len()) as u16;
    syn_frame[16..18].copy_from_slice(&total_len.to_be_bytes());
    syn_frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    syn_frame[22] = 64;
    syn_frame[23] = 6;
    syn_frame[26..30].copy_from_slice(&client_ip.0);
    syn_frame[30..34].copy_from_slice(&ip.0);
    let ip_csum = checksum(&syn_frame[14..34]);
    syn_frame[24..26].copy_from_slice(&ip_csum.to_be_bytes());
    syn_frame[34..].copy_from_slice(&syn_bytes);
    stack.on_packet(&syn_frame, now);
    stack.tx_queue.clear();
    let ack_hdr = TcpHeader {
        src_port: 50000,
        dst_port: 8080,
        seq: Seq(101),
        ack: Seq(21246687),
        data_offset: 5,
        flags: TcpFlags::ACK,
        window: 65535,
        checksum: 0,
        options: Vec::new(),
    };
    let mut ack_bytes = ack_hdr.serialize();
    let csum = tcp_checksum(client_ip, ip, &ack_bytes);
    ack_bytes[16..18].copy_from_slice(&csum.to_be_bytes());
    let mut ack_frame = syn_frame.clone();
    ack_frame[34..].copy_from_slice(&ack_bytes);
    stack.on_packet(&ack_frame, now);
    stack.tx_queue.clear();
    let payload = vec![0x42u8; 1024];
    let data_hdr = TcpHeader {
        src_port: 50000,
        dst_port: 8080,
        seq: Seq(101),
        ack: Seq(21246687),
        data_offset: 5,
        flags: TcpFlags::ACK | TcpFlags::PSH,
        window: 65535,
        checksum: 0,
        options: Vec::new(),
    };
    let mut data_seg = data_hdr.serialize();
    data_seg.extend_from_slice(&payload);
    let csum = tcp_checksum(client_ip, ip, &data_seg);
    data_seg[16..18].copy_from_slice(&csum.to_be_bytes());
    let mut data_frame = vec![0u8; 14 + 20 + data_seg.len()];
    data_frame[0..6].copy_from_slice(&mac.0);
    data_frame[6..12].copy_from_slice(&client_mac.0);
    data_frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    data_frame[14] = 0x45;
    let total_len = (20 + data_seg.len()) as u16;
    data_frame[16..18].copy_from_slice(&total_len.to_be_bytes());
    data_frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    data_frame[22] = 64;
    data_frame[23] = 6;
    data_frame[26..30].copy_from_slice(&client_ip.0);
    data_frame[30..34].copy_from_slice(&ip.0);
    let ip_csum = checksum(&data_frame[14..34]);
    data_frame[24..26].copy_from_slice(&ip_csum.to_be_bytes());
    data_frame[34..].copy_from_slice(&data_seg);
    let iterations = 200_000;
    probes::reset();
    let start = Instant::now();
    for _ in 0..iterations {
        stack.on_packet(&data_frame, now);
        stack.tx_queue.clear();
    }
    let elapsed = start.elapsed();
    let pps = (iterations as f64) / elapsed.as_secs_f64();
    let ns_per_pkt = elapsed.as_nanos() as f64 / iterations as f64;
    let cycles_per_pkt = ns_per_pkt * tsc_ghz;
    let mb_rate = (iterations as f64 * 1024.0) / (elapsed.as_secs_f64() * 1024.0 * 1024.0);
    println!("  Inbound Segment:       1024 bytes payload + L2/L3/L4 headers");
    println!("  Packets Processed:     {}", iterations);
    println!("  Ingress Latency:       \x1b[1;32m{:.2} ns/packet\x1b[0m ({:.1} cycles)", ns_per_pkt, cycles_per_pkt);
    println!("  Packet Ingress Rate:   \x1b[1;32m{:.2} Mpps\x1b[0m", pps / 1_000_000.0);
    println!("  Processing Rate:       \x1b[1;32m{:.2} MB/s\x1b[0m", mb_rate);
}
fn bench_multi_core_sharding() {
    println!("\x1b[1;33m[3/6] Multi-Core Shared-Nothing Sharding Throughput\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    let num_workers = 4;
    let iterations_per_worker = 100_000;
    let start = Instant::now();
    let mut handles = Vec::new();
    for shard_id in 0..num_workers {
        let handle = thread::spawn(move || {
            let local_mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
            let local_ip = Ipv4Address([192, 168, 99, 2]);
            let gateway_ip = Ipv4Address([192, 168, 99, 1]);
            let shard = StackShard::new(shard_id, local_mac, local_ip, gateway_ip, 1024);
            let dummy_tuple = wire_core::SocketTuple {
                local_ip,
                local_port: 8080,
                remote_ip: gateway_ip,
                remote_port: 50000 + shard_id as u16,
            };
            let mut match_count = 0u64;
            for _ in 0..iterations_per_worker {
                if shard.handles_flow(&dummy_tuple, num_workers) {
                    match_count += 1;
                }
            }
            match_count
        });
        handles.push(handle);
    }
    for h in handles {
        let _ = h.join().unwrap();
    }
    let elapsed = start.elapsed();
    let total_ops = num_workers * iterations_per_worker;
    let mpps = (total_ops as f64) / (elapsed.as_secs_f64() * 1_000_000.0);
    println!("  Worker Cores:          {} (Shared-Nothing)", num_workers);
    println!("  Total Classifications: {}", total_ops);
    println!("  Elapsed Time:          {:.3} ms", elapsed.as_secs_f64() * 1000.0);
    println!("  Aggregate Rate:        \x1b[1;32m{:.2} Mpps\x1b[0m", mpps);
}
fn bench_redis_l7_engine() {
    println!("\x1b[1;33m[4/6] L7 Redis RESP Pipelining & Key-Value Store Rate\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    let pipeline_raw = b"*3\r\n$3\r\nSET\r\n$3\r\nfoo\r\n$3\r\nbar\r\n*2\r\n$3\r\nGET\r\n$3\r\nfoo\r\n*2\r\n$6\r\nEXISTS\r\n$3\r\nfoo\r\n*2\r\n$3\r\nDEL\r\n$3\r\nfoo\r\n";
    let iterations = 100_000;
    let mut kv = ShardKvStore::new();
    let mut out = Vec::with_capacity(1024);
    let start = Instant::now();
    for _ in 0..iterations {
        let (cmds, _) = RespParser::parse_commands(pipeline_raw);
        for cmd in &cmds {
            kv.execute(cmd, &mut out);
        }
        out.clear();
    }
    let elapsed = start.elapsed();
    let total_cmds = iterations * 4;
    let ops_per_sec = (total_cmds as f64) / elapsed.as_secs_f64();
    let ns_per_cmd = elapsed.as_nanos() as f64 / total_cmds as f64;
    println!("  Pipelined Commands:    4 Commands per Request (SET, GET, EXISTS, DEL)");
    println!("  Total Executions:      {} operations", total_cmds);
    println!("  Elapsed Time:          {:.3} ms", elapsed.as_secs_f64() * 1000.0);
    println!("  Operation Latency:     \x1b[1;32m{:.2} ns/op\x1b[0m", ns_per_cmd);
    println!("  L7 Execution Rate:     \x1b[1;32m{:.2} Mops/sec\x1b[0m", ops_per_sec / 1_000_000.0);
}
fn bench_bbr_pacing_evaluation() {
    println!("\x1b[1;33m[5/6] Google BBR Pacing Engine Calculation Latency\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    let mac = MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    let ip = Ipv4Address([192, 168, 99, 2]);
    let gw = Ipv4Address([192, 168, 99, 1]);
    let mut stack = Stack::new(mac, ip, gw);
    let tuple = stack.tcp_connect(gw, 8080, 51000, Instant::now());
    let conn = stack.connections.get_mut_by_key(&tuple).unwrap();
    conn.bbr_max_bw = 1.25;
    let start = Instant::now();
    let iterations = 100_000;
    for _ in 0..iterations {
        conn.flush_send_buffer(&mut stack.tx_queue, Instant::now());
    }
    let elapsed = start.elapsed();
    let ns_per_op = elapsed.as_nanos() as f64 / iterations as f64;
    println!("  Pacing Target Rate:    10 Gbps (Saturated)");
    println!("  Iterations:            {}", iterations);
    println!("  FSM Pacing Latency:    \x1b[1;32m{:.2} ns/eval\x1b[0m", ns_per_op);
}
fn bench_simulation_chaos() {
    println!("\x1b[1;35m[6/6] BBR + SACK Scoreboard Deterministic Chaos Simulation\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    println!("  {:<18} | {:<10} | {:<12} | {:<14} | {:<10}", "Scenario", "Loss/Dup", "Payload", "Duration", "Sim Pkts/sec");
    println!("--------------------------------------------------------------------------------");
    let runs = vec![
        ("Clean Wire", 0.0, 0.0, 1_048_576),
        ("1% Packet Loss", 0.01, 0.0, 524_288),
        ("5% Loss + 2% Dup", 0.05, 0.02, 262_144),
    ];
    for (name, loss, dup, data_size) in runs {
        let config = SimConfig {
            loss_rate: loss,
            dup_rate: dup,
            min_delay: Duration::from_micros(10),
            max_delay: Duration::from_micros(200),
            data_size,
            max_ticks: 5_000_000,
        };
        let start = Instant::now();
        let res = run_simulation(42, &config);
        let elapsed = start.elapsed();
        let sim_pps = (res.ticks as f64) / elapsed.as_secs_f64();
        println!("  \x1b[1;37m{:<18}\x1b[0m | {:<4}%/{:<3}% | {:<10} KB | \x1b[1;32m{:<10.2} ms\x1b[0m | \x1b[1;32m{:<8.2} Kpps\x1b[0m",
            name,
            (loss * 100.0) as u32,
            (dup * 100.0) as u32,
            data_size / 1024,
            elapsed.as_secs_f64() * 1000.0,
            sim_pps / 1000.0
        );
    }
}
fn print_profile_report(tsc_ghz: f64, args: &[String]) {
    println!("\x1b[1;33m[STAGE PROFILING BREAKDOWN]\x1b[0m");
    println!("--------------------------------------------------------------------------------");
    println!("  {:<20} | {:<10} | {:<12} | {:<12} | {:<12}", "Stage", "Samples", "Avg (cy)", "Avg (ns)", "% of Total");
    println!("--------------------------------------------------------------------------------");
    let stats = probes::snapshot();
    let total_stage = stats[probes::StageId::TotalPacket as usize];
    let total_avg_cy = if total_stage.count > 0 { total_stage.total_cycles as f64 / total_stage.count as f64 } else { 1.0 };
    for i in 0..NUM_STAGES {
        let s = stats[i];
        if s.count == 0 { continue; }
        let avg_cy = s.total_cycles as f64 / s.count as f64;
        let avg_ns = avg_cy / tsc_ghz;
        let pct = (avg_cy / total_avg_cy) * 100.0;
        println!("  \x1b[1;37m{:<20}\x1b[0m | {:<10} | {:<12.1} | \x1b[1;32m{:<12.2}\x1b[0m | {:<11.1}%",
            STAGE_NAMES[i],
            s.count,
            avg_cy,
            avg_ns,
            pct.min(100.0)
        );
    }
    if let Some(out_flag) = args.iter().find(|a| a.starts_with("--profile-out=")) {
        if let Some(filename) = out_flag.strip_prefix("--profile-out=") {
            let mut json_stages = Vec::new();
            for i in 0..NUM_STAGES {
                let s = stats[i];
                if s.count == 0 { continue; }
                let avg_cy = s.total_cycles as f64 / s.count as f64;
                let avg_ns = avg_cy / tsc_ghz;
                json_stages.push(format!(
                    r#"{{"stage":"{}","count":{},"avg_cycles":{:.1},"avg_ns":{:.2}}}"#,
                    STAGE_NAMES[i], s.count, avg_cy, avg_ns
                ));
            }
            let json = format!(r#"{{"tsc_ghz":{:.3},"stages":[{}]}}"#, tsc_ghz, json_stages.join(","));
            let _ = std::fs::write(filename, json);
            println!("\n   Profile JSON exported to: \x1b[1;34m{}\x1b[0m", filename);
        }
    }
}
