use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf;

pub const INITIAL_SALT_V1: [u8; 20] = [
    0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3,
    0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
    0xcc, 0xbb, 0x7f, 0x0a,
];

pub const AES_GCM_TAG_LEN: usize = 16;

pub struct QuicKeys {
    pub local: DirectionKeys,
    pub remote: DirectionKeys,
}

pub struct DirectionKeys {
    pub key: LessSafeKey,
    pub iv: [u8; 12],
    pub hp_key: Aes128,
}

struct HkdfLabelLen(usize);

impl hkdf::KeyType for HkdfLabelLen {
    fn len(&self) -> usize {
        self.0
    }
}

pub fn hkdf_expand_label(
    prk: &hkdf::Prk,
    label: &[u8],
    context: &[u8],
    out_len: usize,
) -> Vec<u8> {
    let mut label_vec = Vec::with_capacity(10 + label.len() + context.len());
    label_vec.extend_from_slice(&(out_len as u16).to_be_bytes());
    let prefix = b"tls13 ";
    let full_label_len = prefix.len() + label.len();
    label_vec.push(full_label_len as u8);
    label_vec.extend_from_slice(prefix);
    label_vec.extend_from_slice(label);
    label_vec.push(context.len() as u8);
    label_vec.extend_from_slice(context);

    let mut out = vec![0u8; out_len];
    let info = [label_vec.as_slice()];
    prk.expand(&info, HkdfLabelLen(out_len))
        .unwrap()
        .fill(&mut out)
        .unwrap();
    out
}

pub fn derive_initial_keys(client_dst_cid: &[u8], is_server: bool) -> QuicKeys {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &INITIAL_SALT_V1);
    let initial_secret = salt.extract(client_dst_cid);

    let client_secret_bytes = hkdf_expand_label(&initial_secret, b"client in", b"", 32);
    let server_secret_bytes = hkdf_expand_label(&initial_secret, b"server in", b"", 32);

    let client_prk = hkdf::Prk::new_less_safe(hkdf::HKDF_SHA256, &client_secret_bytes);
    let server_prk = hkdf::Prk::new_less_safe(hkdf::HKDF_SHA256, &server_secret_bytes);

    let client_keys = derive_direction_keys(&client_prk);
    let server_keys = derive_direction_keys(&server_prk);

    if is_server {
        QuicKeys {
            local: server_keys,
            remote: client_keys,
        }
    } else {
        QuicKeys {
            local: client_keys,
            remote: server_keys,
        }
    }
}

fn derive_direction_keys(prk: &hkdf::Prk) -> DirectionKeys {
    let key_bytes = hkdf_expand_label(prk, b"quic key", b"", 16);
    let iv_bytes = hkdf_expand_label(prk, b"quic iv", b"", 12);
    let hp_bytes = hkdf_expand_label(prk, b"quic hp", b"", 16);

    let unbound_key = UnboundKey::new(&AES_128_GCM, &key_bytes).unwrap();
    let key = LessSafeKey::new(unbound_key);

    let mut iv = [0u8; 12];
    iv.copy_from_slice(&iv_bytes);

    let mut hp_arr = [0u8; 16];
    hp_arr.copy_from_slice(&hp_bytes);
    let hp_key = Aes128::new(&hp_arr);

    DirectionKeys { key, iv, hp_key }
}

impl DirectionKeys {
    pub fn encrypt_payload(
        &self,
        packet_number: u64,
        header: &[u8],
        payload: &mut Vec<u8>,
    ) {
        let mut nonce_bytes = self.iv;
        let pn_bytes = packet_number.to_be_bytes();
        for i in 0..8 {
            nonce_bytes[12 - 8 + i] ^= pn_bytes[i];
        }
        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes).unwrap();
        let aad = Aad::from(header);
        let tag = self.key.seal_in_place_separate_tag(nonce, aad, payload).unwrap();
        payload.extend_from_slice(tag.as_ref());
    }

    pub fn decrypt_payload(
        &self,
        packet_number: u64,
        header: &[u8],
        payload_and_tag: &mut [u8],
    ) -> Result<usize, ()> {
        if payload_and_tag.len() < AES_GCM_TAG_LEN {
            return Err(());
        }
        let mut nonce_bytes = self.iv;
        let pn_bytes = packet_number.to_be_bytes();
        for i in 0..8 {
            nonce_bytes[12 - 8 + i] ^= pn_bytes[i];
        }
        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes).unwrap();
        let aad = Aad::from(header);
        let plain_slice = self.key.open_in_place(nonce, aad, payload_and_tag).map_err(|_| ())?;
        Ok(plain_slice.len())
    }

    pub fn apply_header_protection(&self, sample: &[u8; 16], first_byte: &mut u8, pn_bytes: &mut [u8]) {
        let mask = self.hp_key.encrypt_block(sample);
        let is_long_header = (*first_byte & 0x80) != 0;
        if is_long_header {
            *first_byte ^= mask[0] & 0x0f;
        } else {
            *first_byte ^= mask[0] & 0x1f;
        }
        let pn_len = pn_bytes.len();
        for (b, m) in pn_bytes.iter_mut().zip(&mask[1..=pn_len]) {
            *b ^= *m;
        }
    }

    pub fn remove_header_protection(&self, sample: &[u8; 16], first_byte: &mut u8, pn_bytes: &mut [u8]) {
        self.apply_header_protection(sample, first_byte, pn_bytes);
    }
}

#[derive(Clone, Copy)]
pub struct Aes128 {
    round_keys: [u32; 44],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Self {
        let mut rk = [0u32; 44];
        for i in 0..4 {
            rk[i] = u32::from_be_bytes([
                key[4 * i],
                key[4 * i + 1],
                key[4 * i + 2],
                key[4 * i + 3],
            ]);
        }
        let rcon: [u32; 10] = [
            0x01000000, 0x02000000, 0x04000000, 0x08000000, 0x10000000,
            0x20000000, 0x40000000, 0x80000000, 0x1b000000, 0x36000000,
        ];
        for i in 4..44 {
            let mut temp = rk[i - 1];
            if i % 4 == 0 {
                temp = Self::sub_word(temp.rotate_left(8)) ^ rcon[(i / 4) - 1];
            }
            rk[i] = rk[i - 4] ^ temp;
        }
        Self { round_keys: rk }
    }

    #[inline(always)]
    fn sub_word(w: u32) -> u32 {
        let b = w.to_be_bytes();
        u32::from_be_bytes([SBOX[b[0] as usize], SBOX[b[1] as usize], SBOX[b[2] as usize], SBOX[b[3] as usize]])
    }

    pub fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let mut s = [0u8; 16];
        s.copy_from_slice(block);
        self.add_round_key(&mut s, 0);

        for r in 1..10 {
            self.sub_bytes(&mut s);
            self.shift_rows(&mut s);
            self.mix_columns(&mut s);
            self.add_round_key(&mut s, r);
        }

        self.sub_bytes(&mut s);
        self.shift_rows(&mut s);
        self.add_round_key(&mut s, 10);
        s
    }

    #[inline(always)]
    fn add_round_key(&self, s: &mut [u8; 16], round: usize) {
        let rk_bytes = [
            self.round_keys[round * 4].to_be_bytes(),
            self.round_keys[round * 4 + 1].to_be_bytes(),
            self.round_keys[round * 4 + 2].to_be_bytes(),
            self.round_keys[round * 4 + 3].to_be_bytes(),
        ];
        for i in 0..4 {
            for j in 0..4 {
                s[i * 4 + j] ^= rk_bytes[i][j];
            }
        }
    }

    #[inline(always)]
    fn sub_bytes(&self, s: &mut [u8; 16]) {
        for b in s.iter_mut() {
            *b = SBOX[*b as usize];
        }
    }

    #[inline(always)]
    fn shift_rows(&self, s: &mut [u8; 16]) {
        let t = *s;
        s[1] = t[5];
        s[5] = t[9];
        s[9] = t[13];
        s[13] = t[1];

        s[2] = t[10];
        s[6] = t[14];
        s[10] = t[2];
        s[14] = t[6];

        s[3] = t[15];
        s[7] = t[3];
        s[11] = t[7];
        s[15] = t[11];
    }

    #[inline(always)]
    fn mix_columns(&self, s: &mut [u8; 16]) {
        for c in 0..4 {
            let i = c * 4;
            let a0 = s[i];
            let a1 = s[i + 1];
            let a2 = s[i + 2];
            let a3 = s[i + 3];

            s[i] = Self::xt(a0) ^ Self::xt(a1) ^ a1 ^ a2 ^ a3;
            s[i + 1] = a0 ^ Self::xt(a1) ^ Self::xt(a2) ^ a2 ^ a3;
            s[i + 2] = a0 ^ a1 ^ Self::xt(a2) ^ Self::xt(a3) ^ a3;
            s[i + 3] = Self::xt(a0) ^ a0 ^ a1 ^ a2 ^ Self::xt(a3);
        }
    }

    #[inline(always)]
    fn xt(b: u8) -> u8 {
        if (b & 0x80) != 0 {
            (b << 1) ^ 0x1b
        } else {
            b << 1
        }
    }
}

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5e, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];
