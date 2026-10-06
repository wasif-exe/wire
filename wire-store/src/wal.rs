use std::fs::OpenOptions;
use std::io::{Error, ErrorKind, Read, Result};
use std::os::unix::io::RawFd;
use std::ptr;
use crate::crc::crc32c;

pub const SECTOR_SIZE: usize = 4096;
pub const BUFFER_CAPACITY: usize = 2 * 1024 * 1024;
pub const WAL_MAGIC: u32 = 0x57414C35;

pub const OP_SET: u8 = 1;
pub const OP_DEL: u8 = 2;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalHeader {
    pub magic: u32,
    pub crc: u32,
    pub seq: u64,
    pub key_len: u32,
    pub val_len: u32,
    pub op: u8,
    pub _pad: [u8; 7],
}

pub struct WalWriter {
    fd: RawFd,
    buf: *mut u8,
    buf_head: usize,
    flushed_offset: u64,
    seq: u64,
}

impl WalWriter {
    pub fn open(path: &str) -> Result<Self> {
        let c_path = std::ffi::CString::new(path).map_err(|e| Error::new(ErrorKind::InvalidInput, e))?;
        
        // SAFETY: System call opening file with O_DIRECT, O_SYNC, and O_RDWR.
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_DIRECT | libc::O_CREAT | libc::O_RDWR | libc::O_DSYNC,
                0o644,
            )
        };

        if fd < 0 {
            return Err(Error::last_os_error());
        }

        let mut aligned_ptr: *mut libc::c_void = ptr::null_mut();
        // SAFETY: Allocating sector-aligned memory buffer for Direct I/O writes.
        let ret = unsafe { libc::posix_memalign(&mut aligned_ptr, SECTOR_SIZE, BUFFER_CAPACITY) };
        if ret != 0 {
            // SAFETY: Clean up file descriptor on allocation failure.
            unsafe { libc::close(fd) };
            return Err(Error::from_raw_os_error(ret));
        }

        // SAFETY: Seeking to end of file to determine current file offset.
        let file_len = unsafe { libc::lseek(fd, 0, libc::SEEK_END) };
        let flushed_offset = if file_len < 0 { 0 } else { file_len as u64 };

        Ok(Self {
            fd,
            buf: aligned_ptr as *mut u8,
            buf_head: 0,
            flushed_offset,
            seq: 0,
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

    fn append_record(&mut self, op: u8, key: &[u8], val: &[u8]) -> Result<u64> {
        let record_payload_len = key.len() + val.len();
        let total_record_len = std::mem::size_of::<WalHeader>() + record_payload_len;

        if self.buf_head + total_record_len > BUFFER_CAPACITY {
            self.flush_aligned_sectors()?;
        }

        if total_record_len > BUFFER_CAPACITY {
            return Err(Error::new(ErrorKind::InvalidInput, "Record exceeds maximum buffer capacity"));
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

        // SAFETY: Copying header and payload directly into aligned Direct I/O memory buffer.
        unsafe {
            let target_ptr = self.buf.add(self.buf_head);
            ptr::copy_nonoverlapping(header_bytes.as_ptr(), target_ptr, header_bytes.len());
            ptr::copy_nonoverlapping(key.as_ptr(), target_ptr.add(header_bytes.len()), key.len());
            ptr::copy_nonoverlapping(val.as_ptr(), target_ptr.add(header_bytes.len() + key.len()), val.len());
        }

        self.buf_head += total_record_len;

        if self.buf_head >= SECTOR_SIZE * 16 {
            self.flush_aligned_sectors()?;
        }

        Ok(seq)
    }

    pub fn flush_aligned_sectors(&mut self) -> Result<()> {
        let write_size = self.buf_head & !(SECTOR_SIZE - 1);
        if write_size == 0 {
            return Ok(());
        }

        // SAFETY: Performing direct aligned write to storage descriptor via pwrite.
        let written = unsafe {
            libc::pwrite(
                self.fd,
                self.buf as *const libc::c_void,
                write_size,
                self.flushed_offset as libc::off_t,
            )
        };

        if written < 0 {
            return Err(Error::last_os_error());
        }

        let written_bytes = written as usize;
        self.flushed_offset += written_bytes as u64;

        let remaining = self.buf_head - written_bytes;
        if remaining > 0 {
            // SAFETY: Relocating unwritten unaligned residue to start of aligned buffer.
            unsafe {
                ptr::copy(self.buf.add(written_bytes), self.buf, remaining);
            }
        }
        self.buf_head = remaining;

        Ok(())
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

            // SAFETY: Writing remaining padded sector to disk.
            let written = unsafe {
                libc::pwrite(
                    self.fd,
                    self.buf as *const libc::c_void,
                    self.buf_head,
                    self.flushed_offset as libc::off_t,
                )
            };

            if written < 0 {
                return Err(Error::last_os_error());
            }

            self.flushed_offset += written as u64;
            self.buf_head = 0;
        }

        // SAFETY: Synchronizing hardware disk caches with storage device.
        let ret = unsafe { libc::fdatasync(self.fd) };
        if ret < 0 {
            return Err(Error::last_os_error());
        }

        Ok(())
    }
}

impl Drop for WalWriter {
    fn drop(&mut self) {
        let _ = self.sync();
        // SAFETY: Freeing aligned memory allocation and closing file descriptor.
        unsafe {
            libc::free(self.buf as *mut libc::c_void);
            libc::close(self.fd);
        }
    }
}

pub struct WalReader;

impl WalReader {
    pub fn replay<F>(path: &str, mut on_record: F) -> Result<u64>
    where
        F: FnMut(u8, &[u8], &[u8]) -> Result<()>,
    {
        let mut file = OpenOptions::new().read(true).open(path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let mut offset = 0;
        let mut last_seq = 0;
        let hdr_size = std::mem::size_of::<WalHeader>();

        while offset + hdr_size <= buffer.len() {
            let header = unsafe {
                // SAFETY: Reading packed POD WalHeader from buffer bounds.
                ptr::read_unaligned(buffer[offset..].as_ptr() as *const WalHeader)
            };

            if header.magic != WAL_MAGIC {
                break;
            }

            let total_len = hdr_size + header.key_len as usize + header.val_len as usize;
            if offset + total_len > buffer.len() {
                break;
            }

            let key_start = offset + hdr_size;
            let key_end = key_start + header.key_len as usize;
            let val_end = key_end + header.val_len as usize;

            let key = &buffer[key_start..key_end];
            let val = &buffer[key_end..val_end];

            let mut crc_payload = Vec::with_capacity(key.len() + val.len() + 1);
            crc_payload.push(header.op);
            crc_payload.extend_from_slice(key);
            crc_payload.extend_from_slice(val);

            if crc32c(&crc_payload) == header.crc {
                on_record(header.op, key, val)?;
                last_seq = header.seq;
                offset += total_len;
            } else {
                break;
            }
        }

        Ok(last_seq)
    }
}
