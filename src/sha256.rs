//! SHA-256 (FIPS 180-4), hand-written and shared.
//!
//! In tree rather than as a dependency for the reason `store::fnv1a` is:
//! these digests NAME persistent entries — a description cache key and an
//! eval state file's sidecar-drift check — so the function has to be a fixed
//! function of the bytes for ever, and `Cargo.toml` justifies every direct
//! dependency and gates the list in `check_docs.py`. `sha2` is not in
//! `Cargo.lock` even transitively, so promoting it would be a genuine new
//! download rather than the "already locked, promoted to direct" pattern
//! `md5` and `brotli-decompressor` follow.
//!
//! It lives in its own module because it belongs to NEITHER caller. It was
//! written twice — a streaming form in `describe` and a whole-message form in
//! `eval`, each with its own copy of the round constants and its own
//! known-answer test — because the second author re-derived the
//! "do not add sha2" argument instead of grepping for `0x428a2f98`. The
//! streaming shape is the one that survived: it is a strict superset (a
//! single `update` then `finish` IS the whole-message form) and it is what
//! lets a frame be read in chunks rather than slurped.
//!
//! [`sha256_hex`] is pinned against the standard's own published digests by
//! `sha256_matches_the_fips_180_4_vectors`, so this is a checked
//! implementation rather than a claimed one.

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Streaming SHA-256 — one block at a time, so a 4 GB frame would cost 64
/// bytes of state rather than its own length in RAM. (The frames this hashes
/// are ~200 KB; the streaming shape is what lets [`frame_digest`] read the
/// file in chunks rather than slurping it.)
pub(crate) struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buffered: usize,
    len_bits: u64,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buffered: 0,
            len_bits: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.len_bits = self.len_bits.wrapping_add((data.len() as u64) * 8);
        while !data.is_empty() {
            let take = (64 - self.buffered).min(data.len());
            self.buf[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buffered = 0;
            }
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in self.h.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(v);
        }
    }

    pub(crate) fn finish(mut self) -> String {
        let bits = self.len_bits;
        self.update(&[0x80]);
        // `update` above counted the padding byte; the length field must be
        // the MESSAGE length, so it is captured before padding starts.
        while self.buffered != 56 {
            self.update(&[0x00]);
        }
        let len = bits.to_be_bytes();
        self.update(&len);
        let mut out = String::with_capacity(64);
        for word in self.h {
            out.push_str(&format!("{word:08x}"));
        }
        out
    }
}

/// The lowercase 64-hex SHA-256 of a byte string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut s = Sha256::new();
    s.update(bytes);
    s.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-4's own published digests, plus the empty string. Without
    /// these the hand-written compression function would be a CLAIM; with
    /// them it is a checked implementation, and a typo in the 64-entry K table
    /// (or in the padding, which is where hand-written SHA-256 usually goes
    /// wrong) fails here rather than silently re-keying every cache entry.
    #[test]
    fn sha256_matches_the_fips_180_4_vectors() {
        for (msg, want) in [
            ("", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            ("abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(sha256_hex(msg.as_bytes()), want, "sha256({msg:?})");
        }
        // The multi-block + length-field path: 1,000,000 'a' is the standard's
        // long vector, and it is the one that catches a length counted in
        // BYTES instead of bits.
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
}
