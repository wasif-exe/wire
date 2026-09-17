use std::fs::OpenOptions;
use std::io::{Read, Write, Result};
use std::os::unix::io::AsRawFd;

#[repr(C)]
struct IfReq {
    ifr_name: [u8; 16],
    ifr_flags: u16,
    _pad: [u8; 22],
}

const IFF_TAP: u16 = 0x0002;
const IFF_NO_PI: u16 = 0x1000;
const TUNSETIFF: libc::c_ulong = 0x400454ca;

pub struct TapDevice {
    file: std::fs::File,
}

impl TapDevice {
    pub fn new(name: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/net/tun")?;

        let mut req = IfReq {
            ifr_name: [0; 16],
            ifr_flags: IFF_TAP | IFF_NO_PI,
            _pad: [0; 22],
        };
        
        let bytes = name.as_bytes();
        let len = bytes.len().min(15);
        req.ifr_name[..len].copy_from_slice(&bytes[..len]);

        let ret = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF, &mut req) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self { file })
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.file.read(buf)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.file.write(buf)
    }
}
