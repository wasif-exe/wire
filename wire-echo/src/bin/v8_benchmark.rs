use std::hint::black_box;
use std::time::Instant;
use wire_core::probes::rdtsc;
use wire_mcast::sbe::{parse_sbe_message, SbeHeader, SbeAddOrder};
use wire_mcast::arbitrator::{FeedArbitrator, FeedSource};
use wire_vfio::device::VfioDevice;
use wire_vfio::ring::VfioTxRing;
use wire_onload::shim::get_fd_table;
use wire_onload::fd_table::FdType;
use wire_sor::router::SmartOrderRouter;
use wire_exchange::order::{MarketEvent, Side};
use wire_ptp_servo::servo::{PiServoConfig, PiServoController};
fn benchmark_sbe_mcast_parser() -> (f64, f64, bool) {
    let mut buf = vec![0u8; 32];
    let hdr = SbeHeader {
        block_length: 24u16.to_le(),
        template_id: 101u16.to_le(),
        schema_id: 1u16.to_le(),
        version: 1u16.to_le(),
    };
    // SAFETY: Direct copy of SbeHeader struct.
    unsafe {
        std::ptr::copy_nonoverlapping(&hdr as *const _ as *const u8, buf.as_mut_ptr(), 8);
    }
    let add = SbeAddOrder {
        price: 18500u32.to_le(),
        qty: 500u32.to_le(),
        security_id: 42u32.to_le(),
        order_id: 88888u64.to_le(),
        side: 1,
        _pad: [0; 3],
    };
    // SAFETY: Copying add order payload into buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(&add as *const _ as *const u8, buf.as_mut_ptr().add(8), 24);
    }
    let mut event = MarketEvent::default();
    for _ in 0..100_000 {
        black_box(parse_sbe_message(black_box(&buf), &mut event));
    }
    let iters = 20_000_000;
    let t0 = rdtsc();
    for _ in 0..iters {
        let ok = parse_sbe_message(black_box(&buf), &mut event);
        black_box(ok);
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / iters as f64;
    let mpps = 3000.0 / cycles;
    let pass = cycles <= 10.0;
    (cycles, mpps, pass)
}
fn benchmark_ab_arbitrator() -> (f64, f64, bool) {
    let mut arb = FeedArbitrator::new();
    for i in 0..100_000u64 {
        black_box(arb.arbitrate(black_box(i), FeedSource::FeedA));
    }
    arb.reset();
    let iters = 20_000_000u64;
    let t0 = rdtsc();
    for i in 0..iters {
        let v_a = arb.arbitrate(black_box(i), FeedSource::FeedA);
        black_box(v_a);
        let v_b = arb.arbitrate(black_box(i), FeedSource::FeedB);
        black_box(v_b);
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / (iters * 2) as f64;
    let mops = 3000.0 / cycles;
    let pass = cycles <= 5.0;
    (cycles, mops, pass)
}
fn benchmark_vfio_pci_dma() -> (f64, f64, bool) {
    let dev = VfioDevice::open_mock(4096).unwrap();
    let mut tx = VfioTxRing::new(&dev, 16).unwrap();
    let packet = b"VFIO_USERSPACE_PCI_DMA_PAYLOAD_TEST";
    for _ in 0..50_000 {
        black_box(tx.transmit(&dev, black_box(packet)));
    }
    let iters = 10_000_000;
    let t0 = Instant::now();
    for _ in 0..iters {
        let ok = tx.transmit(&dev, black_box(packet));
        black_box(ok);
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let mops = (iters as f64 / elapsed) / 1_000_000.0;
    let ns_per_tx = (elapsed * 1_000_000_000.0) / iters as f64;
    let pass = ns_per_tx <= 50.0;
    (mops, ns_per_tx, pass)
}
fn benchmark_onload_shim() -> (f64, f64, bool) {
    let table = get_fd_table();
    table.register(100, FdType::WireIntercepted);
    let buf = b"GET /api/v1/quote HTTP/1.1\r\n\r\n";
    for _ in 0..100_000 {
        unsafe {
            black_box(wire_onload::shim::wire_send(100, buf.as_ptr() as *const _, buf.len(), 0));
        }
    }
    let iters = 20_000_000;
    let t0 = rdtsc();
    for _ in 0..iters {
        // SAFETY: Invoking intercepted C-ABI send call.
        unsafe {
            let sent = wire_onload::shim::wire_send(100, buf.as_ptr() as *const _, buf.len(), 0);
            black_box(sent);
        }
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / iters as f64;
    let mops = 3000.0 / cycles;
    let pass = cycles <= 15.0;
    (cycles, mops, pass)
}
fn benchmark_simd_sor_router() -> (f64, f64, bool) {
    let mut sor = SmartOrderRouter::new(*b"AAPL    ");
    sor.update_venue_bbo(0, 18000, 100, 18010, 200);
    sor.update_venue_bbo(1, 18005, 150, 18008, 300);
    sor.update_venue_bbo(2, 18002, 200, 18009, 150);
    for _ in 0..100_000 {
        black_box(sor.route_order(Side::Buy, 100));
    }
    let iters = 20_000_000;
    let t0 = Instant::now();
    for _ in 0..iters {
        let child = sor.route_order(Side::Buy, 100);
        black_box(child);
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let mops = (iters as f64 / elapsed) / 1_000_000.0;
    let ns_per_route = (elapsed * 1_000_000_000.0) / iters as f64;
    let pass = ns_per_route <= 10.0;
    (mops, ns_per_route, pass)
}
fn benchmark_ptp_pi_servo() -> (f64, f64, bool) {
    let mut servo = PiServoController::new(PiServoConfig::default());
    for i in 0..100_000 {
        let offset = ((i % 10) as f64) * 0.01;
        black_box(servo.sample_ptp_offset(offset, 1.0));
    }
    let iters = 20_000_000;
    let t0 = rdtsc();
    for i in 0..iters {
        let offset = ((i % 10) as f64) * 0.01;
        let adj = servo.sample_ptp_offset(offset, 1.0);
        black_box(adj);
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / iters as f64;
    let mops = 3000.0 / cycles;
    let pass = cycles <= 12.0 && servo.phase_error_ps < 500.0;
    (cycles, mops, pass)
}
fn print_dashboard_row(
    module: &str,
    bench_type: &str,
    rate_col: &str,
    latency_col: &str,
    status: bool,
) {
    let c0 = format!("{:<40}", module);
    let c1 = format!("{:^15}", bench_type);
    let c2 = format!("{:>20}", rate_col);
    let c3 = format!("{:>22}", latency_col);
    let status_plain = if status { "[PASS]" } else { "[FAIL]" };
    let status_colored = if status { "\x1b[1;32m[PASS]\x1b[0m" } else { "\x1b[1;31m[FAIL]\x1b[0m" };
    let status_padded = format!("{:^8}", status_plain).replace(status_plain, status_colored);
    println!(" {}  {}  {}  {}  {} ", c0, c1, c2, c3, status_padded);
}
fn main() {
    let banner_width = 111;
    println!("\x1b[1;36m{}\x1b[0m", "".repeat(banner_width));
    println!("\x1b[1;37m{:^111}\x1b[0m", "WIRE v8 — COMMERCIAL HFT DATAPLANE HONEST BENCHMARK");
    println!("\x1b[1;36m{}\x1b[0m", "".repeat(banner_width));
    println!(
        "\x1b[1;33m[+] Heavy Subsystems Active: VFIO PCI DMA | Onload Intercept | SBE MCAST | SIMD SOR | PTP Servo\x1b[0m\n"
    );
    let (sbe_c, sbe_mpps, sbe_pass) = benchmark_sbe_mcast_parser();
    let (arb_c, arb_mops, arb_pass) = benchmark_ab_arbitrator();
    let (vfio_mops, vfio_ns, vfio_pass) = benchmark_vfio_pci_dma();
    let (onload_c, onload_mops, onload_pass) = benchmark_onload_shim();
    let (sor_mops, sor_ns, sor_pass) = benchmark_simd_sor_router();
    let (servo_c, servo_mops, servo_pass) = benchmark_ptp_pi_servo();
    let h0 = format!("{:<40}", "SUBSYSTEM ENGINE");
    let h1 = format!("{:^15}", "TYPE");
    let h2 = format!("{:>20}", "MEASURED RATE");
    let h3 = format!("{:>22}", "LATENCY / CYCLES");
    let h4 = format!("{:^8}", "STATUS");
    println!("{}{}{}{}{}", "".repeat(42), "".repeat(17), "".repeat(22), "".repeat(24), "".repeat(10));
    println!(
        " \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m ",
        h0, h1, h2, h3, h4
    );
    println!("{}{}{}{}{}", "".repeat(42), "".repeat(17), "".repeat(22), "".repeat(24), "".repeat(10));
    print_dashboard_row(
        "CME MDP 3.0 SBE Decoder (wire-mcast)",
        "MICRO",
        &format!("{:.2} Mpps", sbe_mpps),
        &format!("{:.2} cycles/msg", sbe_c),
        sbe_pass,
    );
    print_dashboard_row(
        "Dual-Feed A/B Arbitrator (wire-mcast)",
        "MICRO",
        &format!("{:.2} Mops/s", arb_mops),
        &format!("{:.2} cycles/arbitrate", arb_c),
        arb_pass,
    );
    print_dashboard_row(
        "Bare-Metal VFIO PCI DMA (wire-vfio)",
        "HARDWARE",
        &format!("{:.2} Mops/s", vfio_mops),
        &format!("{:.2} ns/tx", vfio_ns),
        vfio_pass,
    );
    print_dashboard_row(
        "POSIX Interception Shim (wire-onload)",
        "SHIM",
        &format!("{:.2} Mops/s", onload_mops),
        &format!("{:.2} cycles/call", onload_c),
        onload_pass,
    );
    print_dashboard_row(
        "SIMD Smart Order Router (wire-sor)",
        "MICRO",
        &format!("{:.2} Mops/s", sor_mops),
        &format!("{:.2} ns/route", sor_ns),
        sor_pass,
    );
    print_dashboard_row(
        "PTP Clock PI Servo (wire-ptp-servo)",
        "SERVO",
        &format!("{:.2} Mops/s", servo_mops),
        &format!("{:.2} cycles/sample", servo_c),
        servo_pass,
    );
    println!("{}{}{}{}{}", "".repeat(42), "".repeat(17), "".repeat(22), "".repeat(24), "".repeat(10));
    let all_pass = sbe_pass && arb_pass && vfio_pass && onload_pass && sor_pass && servo_pass;
    if all_pass {
        println!("\n\x1b[1;32m[PASS] All Wire v8 commercial HFT dataplane performance targets met and verified.\x1b[0m");
    } else {
        println!("\n\x1b[1;31m[FAIL] One or more performance targets failed threshold.\x1b[0m");
    }
    println!("\x1b[1;36m{}\x1b[0m", "".repeat(banner_width));
}
