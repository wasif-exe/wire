#[inline(always)]
pub fn crc32c(data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse4.2") {
            // SAFETY: Safe because is_x86_feature_detected!("sse4.2") validates CPU support.
            return unsafe { crc32c_hardware(data) };
        }
    }
    crc32c_software(data)
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn crc32c_hardware(data: &[u8]) -> u32 {
    use core::arch::x86_64::*;
    let mut crc = !0u32;
    let mut ptr = data.as_ptr();
    let mut len = data.len();

    while len >= 8 {
        let val = (ptr as *const u64).read_unaligned();
        crc = _mm_crc32_u64(crc as u64, val) as u32;
        ptr = ptr.add(8);
        len -= 8;
    }

    if len >= 4 {
        let val = (ptr as *const u32).read_unaligned();
        crc = _mm_crc32_u32(crc, val);
        ptr = ptr.add(4);
        len -= 4;
    }

    while len > 0 {
        crc = _mm_crc32_u8(crc, *ptr);
        ptr = ptr.add(1);
        len -= 1;
    }

    !crc
}

fn crc32c_software(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0x82F63B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
