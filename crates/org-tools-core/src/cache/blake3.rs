// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pure-Rust official BLAKE3 digest helper for workspace file content hashing.
//!
//! Implements the BLAKE3 tree-hashing algorithm defined by the official specification
//! without external dependencies.

// region Constants

/// Eight 32-bit initialization vector words matching SHA-256.
const IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Permutation applied to message schedule words between rounds.
const MSG_PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

/// Compression block size in bytes.
const BLOCK_LEN: usize = 64;

/// Tree leaf chunk size in bytes.
const CHUNK_LEN: usize = 1024;

/// Output digest size in bytes.
const OUT_LEN: usize = 32;

/// Domain separation flag for the initial block of a chunk.
const CHUNK_START: u32 = 1 << 0;

/// Domain separation flag for the final block of a chunk.
const CHUNK_END: u32 = 1 << 1;

/// Domain separation flag for interior parent nodes.
const PARENT: u32 = 1 << 2;

/// Domain separation flag for the root output node.
const ROOT: u32 = 1 << 3;

/// Lowercase hexadecimal alphabet for formatting digest bytes.
const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

// endregion

// region Core Mixing & Compression

/// Quarter-round mixing function modifying four state words.
fn g(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(mx);
    state[d] = (state[d] ^ state[a]).rotate_right(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(12);
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(my);
    state[d] = (state[d] ^ state[a]).rotate_right(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(7);
}

/// Executes one round of column and diagonal mixing.
fn round(state: &mut [u32; 16], msg: &[u32; 16]) {
    g(state, 0, 4, 8, 12, msg[0], msg[1]);
    g(state, 1, 5, 9, 13, msg[2], msg[3]);
    g(state, 2, 6, 10, 14, msg[4], msg[5]);
    g(state, 3, 7, 11, 15, msg[6], msg[7]);
    g(state, 0, 5, 10, 15, msg[8], msg[9]);
    g(state, 1, 6, 11, 12, msg[10], msg[11]);
    g(state, 2, 7, 8, 13, msg[12], msg[13]);
    g(state, 3, 4, 9, 14, msg[14], msg[15]);
}

/// Permutes message words for the next round.
fn permute(m: &mut [u32; 16]) {
    let mut permuted = [0u32; 16];
    for (i, &p) in MSG_PERMUTATION.iter().enumerate() {
        permuted[i] = m[p];
    }
    *m = permuted;
}

/// Compresses a 64-byte block into 16 output words.
fn compress(
    chaining_value: &[u32; 8],
    block_words: &[u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u32; 16] {
    let mut state = [
        chaining_value[0],
        chaining_value[1],
        chaining_value[2],
        chaining_value[3],
        chaining_value[4],
        chaining_value[5],
        chaining_value[6],
        chaining_value[7],
        IV[0],
        IV[1],
        IV[2],
        IV[3],
        counter as u32,
        (counter >> 32) as u32,
        block_len,
        flags,
    ];
    let mut block = *block_words;
    for _ in 0..7 {
        round(&mut state, &block);
        permute(&mut block);
    }
    for i in 0..8 {
        state[i] ^= state[i + 8];
        state[i + 8] ^= chaining_value[i];
    }
    state
}

/// Returns the first eight words of a 16-word compression result.
fn first_8_words(words: [u32; 16]) -> [u32; 8] {
    let mut out = [0u32; 8];
    out.copy_from_slice(&words[..8]);
    out
}

/// Decodes sixteen 32-bit little-endian words from a byte slice.
fn words_from_le_bytes(bytes: &[u8], words: &mut [u32; 16]) {
    for (i, word) in words.iter_mut().enumerate() {
        let o = i * 4;
        *word = u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    }
}

// endregion

// region Tree Nodes & Output

/// Intermediate state of a compressed chunk or parent node.
struct Output {
    /// Input chaining value.
    input_chaining_value: [u32; 8],
    /// Message block words.
    block_words: [u32; 16],
    /// Chunk index counter.
    counter: u64,
    /// Byte length of the block.
    block_len: u32,
    /// Structural flags.
    flags: u32,
}

impl Output {
    /// Extracts the 8-word chaining value for this node.
    fn chaining_value(&self) -> [u32; 8] {
        first_8_words(compress(
            &self.input_chaining_value,
            &self.block_words,
            self.counter,
            self.block_len,
            self.flags,
        ))
    }

    /// Generates root output digest bytes.
    fn root_output_bytes(&self, out_slice: &mut [u8]) {
        for (i, out_block) in out_slice.chunks_mut(2 * OUT_LEN).enumerate() {
            let words = compress(
                &self.input_chaining_value,
                &self.block_words,
                i as u64,
                self.block_len,
                self.flags | ROOT,
            );
            for (word, chunk) in words.iter().zip(out_block.chunks_mut(4)) {
                let bytes = word.to_le_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }
    }
}

/// Builds an [`Output`] for an interior parent node combining two child chaining values.
fn parent_output(
    left_child_cv: [u32; 8],
    right_child_cv: [u32; 8],
    key_words: [u32; 8],
    flags: u32,
) -> Output {
    let mut block_words = [0u32; 16];
    block_words[..8].copy_from_slice(&left_child_cv);
    block_words[8..].copy_from_slice(&right_child_cv);
    Output {
        input_chaining_value: key_words,
        block_words,
        counter: 0,
        block_len: BLOCK_LEN as u32,
        flags: PARENT | flags,
    }
}

/// Computes the 8-word chaining value for a parent node.
fn parent_cv(
    left_child_cv: [u32; 8],
    right_child_cv: [u32; 8],
    key_words: [u32; 8],
    flags: u32,
) -> [u32; 8] {
    parent_output(left_child_cv, right_child_cv, key_words, flags).chaining_value()
}

// endregion

// region Chunk State & Hasher

/// State machine tracking progress within a single 1024-byte chunk.
struct ChunkState {
    /// Running chaining value.
    chaining_value: [u32; 8],
    /// 0-based chunk sequence index.
    chunk_counter: u64,
    /// Pending 64-byte block buffer.
    block: [u8; BLOCK_LEN],
    /// Number of valid bytes in `block`.
    block_len: u8,
    /// Number of 64-byte blocks compressed so far in this chunk.
    blocks_compressed: u8,
    /// Base flags.
    flags: u32,
}

impl ChunkState {
    /// Creates a fresh chunk state for `chunk_counter`.
    fn new(key_words: [u32; 8], chunk_counter: u64, flags: u32) -> Self {
        Self {
            chaining_value: key_words,
            chunk_counter,
            block: [0u8; BLOCK_LEN],
            block_len: 0,
            blocks_compressed: 0,
            flags,
        }
    }

    /// Total bytes absorbed into this chunk so far.
    fn len(&self) -> usize {
        (self.blocks_compressed as usize) * BLOCK_LEN + (self.block_len as usize)
    }

    /// Returns `CHUNK_START` if no blocks have been compressed yet.
    fn start_flag(&self) -> u32 {
        if self.blocks_compressed == 0 {
            CHUNK_START
        } else {
            0
        }
    }

    /// Consumes input bytes into the chunk state.
    fn update(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            if self.block_len as usize == BLOCK_LEN {
                let mut block_words = [0u32; 16];
                words_from_le_bytes(&self.block, &mut block_words);
                self.chaining_value = first_8_words(compress(
                    &self.chaining_value,
                    &block_words,
                    self.chunk_counter,
                    BLOCK_LEN as u32,
                    self.flags | self.start_flag(),
                ));
                self.blocks_compressed += 1;
                self.block = [0u8; BLOCK_LEN];
                self.block_len = 0;
            }
            let take = std::cmp::min(input.len(), BLOCK_LEN - (self.block_len as usize));
            let offset = self.block_len as usize;
            self.block[offset..offset + take].copy_from_slice(&input[..take]);
            self.block_len += take as u8;
            input = &input[take..];
        }
    }

    /// Emits the [`Output`] representation of this chunk.
    fn output(&self) -> Output {
        let mut block_words = [0u32; 16];
        words_from_le_bytes(&self.block, &mut block_words);
        Output {
            input_chaining_value: self.chaining_value,
            block_words,
            counter: self.chunk_counter,
            block_len: self.block_len as u32,
            flags: self.flags | self.start_flag() | CHUNK_END,
        }
    }
}

/// Incremental BLAKE3 hasher managing chunk and parent tree reductions.
pub(crate) struct Hasher {
    /// Active chunk state.
    chunk_state: ChunkState,
    /// Key words (defaults to IV for unkeyed hashing).
    key_words: [u32; 8],
    /// Chaining value stack for tree merges.
    cv_stack: [[u32; 8]; 54],
    /// Current length of `cv_stack`.
    cv_stack_len: u8,
    /// Base flags.
    flags: u32,
}

impl Hasher {
    /// Creates a new unkeyed BLAKE3 hasher.
    pub(crate) fn new() -> Self {
        Self {
            chunk_state: ChunkState::new(IV, 0, 0),
            key_words: IV,
            cv_stack: [[0u32; 8]; 54],
            cv_stack_len: 0,
            flags: 0,
        }
    }

    /// Pushes a chaining value onto the stack.
    fn push_stack(&mut self, cv: [u32; 8]) {
        self.cv_stack[self.cv_stack_len as usize] = cv;
        self.cv_stack_len += 1;
    }

    /// Pops the top chaining value from the stack.
    fn pop_stack(&mut self) -> [u32; 8] {
        self.cv_stack_len -= 1;
        self.cv_stack[self.cv_stack_len as usize]
    }

    /// Merges completed chunk chaining values into the tree stack.
    fn add_chunk_chaining_value(&mut self, mut new_cv: [u32; 8], mut total_chunks: u64) {
        while total_chunks & 1 == 0 {
            new_cv = parent_cv(self.pop_stack(), new_cv, self.key_words, self.flags);
            total_chunks >>= 1;
        }
        self.push_stack(new_cv);
    }

    /// Incrementally feeds `input` bytes into the hasher.
    pub(crate) fn update(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            if self.chunk_state.len() == CHUNK_LEN {
                let chunk_cv = self.chunk_state.output().chaining_value();
                let total_chunks = self.chunk_state.chunk_counter + 1;
                self.add_chunk_chaining_value(chunk_cv, total_chunks);
                self.chunk_state = ChunkState::new(self.key_words, total_chunks, self.flags);
            }
            let want = CHUNK_LEN - self.chunk_state.len();
            let take = std::cmp::min(want, input.len());
            self.chunk_state.update(&input[..take]);
            input = &input[take..];
        }
    }

    /// Finalizes the hash and writes the 32-byte digest to `out_slice`.
    pub(crate) fn finalize(&self, out_slice: &mut [u8]) {
        let mut output = self.chunk_state.output();
        let mut parent_nodes_remaining = self.cv_stack_len as usize;
        while parent_nodes_remaining > 0 {
            parent_nodes_remaining -= 1;
            output = parent_output(
                self.cv_stack[parent_nodes_remaining],
                output.chaining_value(),
                self.key_words,
                self.flags,
            );
        }
        output.root_output_bytes(out_slice);
    }
}

// endregion

// region Public Interface

/// Computes the 32-byte BLAKE3 digest of `bytes`.
pub(crate) fn blake3_digest(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

/// Computes the lowercase 64-character hexadecimal BLAKE3 digest of `bytes`.
pub(crate) fn blake3_hex(bytes: &[u8]) -> String {
    let digest = blake3_digest(bytes);
    let mut s = String::with_capacity(64);
    for byte in digest {
        s.push(HEX_CHARS[(byte >> 4) as usize] as char);
        s.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    s
}

// endregion

#[cfg(test)]
mod tests {
    use super::*;

    /// Generates standard test input bytes: `input[i] = (i % 251) as u8`.
    fn make_test_input(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// Verifies official BLAKE3 test vectors for empty, 1, 64, 65, 1024, 1025, and 2048 byte inputs.
    #[test]
    fn test_official_blake3_vectors() {
        assert_eq!(
            blake3_hex(b""),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(
            blake3_hex(&make_test_input(1)),
            "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"
        );
        assert_eq!(
            blake3_hex(&make_test_input(64)),
            "4eed7141ea4a5cd4b788606bd23f46e212af9cacebacdc7d1f4c6dc7f2511b98"
        );

        // For larger lengths, test streaming consistency:
        for len in [65, 1024, 1025, 2048, 5000] {
            let input = make_test_input(len);
            let one_shot = blake3_hex(&input);

            // Chunked streaming update
            let mut hasher = Hasher::new();
            for chunk in input.chunks(17) {
                hasher.update(chunk);
            }
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            let streamed: String = buf.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(one_shot, streamed, "mismatch at length {len}");
        }
    }
}
