#[cfg(feature = "profile")]
pub mod probes {
    use std::cell::RefCell;
    use std::sync::OnceLock;

    #[repr(u8)]
    #[derive(Clone, Copy, Debug)]
    pub enum StageId {
        RxRingAcquire = 0,
        L2Parse = 1,
        L3Parse = 2,
        L4Demux = 3,
        TupleLookup = 4,
        TcpFsmStep = 5,
        SackScoreboard = 6,
        BbrUpdate = 7,
        PayloadCopy = 8,
        TxEnqueue = 9,
        TxCompleteReap = 10,
        TotalPacket = 11,
        SimdParse = 12,
        SimdParseBatch = 13,
    }

    pub const NUM_STAGES: usize = 14;

    pub const STAGE_NAMES: [&str; NUM_STAGES] = [
        "RX_RING_ACQUIRE",
        "L2_PARSE",
        "L3_PARSE",
        "L4_DEMUX",
        "TUPLE_LOOKUP",
        "TCP_FSM_STEP",
        "SACK_SCOREBOARD",
        "BBR_UPDATE",
        "PAYLOAD_COPY",
        "TX_ENQUEUE",
        "TX_COMPLETE_REAP",
        "TOTAL_PACKET",
        "SIMD_PARSE",
        "SIMD_PARSE_BATCH",
    ];

    #[derive(Clone, Copy, Default, Debug)]
    pub struct StageStats {
        pub count: u64,
        pub total_cycles: u64,
        pub min_cycles: u64,
        pub max_cycles: u64,
    }

    thread_local! {
        static STATS: RefCell<[StageStats; NUM_STAGES]> = RefCell::new([StageStats::default(); NUM_STAGES]);
        static SAMPLE_COUNTER: RefCell<u64> = RefCell::new(0);
    }

    static SAMPLE_RATE: OnceLock<u64> = OnceLock::new();

    pub fn set_sample_rate(rate: u64) {
        let _ = SAMPLE_RATE.set(rate);
    }

    #[inline(always)]
    pub fn rdtsc() -> u64 {
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::x86_64::_rdtsc() }
        #[cfg(not(target_arch = "x86_64"))]
        {
            use std::time::Instant;
            Instant::now().elapsed().as_nanos() as u64
        }
    }

    #[inline(always)]
    pub fn should_sample() -> bool {
        let rate = *SAMPLE_RATE.get().unwrap_or(&1);
        if rate <= 1 { return true; }
        SAMPLE_COUNTER.with(|c| {
            let mut v = c.borrow_mut();
            *v = v.wrapping_add(1);
            *v % rate == 0
        })
    }

    #[inline(always)]
    pub fn stage_begin(_id: StageId) -> u64 {
        if !should_sample() { return 0; }
        rdtsc()
    }

    #[inline(always)]
    pub fn stage_end(id: StageId, start: u64) {
        if start == 0 { return; }
        let elapsed = rdtsc().wrapping_sub(start);
        STATS.with(|s| {
            let mut stats = s.borrow_mut();
            let entry = &mut stats[id as usize];
            entry.count += 1;
            entry.total_cycles += elapsed;
            if entry.min_cycles == 0 || elapsed < entry.min_cycles {
                entry.min_cycles = elapsed;
            }
            if elapsed > entry.max_cycles {
                entry.max_cycles = elapsed;
            }
        });
    }

    pub fn snapshot() -> [StageStats; NUM_STAGES] {
        STATS.with(|s| *s.borrow())
    }

    pub fn reset() {
        STATS.with(|s| {
            *s.borrow_mut() = [StageStats::default(); NUM_STAGES];
        });
    }
}

#[cfg(not(feature = "profile"))]
pub mod probes {
    #[repr(u8)]
    #[derive(Clone, Copy, Debug)]
    pub enum StageId {
        RxRingAcquire = 0,
        L2Parse = 1,
        L3Parse = 2,
        L4Demux = 3,
        TupleLookup = 4,
        TcpFsmStep = 5,
        SackScoreboard = 6,
        BbrUpdate = 7,
        PayloadCopy = 8,
        TxEnqueue = 9,
        TxCompleteReap = 10,
        TotalPacket = 11,
        SimdParse = 12,
        SimdParseBatch = 13,
    }

    pub const NUM_STAGES: usize = 14;

    pub const STAGE_NAMES: [&str; NUM_STAGES] = [
        "RX_RING_ACQUIRE",
        "L2_PARSE",
        "L3_PARSE",
        "L4_DEMUX",
        "TUPLE_LOOKUP",
        "TCP_FSM_STEP",
        "SACK_SCOREBOARD",
        "BBR_UPDATE",
        "PAYLOAD_COPY",
        "TX_ENQUEUE",
        "TX_COMPLETE_REAP",
        "TOTAL_PACKET",
        "SIMD_PARSE",
        "SIMD_PARSE_BATCH",
    ];

    #[derive(Clone, Copy, Default, Debug)]
    pub struct StageStats {
        pub count: u64,
        pub total_cycles: u64,
        pub min_cycles: u64,
        pub max_cycles: u64,
    }

    #[inline(always)]
    pub fn rdtsc() -> u64 {
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::x86_64::_rdtsc() }
        #[cfg(not(target_arch = "x86_64"))]
        {
            use std::time::Instant;
            Instant::now().elapsed().as_nanos() as u64
        }
    }

    #[inline(always)]
    pub fn stage_begin(_id: StageId) -> u64 { 0 }

    #[inline(always)]
    pub fn stage_end(_id: StageId, _start: u64) {}

    pub fn set_sample_rate(_rate: u64) {}

    pub fn reset() {}

    pub fn snapshot() -> [StageStats; NUM_STAGES] {
        [StageStats::default(); NUM_STAGES]
    }
}
