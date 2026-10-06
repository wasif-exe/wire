use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const NUM_TIERS: usize = 4;
pub const SLOTS_PER_TIER: usize = 64;
pub const SLOT_MASK: u64 = 63;
pub const TIER_SHIFT: u32 = 6;
pub const MAX_EVENTS: usize = 65536;

pub const TICK_NANOS: u64 = 1_000;

#[repr(C, align(64))]
#[derive(Clone, Copy, Debug)]
pub struct TimerEntry {
    pub token: u64,
    pub deadline_ticks: u64,
    pub next: u32,
    pub active: bool,
}

impl Default for TimerEntry {
    fn default() -> Self {
        Self {
            token: 0,
            deadline_ticks: 0,
            next: u32::MAX,
            active: false,
        }
    }
}

pub struct TimingWheel {
    bitmasks: [AtomicU64; NUM_TIERS],
    slots: Box<[[AtomicU32; SLOTS_PER_TIER]; NUM_TIERS]>,
    entries: Box<[TimerEntry; MAX_EVENTS]>,
    free_head: u32,
    free_count: usize,
    current_ticks: u64,
}

impl TimingWheel {
    pub fn new(start_ticks: u64) -> Self {
        let mut entries = Box::new([TimerEntry::default(); MAX_EVENTS]);
        for i in 0..MAX_EVENTS - 1 {
            entries[i].next = (i + 1) as u32;
        }
        entries[MAX_EVENTS - 1].next = u32::MAX;

        let bitmasks = [
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
        ];

        let mut slots_raw: Box<[[AtomicU32; SLOTS_PER_TIER]; NUM_TIERS]> = Box::new(
            // SAFETY: Array of AtomicU32 initialized to atomic representation of u32::MAX.
            unsafe { std::mem::zeroed() }
        );

        for t in 0..NUM_TIERS {
            for s in 0..SLOTS_PER_TIER {
                slots_raw[t][s] = AtomicU32::new(u32::MAX);
            }
        }

        Self {
            bitmasks,
            slots: slots_raw,
            entries,
            free_head: 0,
            free_count: MAX_EVENTS,
            current_ticks: start_ticks,
        }
    }

    #[inline(always)]
    pub fn current_ticks(&self) -> u64 {
        self.current_ticks
    }

    #[inline]
    pub fn schedule(&mut self, token: u64, deadline_ticks: u64) -> Option<u32> {
        if self.free_count == 0 {
            return None;
        }

        let entry_idx = self.free_head;
        self.free_head = self.entries[entry_idx as usize].next;
        self.free_count -= 1;

        let entry = &mut self.entries[entry_idx as usize];
        entry.token = token;
        entry.deadline_ticks = deadline_ticks;
        entry.active = true;
        entry.next = u32::MAX;

        self.insert_entry(entry_idx, deadline_ticks);
        Some(entry_idx)
    }

    #[inline]
    pub fn cancel(&mut self, entry_idx: u32) {
        if (entry_idx as usize) >= MAX_EVENTS {
            return;
        }
        let entry = &mut self.entries[entry_idx as usize];
        if !entry.active {
            return;
        }
        entry.active = false;
        entry.next = self.free_head;
        self.free_head = entry_idx;
        self.free_count += 1;
    }

    fn insert_entry(&mut self, entry_idx: u32, deadline_ticks: u64) {
        let delta = deadline_ticks.saturating_sub(self.current_ticks);
        let (tier, slot) = if delta < 64 {
            (0, (deadline_ticks & SLOT_MASK) as usize)
        } else if delta < 4096 {
            (1, ((deadline_ticks >> TIER_SHIFT) & SLOT_MASK) as usize)
        } else if delta < 262144 {
            (2, ((deadline_ticks >> (TIER_SHIFT * 2)) & SLOT_MASK) as usize)
        } else {
            (3, ((deadline_ticks >> (TIER_SHIFT * 3)) & SLOT_MASK) as usize)
        };

        let head = self.slots[tier][slot].load(Ordering::Relaxed);
        self.entries[entry_idx as usize].next = head;
        self.slots[tier][slot].store(entry_idx, Ordering::Release);
        self.bitmasks[tier].fetch_or(1u64 << slot, Ordering::Release);
    }

    pub fn advance_to(&mut self, now_ticks: u64, mut callback: impl FnMut(u64)) {
        while self.current_ticks < now_ticks {
            self.current_ticks += 1;
            let slot = (self.current_ticks & SLOT_MASK) as usize;

            if slot == 0 {
                self.cascade();
            }

            let head = self.slots[0][slot].swap(u32::MAX, Ordering::AcqRel);
            self.bitmasks[0].fetch_and(!(1u64 << slot), Ordering::Release);

            let mut curr = head;
            while curr != u32::MAX {
                let entry = &mut self.entries[curr as usize];
                let next = entry.next;
                if entry.active {
                    entry.active = false;
                    callback(entry.token);
                    entry.next = self.free_head;
                    self.free_head = curr;
                    self.free_count += 1;
                }
                curr = next;
            }
        }
    }

    fn cascade(&mut self) {
        for tier in 1..NUM_TIERS {
            let shift = TIER_SHIFT * tier as u32;
            let slot = ((self.current_ticks >> shift) & SLOT_MASK) as usize;

            let head = self.slots[tier][slot].swap(u32::MAX, Ordering::AcqRel);
            self.bitmasks[tier].fetch_and(!(1u64 << slot), Ordering::Release);

            let mut curr = head;
            while curr != u32::MAX {
                let next = self.entries[curr as usize].next;
                if self.entries[curr as usize].active {
                    let deadline = self.entries[curr as usize].deadline_ticks;
                    self.insert_entry(curr, deadline);
                } else {
                    self.entries[curr as usize].next = self.free_head;
                    self.free_head = curr;
                    self.free_count += 1;
                }
                curr = next;
            }

            if slot != 0 {
                break;
            }
        }
    }
}
