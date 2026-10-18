//! SHA-256 (FIPS 180-4), in the crate so that the core adds no
//! dependency (design `docs/research/m3-socket.md`, R21). Documents name
//! the file they were read from by its digest, never by its path, and a
//! socket document guards its extracted values with one.

/// The first 32 bits of the fractional parts of the cube roots of the
/// first 64 primes.
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// The first 32 bits of the fractional parts of the square roots of the
/// first 8 primes.
const H0: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// Folds one 64-byte block into the state.
// The names are FIPS 180-4's, so the code can be read against it.
#[allow(clippy::many_single_char_names)]
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
    for t in 16..64 {
        let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
        let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
        w[t] = w[t - 16]
            .wrapping_add(s0)
            .wrapping_add(w[t - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (k, w) in K.iter().zip(w) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(choose)
            .wrapping_add(*k)
            .wrapping_add(w);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *word = word.wrapping_add(add);
    }
}

/// An incremental SHA-256 (docs/research/m6-api.md, CO1): bytes fed in
/// pieces of any size give the digest of their concatenation, through the
/// same compression function as [`sha256`], so every content name in
/// fitment (blobs, objects, uploads, documents, tokens' stored hashes)
/// comes from one implementation.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    /// The bytes of a block not yet full.
    block: [u8; 64],
    /// How many of `block` are filled: always below 64.
    filled: usize,
    /// Every byte fed, modulo 2⁶⁴ (FIPS 180-4 counts bits modulo 2⁶⁴).
    length: u64,
}

impl std::fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sha256")
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// A hash of nothing yet.
    pub fn new() -> Self {
        Self {
            state: H0,
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    /// Feeds `bytes`: each block folds in as soon as it is full, so the
    /// hasher holds at most 63 bytes.
    pub fn update(&mut self, mut bytes: &[u8]) {
        // A slice's length fits in u64 on every target Rust supports.
        self.length = self.length.wrapping_add(bytes.len() as u64);
        if self.filled > 0 {
            let take = (64 - self.filled).min(bytes.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled < 64 {
                return;
            }
            let block = self.block;
            compress(&mut self.state, &block);
            self.filled = 0;
        }
        let mut blocks = bytes.chunks_exact(64);
        for block in blocks.by_ref() {
            let mut full = [0u8; 64];
            full.copy_from_slice(block);
            compress(&mut self.state, &full);
        }
        let rest = blocks.remainder();
        self.block[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }

    /// The digest of everything fed: the rest, a 1 bit, zeros and the
    /// length in bits, in one block or two.
    pub fn finish(mut self) -> [u8; 32] {
        let mut tail = [0u8; 128];
        tail[..self.filled].copy_from_slice(&self.block[..self.filled]);
        tail[self.filled] = 0x80;
        let end = if self.filled < 56 { 64 } else { 128 };
        let bits = self.length.wrapping_mul(8);
        tail[end - 8..end].copy_from_slice(&bits.to_be_bytes());
        for block in tail[..end].chunks_exact(64) {
            let mut full = [0u8; 64];
            full.copy_from_slice(block);
            compress(&mut self.state, &full);
        }
        let mut digest = [0u8; 32];
        for (out, word) in digest.chunks_exact_mut(4).zip(self.state) {
            out.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    /// [`Sha256::finish`] as 64 lower-case hexadecimal digits.
    pub fn finish_hex(self) -> String {
        hex(&self.finish())
    }
}

/// `digest` as lower-case hexadecimal digits.
fn hex(digest: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// The SHA-256 digest of `bytes`: [`Sha256`] fed once.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finish()
}

/// The SHA-256 digest of `bytes` as 64 lower-case hexadecimal digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

#[cfg(test)]
mod tests {
    use super::{H0, Sha256, compress, sha256, sha256_hex};

    /// The one-shot function as it was before CO1, kept as the oracle the
    /// incremental hasher is held to.
    fn one_shot(bytes: &[u8]) -> [u8; 32] {
        let mut state = H0;
        let mut blocks = bytes.chunks_exact(64);
        for block in blocks.by_ref() {
            let mut full = [0u8; 64];
            full.copy_from_slice(block);
            compress(&mut state, &full);
        }
        let rest = blocks.remainder();
        let mut tail = [0u8; 128];
        tail[..rest.len()].copy_from_slice(rest);
        tail[rest.len()] = 0x80;
        let end = if rest.len() < 56 { 64 } else { 128 };
        let bits = (bytes.len() as u64).wrapping_mul(8);
        tail[end - 8..end].copy_from_slice(&bits.to_be_bytes());
        for block in tail[..end].chunks_exact(64) {
            let mut full = [0u8; 64];
            full.copy_from_slice(block);
            compress(&mut state, &full);
        }
        let mut digest = [0u8; 32];
        for (out, word) in digest.chunks_exact_mut(4).zip(state) {
            out.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    /// Bytes that vary in every position, so a piece fed twice or out of
    /// order changes the digest.
    fn message(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| u8::try_from((i * 131 + 7) % 251).unwrap())
            .collect()
    }

    /// CO1, VT37: fed in two pieces split at every point, and in three at
    /// every pair of points, messages of 0 to 200 bytes give the one-shot
    /// digest, and so does the wrapper.
    #[test]
    fn every_split_is_the_one_shot_digest() {
        for n in 0..=200 {
            let bytes = message(n);
            let want = one_shot(&bytes);
            assert_eq!(sha256(&bytes), want, "{n} bytes, one piece");
            for k in 0..=n {
                let mut hasher = Sha256::new();
                hasher.update(&bytes[..k]);
                hasher.update(&bytes[k..]);
                assert_eq!(hasher.finish(), want, "{n} bytes split at {k}");
            }
            if n <= 140 {
                for a in 0..=n {
                    for b in a..=n {
                        let mut hasher = Sha256::new();
                        hasher.update(&bytes[..a]);
                        hasher.update(&bytes[a..b]);
                        hasher.update(&bytes[b..]);
                        assert_eq!(hasher.finish(), want, "{n} bytes split at {a} and {b}");
                    }
                }
            }
        }
    }

    /// CO1, VT37: byte by byte, and in uneven pieces of a long message,
    /// the standard vectors and the one-shot digest; a clone hashes on
    /// alone.
    #[test]
    fn pieces_of_any_size_give_the_standard_digests() {
        let mut hasher = Sha256::new();
        for byte in b"abc" {
            hasher.update(&[*byte]);
        }
        assert_eq!(
            hasher.finish_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let million = vec![b'a'; 1_000_000];
        let mut hasher = Sha256::new();
        for piece in million.chunks(997) {
            hasher.update(piece);
        }
        assert_eq!(
            hasher.finish_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        let bytes = message(10_000);
        let mut hasher = Sha256::new();
        let mut at = 0;
        for size in (1..).map(|i| (i * 37) % 600) {
            if at >= bytes.len() {
                break;
            }
            let end = (at + size).min(bytes.len());
            hasher.update(&bytes[at..end]);
            at = end;
        }
        let fork = hasher.clone();
        assert_eq!(hasher.finish(), one_shot(&bytes));
        let mut fork = fork;
        fork.update(b"more");
        let mut longer = bytes.clone();
        longer.extend_from_slice(b"more");
        assert_eq!(fork.finish(), one_shot(&longer));
        assert_eq!(Sha256::default().finish_hex(), sha256_hex(b""));
    }

    // FIPS 180-4's examples (NIST CSRC, "Example Algorithms") and the
    // NIST byte-oriented test vectors.
    #[test]
    fn the_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            sha256_hex(
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
            ),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
        );
        assert_eq!(
            sha256_hex(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    // Every length around the block and padding boundaries goes through a
    // path of its own; the digests are those of `shasum -a 256`.
    #[test]
    fn lengths_around_the_padding_boundary() {
        let digests = [
            (
                55,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                63,
                "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34",
            ),
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                65,
                "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
            ),
        ];
        for (n, digest) in digests {
            assert_eq!(sha256_hex(&vec![b'a'; n]), digest, "{n} bytes");
        }
    }
}
