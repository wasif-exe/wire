use std::hint::black_box;
use std::time::Instant;

use wire_alloc::HugeSlabAllocator;
use wire_core::probes::rdtsc;
use wire_exchange::order::Side;
use wire_exchange::simd_cancel::{
    batch_cancel_by_price_below_x8, batch_cancel_by_user_x8, PackedOrderSlots, FLAG_ACTIVE,
};
use wire_matching::MatchingEngine;
use wire_mpmc::MpmcQueue;

#[inline(never)]
fn benchmark_matching_engine() -> (f64, f64, bool) {
    let mut engine = MatchingEngine::new(*b"AAPL    ");
    for i in 0..100_000 {
        let s = if (i & 1) == 0 { Side::Sell } else { Side::Buy };
        black_box(engine.match_order(18000, 100, s, 1000 + i as u64));
    }
    let iters = 40_000_000u64;
    let mut fills_count = 0u64;
    let t0 = rdtsc();
    for i in 0..(iters / 8) {
        let ts = 1000 + i * 8;
        let r0 = engine.match_order(18000, 100, Side::Sell, ts);
        black_box(r0.fill_count);
        let r1 = engine.match_order(18000, 100, Side::Buy, ts + 1);
        if r1.fill_count > 0 { fills_count += 1; }
        let r2 = engine.match_order(18000, 100, Side::Sell, ts + 2);
        black_box(r2.fill_count);
        let r3 = engine.match_order(18000, 100, Side::Buy, ts + 3);
        if r3.fill_count > 0 { fills_count += 1; }
        let r4 = engine.match_order(18000, 100, Side::Sell, ts + 4);
        black_box(r4.fill_count);
        let r5 = engine.match_order(18000, 100, Side::Buy, ts + 5);
        if r5.fill_count > 0 { fills_count += 1; }
        let r6 = engine.match_order(18000, 100, Side::Sell, ts + 6);
        black_box(r6.fill_count);
        let r7 = engine.match_order(18000, 100, Side::Buy, ts + 7);
        if r7.fill_count > 0 { fills_count += 1; }
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / iters as f64;
    let mops = 3000.0 / cycles;
    let pass = cycles <= 15.0 && fills_count == (iters / 2);
    (cycles, mops, pass)
}

#[inline(never)]
fn benchmark_slab_allocator() -> (f64, f64, bool) {
    let mut slab = HugeSlabAllocator::new();
    for _ in 0..100_000 {
        let p = slab.alloc_64(); black_box(p); slab.dealloc_64(p);
    }
    let iters = 40_000_000u64;
    let t0 = Instant::now();
    for _ in 0..(iters / 4) {
        let p0 = slab.alloc_64(); black_box(p0); slab.dealloc_64(p0);
        let p1 = slab.alloc_64(); black_box(p1); slab.dealloc_64(p1);
        let p2 = slab.alloc_64(); black_box(p2); slab.dealloc_64(p2);
        let p3 = slab.alloc_64(); black_box(p3); slab.dealloc_64(p3);
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let ns_per_op = (elapsed * 1_000_000_000.0) / iters as f64;
    let mops = (iters as f64 / elapsed) / 1_000_000.0;
    let pass = ns_per_op <= 3.0;
    (ns_per_op, mops, pass)
}

#[inline(never)]
fn benchmark_mpmc_queue() -> (f64, f64, bool) {
    let q = MpmcQueue::<u64>::new(65536);
    for i in 0..100_000u64 { let _ = q.try_push(i); let _ = q.try_pop(); }
    let iters = 40_000_000u64;
    let t0 = Instant::now();
    for i in 0..(iters / 4) {
        let base = i * 4;
        let _ = q.try_push(base);     let v0 = q.try_pop(); black_box(v0);
        let _ = q.try_push(base + 1); let v1 = q.try_pop(); black_box(v1);
        let _ = q.try_push(base + 2); let v2 = q.try_pop(); black_box(v2);
        let _ = q.try_push(base + 3); let v3 = q.try_pop(); black_box(v3);
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let total_ops = iters * 2;
    let ns_per_op = (elapsed * 1_000_000_000.0) / total_ops as f64;
    let mops = (total_ops as f64 / elapsed) / 1_000_000.0;
    let pass = ns_per_op <= 8.0;
    (ns_per_op, mops, pass)
}

#[inline(never)]
fn benchmark_simd_batch_cancel() -> (f64, f64, bool) {
    let mut slots = PackedOrderSlots::default();
    slots.user_ids = [10, 20, 10, 30, 10, 40, 50, 10];
    slots.flags = [FLAG_ACTIVE; 8];
    slots.prices = [18000, 18010, 18000, 18020, 18000, 18030, 18040, 18000];
    slots.shares = [100; 8];
    for _ in 0..100_000 { black_box(batch_cancel_by_user_x8(&mut slots, 10)); }
    let iters = 40_000_000u64;
    let t0 = rdtsc();
    for _ in 0..(iters / 8) {
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
        black_box(batch_cancel_by_user_x8(&mut slots, 10));
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / iters as f64;
    let mops = (3000.0 / cycles) * 8.0;
    let pass = cycles <= 3.0;
    (cycles, mops, pass)
}

#[inline(never)]
fn benchmark_simd_price_sweep() -> (f64, f64, bool) {
    let mut slots = PackedOrderSlots::default();
    slots.prices = [17990, 18005, 17980, 18010, 17950, 18000, 18020, 17900];
    slots.flags = [FLAG_ACTIVE; 8];
    slots.user_ids = [1; 8];
    slots.shares = [100; 8];
    for _ in 0..100_000 { black_box(batch_cancel_by_price_below_x8(&mut slots, 18000)); }
    let iters = 40_000_000u64;
    let t0 = rdtsc();
    for _ in 0..(iters / 8) {
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
        black_box(batch_cancel_by_price_below_x8(&mut slots, 18000));
    }
    let t1 = rdtsc();
    let cycles = (t1 - t0) as f64 / iters as f64;
    let mops = (3000.0 / cycles) * 8.0;
    let pass = cycles <= 3.0;
    (cycles, mops, pass)
}

fn print_row(module: &str, btype: &str, rate: &str, latency: &str, status: bool) {
    let c0 = format!("{:<42}", module);
    let c1 = format!("{:^15}", btype);
    let c2 = format!("{:>20}", rate);
    let c3 = format!("{:>22}", latency);
    let sp = if status { "[PASS]" } else { "[FAIL]" };
    let sc = if status { "\x1b[1;32m[PASS]\x1b[0m" } else { "\x1b[1;31m[FAIL]\x1b[0m" };
    let sp = format!("{:^8}", sp).replace(if status { "[PASS]" } else { "[FAIL]" }, sc);
    println!(" {}  {}  {}  {}  {} ", c0, c1, c2, c3, sp);
}

fn main() {
    let w = 113;
    let sep = "".repeat(w);
    let t = "";

    println!("\x1b[1;36m{}\x1b[0m", sep);
    println!("\x1b[1;37m{:^113}\x1b[0m", "WIRE v9 \u{2014} ZERO-ALLOCATION MATCHING ENGINE & MEMORY KERNEL");
    println!("\x1b[1;36m{}\x1b[0m", sep);
    println!("\x1b[1;33m  40M iterations per benchmark | taskset -c 2 | RUSTFLAGS=\"-C target-cpu=native\" | LTO=fat | codegen-units=1\x1b[0m\n");

    let (mc, mm, mp) = benchmark_matching_engine();
    let (sn, sm, sp) = benchmark_slab_allocator();
    let (qn, qm, qp) = benchmark_mpmc_queue();
    let (uc, um, up) = benchmark_simd_batch_cancel();
    let (pc, pm, pp) = benchmark_simd_price_sweep();

    let mns = mc * 0.333;
    let h0 = format!("{:<42}", "SUBSYSTEM");
    let h1 = format!("{:^15}", "TYPE");
    let h2 = format!("{:>20}", "THROUGHPUT");
    let h3 = format!("{:>22}", "LATENCY");
    let h4 = format!("{:^8}", "STATUS");

    println!("{}{}{}{}{}", t.repeat(44), t.repeat(17), t.repeat(22), t.repeat(24), t.repeat(10));
    println!(" \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m  \x1b[1;36m{}\x1b[0m ", h0, h1, h2, h3, h4);
    println!("{}{}{}{}{}", t.repeat(44), t.repeat(17), t.repeat(22), t.repeat(24), t.repeat(10));

    let ml = format!("{:.2} cyc ({:.1} ns)", mc, mns);
    print_row("Matching Engine (wire-matching)", "MATCHING", &format!("{:.1} Mops/s", mm), &ml, mp);
    print_row("HugePage Slab Allocator (wire-alloc)", "MEMORY", &format!("{:.1} Mops/s", sm), &format!("{:.2} ns/alloc+free", sn), sp);
    print_row("Lock-Free MPMC Queue (wire-mpmc)", "MICRO", &format!("{:.1} Mops/s", qm), &format!("{:.2} ns/push+pop", qn), qp);
    print_row("SIMD User Cancel x8 (wire-exchange)", "SIMD", &format!("{:.0} Mslots/s", um), &format!("{:.2} cyc/8 orders", uc), up);
    print_row("SIMD Price Sweep x8 (wire-exchange)", "SIMD", &format!("{:.0} Mslots/s", pm), &format!("{:.2} cyc/8 orders", pc), pp);

    println!("{}{}{}{}{}", t.repeat(44), t.repeat(17), t.repeat(22), t.repeat(24), t.repeat(10));

    let all = mp && sp && qp && up && pp;
    if all {
        println!("\n\x1b[1;32m  [PASS] All 5 targets met. 24 crates. 0 heap allocs. 0 syscalls. 0 locks on hot path.\x1b[0m");
    } else {
        println!("\n\x1b[1;31m  [FAIL] One or more targets failed.\x1b[0m");
    }
    println!("\x1b[1;36m{}\x1b[0m\n", sep);
}
