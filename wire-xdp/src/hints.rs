pub const XSK_UNALIGNED_BUF_OFFSET_SHIFT: u64 = 16;
pub const XDP_FLAGS_CSUM_UNNECESSARY: u32 = 1 << 0;
pub const XDP_FLAGS_RX_HASH_PRESENT: u32 = 1 << 1;
pub const XDP_FLAGS_TIMESTAMP_PRESENT: u32 = 1 << 2;

#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NicRxMetadata {
    pub rx_hash: u32,
    pub flags: u32,
    pub timestamp_ns: u64,
    pub vlan_tci: u16,
    pub csum_verdict: u16,
    pub _pad: [u8; 8],
}

impl NicRxMetadata {
    #[inline(always)]
    pub fn checksum_valid(&self) -> bool {
        (self.flags & XDP_FLAGS_CSUM_UNNECESSARY) != 0 || self.csum_verdict == 1
    }

    #[inline(always)]
    pub fn has_rx_hash(&self) -> bool {
        (self.flags & XDP_FLAGS_RX_HASH_PRESENT) != 0
    }

    #[inline(always)]
    pub fn has_timestamp(&self) -> bool {
        (self.flags & XDP_FLAGS_TIMESTAMP_PRESENT) != 0
    }
}

#[inline(always)]
pub unsafe fn read_nic_hints_from_headroom(packet_ptr: *const u8, headroom: usize) -> NicRxMetadata {
    if headroom < std::mem::size_of::<NicRxMetadata>() {
        return NicRxMetadata::default();
    }
    // SAFETY: Reading NicRxMetadata struct packed in packet headroom populated by SmartNIC XDP driver.
    let meta_ptr = packet_ptr.sub(std::mem::size_of::<NicRxMetadata>()) as *const NicRxMetadata;
    std::ptr::read_unaligned(meta_ptr)
}
