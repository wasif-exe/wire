use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::mem::MaybeUninit;
use wire_core::cacheline::cpu_relax;

#[repr(C, align(64))]
struct Node<T> {
    sequence: AtomicU64,
    data: UnsafeCell<MaybeUninit<T>>,
}

#[repr(C, align(64))]
pub struct MpmcQueue<T> {
    head: AtomicU64,
    _pad_head: [u8; 56],
    tail: AtomicU64,
    _pad_tail: [u8; 56],
    buffer: Box<[Node<T>]>,
    mask: u64,
    capacity: u64,
}

unsafe impl<T: Send> Send for MpmcQueue<T> {}
unsafe impl<T: Send> Sync for MpmcQueue<T> {}

impl<T> MpmcQueue<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two());
        assert!(capacity >= 2);
        let cap = capacity as u64;
        let mask = cap - 1;

        let mut nodes = Vec::with_capacity(capacity);
        for i in 0..cap {
            nodes.push(Node {
                sequence: AtomicU64::new(i),
                data: UnsafeCell::new(MaybeUninit::uninit()),
            });
        }

        Self {
            head: AtomicU64::new(0),
            _pad_head: [0; 56],
            tail: AtomicU64::new(0),
            _pad_tail: [0; 56],
            buffer: nodes.into_boxed_slice(),
            mask,
            capacity: cap,
        }
    }

    #[inline(always)]
    pub fn try_push(&self, val: T) -> Result<(), T> {
        let mut tail = self.tail.load(Ordering::Relaxed);
        loop {
            let node_idx = (tail & self.mask) as usize;
            // SAFETY: node_idx is bounded by mask within buffer length.
            let node = unsafe { self.buffer.get_unchecked(node_idx) };
            let seq = node.sequence.load(Ordering::Acquire);
            let diff = seq as i64 - tail as i64;

            if diff == 0 {
                match self.tail.compare_exchange_weak(
                    tail,
                    tail.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: Claimed sequence ticket exclusively via atomic compare_exchange.
                        unsafe {
                            (*node.data.get()).write(val);
                        }
                        node.sequence.store(tail.wrapping_add(1), Ordering::Release);
                        return Ok(());
                    }
                    Err(actual) => {
                        tail = actual;
                    }
                }
            } else if diff < 0 {
                return Err(val);
            } else {
                tail = self.tail.load(Ordering::Relaxed);
            }
        }
    }

    #[inline(always)]
    pub fn push_spin(&self, mut val: T) {
        loop {
            match self.try_push(val) {
                Ok(()) => return,
                Err(v) => {
                    val = v;
                    cpu_relax();
                }
            }
        }
    }

    #[inline(always)]
    pub fn try_pop(&self) -> Option<T> {
        let mut head = self.head.load(Ordering::Relaxed);
        loop {
            let node_idx = (head & self.mask) as usize;
            // SAFETY: node_idx is bounded by mask within buffer length.
            let node = unsafe { self.buffer.get_unchecked(node_idx) };
            let seq = node.sequence.load(Ordering::Acquire);
            let diff = seq as i64 - head.wrapping_add(1) as i64;

            if diff == 0 {
                match self.head.compare_exchange_weak(
                    head,
                    head.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: Claimed read ticket exclusively; memory initialized on prior push.
                        let val = unsafe { (*node.data.get()).assume_init_read() };
                        node.sequence.store(
                            head.wrapping_add(self.capacity),
                            Ordering::Release,
                        );
                        return Some(val);
                    }
                    Err(actual) => {
                        head = actual;
                    }
                }
            } else if diff < 0 {
                return None;
            } else {
                head = self.head.load(Ordering::Relaxed);
            }
        }
    }

    #[inline(always)]
    pub fn pop_spin(&self) -> T {
        loop {
            if let Some(val) = self.try_pop() {
                return val;
            }
            cpu_relax();
        }
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        tail <= head
    }
}

impl<T> Drop for MpmcQueue<T> {
    fn drop(&mut self) {
        while self.try_pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn single_thread_fifo() {
        let q = MpmcQueue::<u64>::new(16);
        for i in 0..10 {
            assert!(q.try_push(i).is_ok());
        }
        for i in 0..10 {
            assert_eq!(q.try_pop(), Some(i));
        }
        assert_eq!(q.try_pop(), None);
    }

    #[test]
    fn queue_full_backpressure() {
        let q = MpmcQueue::<u32>::new(4);
        assert!(q.try_push(1).is_ok());
        assert!(q.try_push(2).is_ok());
        assert!(q.try_push(3).is_ok());
        assert!(q.try_push(4).is_ok());
        assert_eq!(q.try_push(5), Err(5));
        assert_eq!(q.try_pop(), Some(1));
        assert!(q.try_push(5).is_ok());
    }

    #[test]
    fn mpmc_multi_producer_multi_consumer() {
        let q = Arc::new(MpmcQueue::<u64>::new(1024));
        let num_producers = 4;
        let num_consumers = 4;
        let items_per_prod = 50_000u64;

        let mut prod_handles = Vec::new();
        for p in 0..num_producers {
            let q_clone = Arc::clone(&q);
            prod_handles.push(thread::spawn(move || {
                for i in 0..items_per_prod {
                    let val = (p as u64) * items_per_prod + i;
                    q_clone.push_spin(val);
                }
            }));
        }

        let mut cons_handles = Vec::new();
        for _ in 0..num_consumers {
            let q_clone = Arc::clone(&q);
            cons_handles.push(thread::spawn(move || {
                let mut count = 0u64;
                let target = (num_producers as u64 * items_per_prod) / num_consumers as u64;
                while count < target {
                    if let Some(_val) = q_clone.try_pop() {
                        count += 1;
                    } else {
                        cpu_relax();
                    }
                }
                count
            }));
        }

        for h in prod_handles {
            h.join().unwrap();
        }
        let mut total_consumed = 0u64;
        for h in cons_handles {
            total_consumed += h.join().unwrap();
        }

        assert_eq!(total_consumed, (num_producers as u64) * items_per_prod);
    }

    #[test]
    fn cacheline_alignment_mpmc() {
        assert_eq!(std::mem::align_of::<MpmcQueue<u64>>(), 64);
        assert_eq!(std::mem::align_of::<Node<u64>>(), 64);
    }
}
