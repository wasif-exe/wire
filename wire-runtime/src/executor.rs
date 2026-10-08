use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::unix::io::AsRawFd;
use std::task::{Context, Poll};
use io_uring::IoUring;
use crate::task::{Task, TaskSlot};
use crate::waker::create_waker;

pub const MAX_TASKS: usize = 4096;

thread_local! {
    static CURRENT_EXECUTOR: RefCell<Option<*mut LocalExecutor>> = RefCell::new(None);
}

pub fn wake_task_by_id(task_id: usize) {
    CURRENT_EXECUTOR.with(|exec_cell| {
        if let Some(exec_ptr) = *exec_cell.borrow() {
            // SAFETY: Dereferencing current thread-local pinned executor pointer.
            unsafe {
                (*exec_ptr).enqueue_task(task_id);
            }
        }
    });
}

pub struct LocalExecutor {
    core_id: usize,
    slots: Vec<TaskSlot>,
    run_queue: VecDeque<usize>,
    free_slots: Vec<usize>,
    ring: IoUring,
}

impl LocalExecutor {
    pub fn new(core_id: usize) -> anyhow::Result<Self> {
        let _ = wire_xdp::pin_thread_to_core(core_id);

        let mut slots = Vec::with_capacity(MAX_TASKS);
        let mut free_slots = Vec::with_capacity(MAX_TASKS);
        for i in 0..MAX_TASKS {
            slots.push(TaskSlot::empty());
            free_slots.push(MAX_TASKS - 1 - i);
        }

        let ring = match IoUring::builder().setup_sqpoll(2000).build(512) {
            Ok(r) => r,
            Err(_) => IoUring::new(512)?,
        };

        Ok(Self {
            core_id,
            slots,
            run_queue: VecDeque::with_capacity(MAX_TASKS),
            free_slots,
            ring,
        })
    }

    pub fn core_id(&self) -> usize {
        self.core_id
    }

    pub fn ring_fd(&self) -> std::os::unix::io::RawFd {
        self.ring.as_raw_fd()
    }

    pub fn spawn<F>(&mut self, future: F) -> Option<usize>
    where
        F: std::future::Future<Output = ()> + 'static,
    {
        let slot_idx = self.free_slots.pop()?;
        let slot = &mut self.slots[slot_idx];
        slot.runnable = Some(Box::new(Task::new(future)));
        slot.is_queued = true;
        slot.generation = slot.generation.wrapping_add(1);

        self.run_queue.push_back(slot_idx);
        Some(slot_idx)
    }

    #[inline(always)]
    pub fn enqueue_task(&mut self, task_id: usize) {
        if task_id < MAX_TASKS {
            let slot = &mut self.slots[task_id];
            if slot.runnable.is_some() && !slot.is_queued {
                slot.is_queued = true;
                self.run_queue.push_back(task_id);
            }
        }
    }

    pub fn run_single_tick(&mut self) -> usize {
        let prev = CURRENT_EXECUTOR.with(|c| c.replace(Some(self as *mut _)));
        let mut completed_count = 0;
        let mut budget = 64;

        while budget > 0 {
            let task_id = match self.run_queue.pop_front() {
                Some(id) => id,
                None => break,
            };

            budget -= 1;
            let mut runnable = {
                let slot = &mut self.slots[task_id];
                slot.is_queued = false;
                slot.runnable.take()
            };

            if let Some(ref mut task) = runnable {
                let waker = create_waker(task_id);
                let mut cx = Context::from_waker(&waker);

                match task.poll_run(&mut cx) {
                    Poll::Ready(()) => {
                        let slot = &mut self.slots[task_id];
                        slot.runnable = None;
                        slot.is_queued = false;
                        self.free_slots.push(task_id);
                        completed_count += 1;
                    }
                    Poll::Pending => {
                        let slot = &mut self.slots[task_id];
                        slot.runnable = runnable;
                    }
                }
            }
        }

        let _ = self.ring.submit();
        CURRENT_EXECUTOR.with(|c| c.replace(prev));
        completed_count
    }

    pub fn run_until_stalled(&mut self) {
        while !self.run_queue.is_empty() {
            self.run_single_tick();
        }
    }
}
