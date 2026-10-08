#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VarInt(pub u64);

pub const MAX_VARINT: u64 = (1 << 62) - 1;

impl VarInt {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1);

    #[inline(always)]
    pub fn from_u32(v: u32) -> Self {
        Self(v as u64)
    }

    #[inline(always)]
    pub fn from_u64(v: u64) -> Option<Self> {
        if v <= MAX_VARINT {
            Some(Self(v))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn encode(&self, out: &mut [u8]) -> usize {
        let val = self.0;
        if val < (1 << 6) {
            out[0] = val as u8;
            1
        } else if val < (1 << 14) {
            let b = ((val as u16) | 0x4000).to_be_bytes();
            out[0] = b[0];
            out[1] = b[1];
            2
        } else if val < (1 << 30) {
            let b = ((val as u32) | 0x80000000).to_be_bytes();
            out[..4].copy_from_slice(&b);
            4
        } else {
            let b = (val | 0xc000000000000000).to_be_bytes();
            out[..8].copy_from_slice(&b);
            8
        }
    }

    #[inline(always)]
    pub fn encode_vec(&self, out: &mut Vec<u8>) {
        let mut buf = [0u8; 8];
        let len = self.encode(&mut buf);
        out.extend_from_slice(&buf[..len]);
    }

    #[inline(always)]
    pub fn decode(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.is_empty() {
            return None;
        }
        let first = buf[0];
        let tag = first >> 6;
        match tag {
            0 => Some((Self(first as u64), 1)),
            1 => {
                if buf.len() < 2 {
                    return None;
                }
                let val = u16::from_be_bytes([first & 0x3f, buf[1]]) as u64;
                Some((Self(val), 2))
            }
            2 => {
                if buf.len() < 4 {
                    return None;
                }
                let val = u32::from_be_bytes([first & 0x3f, buf[1], buf[2], buf[3]]) as u64;
                Some((Self(val), 4))
            }
            3 => {
                if buf.len() < 8 {
                    return None;
                }
                let val = u64::from_be_bytes([
                    first & 0x3f, buf[1], buf[2], buf[3],
                    buf[4], buf[5], buf[6], buf[7],
                ]);
                Some((Self(val), 8))
            }
            _ => unreachable!(),
        }
    }

    #[inline(always)]
    pub fn size(&self) -> usize {
        let val = self.0;
        if val < (1 << 6) {
            1
        } else if val < (1 << 14) {
            2
        } else if val < (1 << 30) {
            4
        } else {
            8
        }
    }
}
