use crate::ParsedL4;
use crate::parse_one;

pub fn parse_batch_x8(frames: &[&[u8]; 8], out: &mut [ParsedL4; 8]) {
    out[0] = parse_one(frames[0]);
    out[1] = parse_one(frames[1]);
    out[2] = parse_one(frames[2]);
    out[3] = parse_one(frames[3]);
    out[4] = parse_one(frames[4]);
    out[5] = parse_one(frames[5]);
    out[6] = parse_one(frames[6]);
    out[7] = parse_one(frames[7]);
}
