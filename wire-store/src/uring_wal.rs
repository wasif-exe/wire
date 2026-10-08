use std::fs::OpenOptions;
use std::io::{Error, ErrorKind, Result};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::ptr;
use io_uring::{opcode, types, IoUring};
use crate::crc::crc32c;
use crate::wal::{WalHeader, BUFFER_CAPACITY, OP_DEL, OP_SET, SECTOR_SIZE, WAL_MAGIC};

pub const SQ_ENTRIES: u32 = 256;

pub struct AsyncWalWriter {
    fd: RawFd,
    ring: IoUring,
    buf: *mut u8,
    buf_head: usize,
    flushed_offset: u64,
    seq: u64,
    in_flight_bytes: usize,
}

impl AsyncWalWriter {
    pub fn open(path: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(libc::O_DIRECT | libc::O_DSYNC)
            .open(path)?;

        let fd = file.as_raw_fd();
        std::mem::forget(file);

        let ring = match IoUring::builder()
            .setup_sqpoll(2000)
            .build(SQ_ENTRIES)
        {
            Ok(r) => r,
            Err(_) => IoUring::new(SQ_ENTRIES)?,
        };

        let mut aligned_ptr: *mut libc::c_void = ptr::null_mut();
        // SAFETY: Allocating sector-aligned memory buffer for Direct I/O writes.
        let ret = unsafe { libc::posix_memalign(&mut aligned_ptr, SECTOR_SIZE, BUFFER_CAPACITY) };
        if ret != 0 {
            // SAFETY: Clean up file descriptor on allocation failure.
            unsafe { libc::close(fd) };
            return Err(Error::from_raw_os_error(ret));
        }

        let iov = libc::iovec {
            iov_base: aligned_ptr,
            iov_len: BUFFER_CAPACITY,
        };
        // SAFETY: Registering fixed IO buffer with kernel io_uring reactor.
        unsafe {
            let _ = ring.submitter().register_buffers(&[iov]);
        }

        // SAFETY: Seeking to end of file to determine initial log offset.
        let file_len = unsafe { libc::lseek(fd, 0, libc::SEEK_END) };
        let flushed_offset = if file_len < 0 { 0 } else { file_len as u64 };

        Ok(Self {
            fd,
            ring,
            buf: aligned_ptr as *mut u8,
            buf_head: 0,
            flushed_offset,
            seq: 0,
            in_flight_bytes: 0,
        })
    }

    #[inline(always)]
    pub fn append_set(&mut self, key: &[u8], val: &[u8]) -> Result<u64> {
        self.append_record(OP_SET, key, val)
    }

    #[inline(always)]
    pub fn append_del(&mut self, key: &[u8]) -> Result<u64> {
        self.append_record(OP_DEL, key, &[])
    }

    pub fn append_record(&mut self, op: u8, key: &[u8], val: &[u8]) -> Result<u64> {
        let record_payload_len = key.len() + val.len();
        let total_record_len = std::mem::size_of::<WalHeader>() + record_payload_len;

        if self.buf_head + total_record_len > BUFFER_CAPACITY {
            self.submit_and_wait_flush()?;
        }

        if total_record_len > BUFFER_CAPACITY {
            return Err(Error::new(ErrorKind::InvalidInput, "Record exceeds buffer capacity"));
        }

        self.seq += 1;
        let seq = self.seq;

        let mut header = WalHeader {
            magic: WAL_MAGIC,
            crc: 0,
            seq,
            key_len: key.len() as u32,
            val_len: val.len() as u32,
            op,
            _pad: [0; 7],
        };

        let mut crc_payload = Vec::with_capacity(record_payload_len + 1);
        crc_payload.push(op);
        crc_payload.extend_from_slice(key);
        crc_payload.extend_from_slice(val);
        header.crc = crc32c(&crc_payload);

        let header_bytes = unsafe {
            // SAFETY: Transmuting packed POD header struct to immutable byte slice.
            std::slice::from_raw_parts(
                &header as *const WalHeader as *const u8,
                std::mem::size_of::<WalHeader>(),
            )
        };

        // SAFETY: Copying header and key/val payload directly into sector-aligned memory buffer.
        unsafe {
            let target_ptr = self.buf.add(self.buf_head);
            ptr::copy_nonoverlapping(header_bytes.as_ptr(), target_ptr, header_bytes.len());
            ptr::copy_nonoverlapping(key.as_ptr(), target_ptr.add(header_bytes.len()), key.len());
            ptr::copy_nonoverlapping(val.as_ptr(), target_ptr.add(header_bytes.len() + key.len()), val.len());
        }

        self.buf_head += total_record_len;

        if self.buf_head >= SECTOR_SIZE * 16 {
            self.submit_async_flush()?;
        }

        Ok(seq)
    }

    pub fn submit_async_flush(&mut self) -> Result<()> {
        let write_size = self.buf_head & !(SECTOR_SIZE - 1);
        if write_size == 0 {
            return Ok(());
        }

        self.reap_completions();

        let write_op = opcode::WriteFixed::new(
            types::Fd(self.fd),
            self.buf,
            write_size as u32,
            0,
        )
        .offset(self.flushed_offset)
        .build()
        .user_data(write_size as u64);

        // SAFETY: Pushing asynchronous SQE into kernel SQPOLL submission ring.
        unsafe {
            if self.ring.submission().push(&write_op).is_err() {
                self.submit_and_wait_flush()?;
                return Ok(());
            }
        }

        let _ = self.ring.submit();
        self.flushed_offset += write_size as u64;
        self.in_flight_bytes += write_size;

        let remaining = self.buf_head - write_size;
        if remaining > 0 {
            // SAFETY: Relocating trailing bytes to front of aligned buffer.
            unsafe {
                ptr::copy(self.buf.add(write_size), self.buf, remaining);
            }
        }
        self.buf_head = remaining;

        Ok(())
    }

    pub fn submit_and_wait_flush(&mut self) -> Result<()> {
        self.submit_async_flush()?;
        while self.in_flight_bytes > 0 {
            self.ring.submit_and_wait(1)?;
            self.reap_completions();
        }
        Ok(())
    }

    fn reap_completions(&mut self) {
        let mut completed_bytes = 0;
        for cqe in self.ring.completion() {
            let res = cqe.result();
            if res > 0 {
                completed_bytes += res as usize;
            }
        }
        self.in_flight_bytes = self.in_flight_bytes.saturating_sub(completed_bytes);
    }

    pub fn sync(&mut self) -> Result<()> {
        if self.buf_head > 0 {
            let pad_len = (SECTOR_SIZE - (self.buf_head % SECTOR_SIZE)) % SECTOR_SIZE;
            if pad_len > 0 {
                // SAFETY: Zeroing remainder of trailing sector for strict Direct I/O alignment.
                unsafe {
                    ptr::write_bytes(self.buf.add(self.buf_head), 0, pad_len);
                }
                self.buf_head += pad_len;
            }
            self.submit_async_flush()?;
        }

        self.submit_and_wait_flush()?;

        let sync_op = opcode::Fsync::new(types::Fd(self.fd)).build().user_data(0xfa57);
        // SAFETY: Synchronizing hardware NVMe disk caches via io_uring fsync.
        unsafe {
            let _ = self.ring.submission().push(&sync_op);
        }
        self.ring.submit_and_wait(1)?;
        self.reap_completions();

        Ok(())
    }
}

impl Drop for AsyncWalWriter {
    fn drop(&mut self) {
        let _ = self.sync();
        // SAFETY: Freeing sector-aligned buffer and closing file descriptor.
        unsafe {
            libc::free(self.buf as *mut libc::c_void);
            libc::close(self.fd);
        }
    }
}
