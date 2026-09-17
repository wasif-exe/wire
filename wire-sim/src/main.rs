use std::time::Duration;
use wire_sim::{SimConfig, run_simulation};

fn main() {
    let configs = vec![
        ("clean", SimConfig {
            loss_rate: 0.0,
            dup_rate: 0.0,
            min_delay: Duration::from_micros(10),
            max_delay: Duration::from_micros(100),
            data_size: 64_000,
            max_ticks: 500_000,
        }),
        ("1% loss", SimConfig {
            loss_rate: 0.01,
            dup_rate: 0.0,
            min_delay: Duration::from_micros(10),
            max_delay: Duration::from_millis(1),
            data_size: 32_000,
            max_ticks: 1_000_000,
        }),
        ("5% loss + 2% dup", SimConfig {
            loss_rate: 0.05,
            dup_rate: 0.02,
            min_delay: Duration::from_micros(50),
            max_delay: Duration::from_millis(5),
            data_size: 16_000,
            max_ticks: 2_000_000,
        }),
    ];

    for (name, config) in &configs {
        let mut pass = 0;
        let mut fail = 0;
        let seeds = 100;
        for seed in 0..seeds {
            let result = run_simulation(seed, config);
            if result.data_match && result.connection_closed {
                pass += 1;
            } else {
                fail += 1;
                if fail <= 3 {
                    println!("  FAIL seed={} sent={} recv={} match={} closed={} ticks={}",
                        result.seed, result.bytes_sent, result.bytes_received,
                        result.data_match, result.connection_closed, result.ticks);
                }
            }
        }
        println!("[{}] {}/{} passed ({} failed)", name, pass, seeds, fail);
    }
}
