use criterion::{black_box, criterion_group, criterion_main, Criterion};
use wire_simd::{parse_one, ParsedL4};

fn bench_simd_parser(c: &mut Criterion) {
    let mut frame = vec![0u8; 1514];
    
    frame[0..6].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
    frame[6..12].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    
    frame[14] = 0x45;
    frame[15] = 0x00;
    frame[16..18].copy_from_slice(&1500u16.to_be_bytes());
    frame[18..20].copy_from_slice(&0u16.to_be_bytes());
    frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    frame[22] = 64;
    frame[23] = 6;
    frame[24..26].copy_from_slice(&0u16.to_be_bytes());
    frame[26..30].copy_from_slice(&[10, 0, 0, 1]);
    frame[30..34].copy_from_slice(&[10, 0, 0, 2]);
    
    frame[34..36].copy_from_slice(&12345u16.to_be_bytes());
    frame[36..38].copy_from_slice(&80u16.to_be_bytes());
    frame[38..42].copy_from_slice(&1000u32.to_be_bytes());
    frame[42..46].copy_from_slice(&2000u32.to_be_bytes());
    frame[46] = 5 << 4;
    frame[47] = 0x18;
    frame[48..50].copy_from_slice(&65535u16.to_be_bytes());
    
    let mut sum: u32 = 0;
    for chunk in frame[14..34].chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let ip_csum = !(sum as u16);
    frame[24..26].copy_from_slice(&ip_csum.to_be_bytes());

    c.bench_function("parse_one_avx2", |b| {
        b.iter(|| {
            let res = parse_one(black_box(&frame));
            black_box(res);
        })
    });
}

criterion_group!(benches, bench_simd_parser);
criterion_main!(benches);
