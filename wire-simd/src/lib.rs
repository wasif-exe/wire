pub mod masks;
pub mod scalar;
pub mod avx2;
pub mod avx512;
pub mod batch;

use wire_core::types::PackedTuple;
use wire_core::probes::{self, StageId};

#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedL4 {
    pub tuple: PackedTuple,
    pub payload_offset: u16,
    pub payload_len: u16,
    pub tcp_seq: u32,
    pub tcp_ack: u32,
    pub tcp_flags: u8,
    pub tcp_window: u16,
    pub valid: bool,
}

impl Default for ParsedL4 {
    #[inline(always)]
    fn default() -> Self {
        Self {
            tuple: PackedTuple {
                src_ip: [0; 4],
                dst_ip: [0; 4],
                src_port: 0,
                dst_port: 0,
                proto: 0,
                _pad: [0; 3],
            },
            payload_offset: 0,
            payload_len: 0,
            tcp_seq: 0,
            tcp_ack: 0,
            tcp_flags: 0,
            tcp_window: 0,
            valid: false,
        }
    }
}

pub fn parse_one(frame: &[u8]) -> ParsedL4 {
    let t_begin = probes::stage_begin(StageId::SimdParse);
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") && !cfg!(feature = "force_scalar") {
            // SAFETY: Safe because is_x86_feature_detected!("avx512f") asserts hardware capability.
            let res = unsafe { avx512::parse_one_avx512(frame) };
            probes::stage_end(StageId::SimdParse, t_begin);
            return res;
        }
        if is_x86_feature_detected!("avx2") && !cfg!(feature = "force_scalar") {
            // SAFETY: Safe because is_x86_feature_detected!("avx2") asserts hardware capability.
            let res = unsafe { avx2::parse_one_avx2(frame) };
            probes::stage_end(StageId::SimdParse, t_begin);
            return res;
        }
    }
    let res = scalar::parse_one_scalar(frame);
    probes::stage_end(StageId::SimdParse, t_begin);
    res
}

pub fn parse_batch_x8(frames: &[&[u8]; 8], out: &mut [ParsedL4; 8]) {
    let t_begin = probes::stage_begin(StageId::SimdParseBatch);
    batch::parse_batch_x8(frames, out);
    probes::stage_end(StageId::SimdParseBatch, t_begin);
}
