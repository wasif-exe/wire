use std::cell::UnsafeCell;
use std::ptr::NonNull;
use wire_disruptor::hugepage::{HugeAllocation, HugePageKind};

pub const CLASS_64: usize = 64;
pub const CLASS_128: usize = 128;
pub const CLASS_256: usize = 256;

const SLAB_64_CAPACITY: usize = 1024 * 1024;
const SLAB_128_CAPACITY: usize = 512 * 1024;
const SLAB_256_CAPACITY: usize = 256 * 1024;

pub struct FixedSlab<const BLOCK_SIZE: usize> {
    buffer: HugeAllocation<u8>,
    bump_offset: usize,
    free_head: u32,
    capacity_blocks: usize,
}

const NULL_INDEX: u32 = u32::MAX;

impl<const BLOCK_SIZE: usize> FixedSlab<BLOCK_SIZE> {
    pub fn new(capacity_blocks: usize) -> Self {
        let byte_size = capacity_blocks * BLOCK_SIZE;
        let buffer = HugeAllocation::<u8>::alloc(byte_size)
            .expect("failed to allocate hugepage slab memory");
        Self {
            buffer,
            bump_offset: 0,
            free_head: NULL_INDEX,
            capacity_blocks,
        }
    }

    #[inline(always)]
    pub fn alloc(&mut self) -> *mut u8 {
        if self.free_head != NULL_INDEX {
            let idx = self.free_head as usize;
            let offset = idx * BLOCK_SIZE;
            // SAFETY: idx was validated on free and offset is within buffer allocation.
            let ptr = unsafe { self.buffer.ptr.add(offset) };
            // SAFETY: ptr points to valid allocated slab block storing next free index.
            self.free_head = unsafe { *(ptr as *const u32) };
            return ptr;
        }

        if self.bump_offset < self.capacity_blocks {
            let offset = self.bump_offset * BLOCK_SIZE;
            self.bump_offset += 1;
            // SAFETY: offset is strictly less than allocated capacity * BLOCK_SIZE.
            unsafe { self.buffer.ptr.add(offset) }
        } else {
            std::ptr::null_mut()
        }
    }

    #[inline(always)]
    pub fn dealloc(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        let base = self.buffer.ptr as usize;
        let p = ptr as usize;
        if p < base {
            return;
        }
        let diff = p - base;
        let idx = diff / BLOCK_SIZE;
        if idx >= self.capacity_blocks || (diff % BLOCK_SIZE) != 0 {
            return;
        }
        // SAFETY: ptr is verified to lie inside slab bounds and be block-aligned.
        unsafe {
            *(ptr as *mut u32) = self.free_head;
        }
        self.free_head = idx as u32;
    }

    #[inline(always)]
    pub fn owns(&self, ptr: *const u8) -> bool {
        let base = self.buffer.ptr as usize;
        let p = ptr as usize;
        let end = base + self.capacity_blocks * BLOCK_SIZE;
        p >= base && p < end
    }

    pub fn page_kind(&self) -> HugePageKind {
        self.buffer.kind
    }
}

pub struct HugeSlabAllocator {
    slab_64: FixedSlab<CLASS_64>,
    slab_128: FixedSlab<CLASS_128>,
    slab_256: FixedSlab<CLASS_256>,
}

impl HugeSlabAllocator {
    pub fn new() -> Self {
        Self {
            slab_64: FixedSlab::new(SLAB_64_CAPACITY),
            slab_128: FixedSlab::new(SLAB_128_CAPACITY),
            slab_256: FixedSlab::new(SLAB_256_CAPACITY),
        }
    }

    #[inline(always)]
    pub fn alloc_64(&mut self) -> *mut u8 {
        self.slab_64.alloc()
    }

    #[inline(always)]
    pub fn dealloc_64(&mut self, ptr: *mut u8) {
        self.slab_64.dealloc(ptr);
    }

    #[inline(always)]
    pub fn alloc_128(&mut self) -> *mut u8 {
        self.slab_128.alloc()
    }

    #[inline(always)]
    pub fn dealloc_128(&mut self, ptr: *mut u8) {
        self.slab_128.dealloc(ptr);
    }

    #[inline(always)]
    pub fn alloc_256(&mut self) -> *mut u8 {
        self.slab_256.alloc()
    }

    #[inline(always)]
    pub fn dealloc_256(&mut self, ptr: *mut u8) {
        self.slab_256.dealloc(ptr);
    }

    #[inline(always)]
    pub fn alloc_fit(&mut self, size: usize) -> *mut u8 {
        if size <= CLASS_64 {
            self.slab_64.alloc()
        } else if size <= CLASS_128 {
            self.slab_128.alloc()
        } else if size <= CLASS_256 {
            self.slab_256.alloc()
        } else {
            std::ptr::null_mut()
        }
    }

    #[inline(always)]
    pub fn dealloc_fit(&mut self, ptr: *mut u8, size: usize) {
        if size <= CLASS_64 {
            self.slab_64.dealloc(ptr);
        } else if size <= CLASS_128 {
            self.slab_128.dealloc(ptr);
        } else if size <= CLASS_256 {
            self.slab_256.dealloc(ptr);
        }
    }

    #[inline(always)]
    pub fn owns_64(&self, ptr: *const u8) -> bool {
        self.slab_64.owns(ptr)
    }

    #[inline(always)]
    pub fn owns_128(&self, ptr: *const u8) -> bool {
        self.slab_128.owns(ptr)
    }

    #[inline(always)]
    pub fn owns_256(&self, ptr: *const u8) -> bool {
        self.slab_256.owns(ptr)
    }

    pub fn page_kind(&self) -> HugePageKind {
        self.slab_64.page_kind()
    }
}

impl Default for HugeSlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static LOCAL_SLAB: UnsafeCell<HugeSlabAllocator> = UnsafeCell::new(HugeSlabAllocator::new());
}

pub struct ThreadSlab;

impl ThreadSlab {
    #[inline(always)]
    pub fn alloc_64() -> *mut u8 {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).alloc_64() }
        })
    }

    #[inline(always)]
    pub fn dealloc_64(ptr: *mut u8) {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).dealloc_64(ptr) }
        })
    }

    #[inline(always)]
    pub fn alloc_128() -> *mut u8 {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).alloc_128() }
        })
    }

    #[inline(always)]
    pub fn dealloc_128(ptr: *mut u8) {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).dealloc_128(ptr) }
        })
    }

    #[inline(always)]
    pub fn alloc_256() -> *mut u8 {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).alloc_256() }
        })
    }

    #[inline(always)]
    pub fn dealloc_256(ptr: *mut u8) {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).dealloc_256(ptr) }
        })
    }

    #[inline(always)]
    pub fn alloc_fit(size: usize) -> *mut u8 {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).alloc_fit(size) }
        })
    }

    #[inline(always)]
    pub fn dealloc_fit(ptr: *mut u8, size: usize) {
        LOCAL_SLAB.with(|cell| {
            // SAFETY: Thread-local UnsafeCell accessed exclusively by the current thread without reentrancy.
            unsafe { (*cell.get()).dealloc_fit(ptr, size) }
        })
    }
}

pub struct SlabBox<T> {
    ptr: NonNull<T>,
    size: usize,
}

impl<T> SlabBox<T> {
    pub fn new(val: T) -> Option<Self> {
        let size = std::mem::size_of::<T>();
        if size > CLASS_256 {
            return None;
        }
        let raw = ThreadSlab::alloc_fit(size);
        if raw.is_null() {
            return None;
        }
        let typed_ptr = raw as *mut T;
        // SAFETY: raw is non-null, properly aligned, and large enough to hold T.
        unsafe {
            typed_ptr.write(val);
        }
        Some(Self {
            ptr: NonNull::new(typed_ptr).unwrap(),
            size,
        })
    }
}

impl<T> std::ops::Deref for SlabBox<T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        // SAFETY: self.ptr is non-null and points to an active T instance.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> std::ops::DerefMut for SlabBox<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: self.ptr is non-null and points to an active mutable T instance.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for SlabBox<T> {
    fn drop(&mut self) {
        // SAFETY: self.ptr is non-null and drops the contained T before deallocating the memory block.
        unsafe {
            std::ptr::drop_in_place(self.ptr.as_ptr());
            ThreadSlab::dealloc_fit(self.ptr.as_ptr() as *mut u8, self.size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_cycle_64() {
        let mut slab = HugeSlabAllocator::new();
        let p1 = slab.alloc_64();
        assert!(!p1.is_null());
        // SAFETY: p1 points to 64 bytes of valid slab memory.
        unsafe {
            p1.write_bytes(0xAA, CLASS_64);
        }
        let p2 = slab.alloc_64();
        assert!(!p2.is_null());
        assert_ne!(p1, p2);
        slab.dealloc_64(p1);
        let p3 = slab.alloc_64();
        assert_eq!(p1, p3);
        slab.dealloc_64(p2);
        slab.dealloc_64(p3);
    }

    #[test]
    fn alloc_free_cycle_128_256() {
        let mut slab = HugeSlabAllocator::new();
        let p128 = slab.alloc_128();
        let p256 = slab.alloc_256();
        assert!(!p128.is_null());
        assert!(!p256.is_null());
        // SAFETY: Valid slab block memory writes.
        unsafe {
            p128.write_bytes(0xBB, CLASS_128);
            p256.write_bytes(0xCC, CLASS_256);
        }
        slab.dealloc_128(p128);
        slab.dealloc_256(p256);
    }

    #[test]
    fn thread_slab_box() {
        #[repr(C, align(64))]
        struct AlignedEvent {
            seq: u64,
            price: u32,
            qty: u32,
            _pad: [u8; 48],
        }
        let b = SlabBox::new(AlignedEvent {
            seq: 42,
            price: 18500,
            qty: 100,
            _pad: [0; 48],
        });
        assert!(b.is_some());
        let boxed = b.unwrap();
        assert_eq!(boxed.seq, 42);
        assert_eq!(boxed.price, 18500);
        assert_eq!(boxed.qty, 100);
    }

    #[test]
    fn slab_bounds_check() {
        let slab = FixedSlab::<64>::new(1024);
        let outside = 0x1234usize as *const u8;
        assert!(!slab.owns(outside));
    }
}
