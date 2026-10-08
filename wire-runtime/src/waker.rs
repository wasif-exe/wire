use std::task::{RawWaker, RawWakerVTable, Waker};

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    clone_waker,
    wake_waker,
    wake_by_ref_waker,
    drop_waker,
);

unsafe fn clone_waker(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

unsafe fn wake_waker(data: *const ()) {
    wake_by_ref_waker(data);
}

unsafe fn wake_by_ref_waker(data: *const ()) {
    let task_id = data as usize;
    crate::executor::wake_task_by_id(task_id);
}

unsafe fn drop_waker(_data: *const ()) {}

pub fn create_waker(task_id: usize) -> Waker {
    let raw_waker = RawWaker::new(task_id as *const (), &VTABLE);
    // SAFETY: Constructing Waker from a statically defined valid RawWakerVTable.
    unsafe { Waker::from_raw(raw_waker) }
}
