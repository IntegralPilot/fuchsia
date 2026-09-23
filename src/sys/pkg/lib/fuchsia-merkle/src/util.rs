// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bssl_crypto::digest::Sha256;
use std::mem::size_of;
use storage_ptr_slice::PtrByteSlice;

use crate::{BLOCK_SIZE, HASH_SIZE, Hash};

pub const HASHES_PER_BLOCK: usize = BLOCK_SIZE / HASH_SIZE;

type BlockIdentity = [u8; size_of::<u64>() + size_of::<u32>()];

/// Generate the bytes representing a block's identity.
#[inline]
fn make_identity(length: usize, level: usize, offset: usize) -> BlockIdentity {
    let [o0, o1, o2, o3, o4, o5, o6, o7] = (offset as u64 | level as u64).to_le_bytes();
    let [l0, l1, l2, l3] = (length as u32).to_le_bytes();
    [o0, o1, o2, o3, o4, o5, o6, o7, l0, l1, l2, l3]
}

/// Compute the merkle hash of a block of data.
///
/// A merkle hash is the SHA-256 hash of a block of data with a small header built from the length
/// of the data, the level of the tree (0 for data blocks), and the offset into the level. The
/// block will be zero filled if its len is less than [`BLOCK_SIZE`], except for when the first
/// data block is completely empty.
///
/// # Panics
///
/// Panics if `block.len()` exceeds [`BLOCK_SIZE`] or if `offset` is not aligned to [`BLOCK_SIZE`]
pub fn hash_block(block: &[u8], offset: usize) -> Hash {
    assert!(block.len() <= BLOCK_SIZE);
    assert!(offset.is_multiple_of(BLOCK_SIZE));

    let mut hasher = Sha256::new();
    hasher.update(&make_identity(block.len(), 0, offset));
    hasher.update(block);
    // Zero fill block up to BLOCK_SIZE. As a special case, if the first data block is completely
    // empty, it is not zero filled.
    if block.len() != BLOCK_SIZE && !(block.is_empty() && offset == 0) {
        update_with_zeros(&mut hasher, BLOCK_SIZE - block.len());
    }

    Hash::from(hasher.digest())
}

/// Hashes a single aligned block (4096 or 8192 bytes) from `data`.
///
/// `unaligned_len` specifies the number of valid bytes in the block
/// (`0 < unaligned_len <= buffer_len`).
/// Note: Any data in `data` beyond `unaligned_len` (up to `buffer_len`) MUST be zeroed
/// for the computed hash to match.
///
/// Reads directly through raw pointers in `data` without minting Rust slice references,
/// avoiding aliasing/provenance UB on shared DMA memory.
///
/// # Panics
///
/// Panics if `buffer_len` is not 4096 or 8192, or if `unaligned_len == 0` or
/// `unaligned_len > buffer_len`.
pub fn hash_block_aligned(
    offset: usize,
    data: PtrByteSlice<'_>,
    buffer_len: usize,
    unaligned_len: usize,
) -> Hash {
    assert!(buffer_len == BLOCK_SIZE || buffer_len == 4096);
    assert!(unaligned_len > 0 && unaligned_len <= buffer_len);
    assert!(offset.is_multiple_of(BLOCK_SIZE));
    assert!(data.len() >= buffer_len);

    #[cfg(all(target_arch = "aarch64", target_feature = "sha2"))]
    {
        arm64::hash_block_arm64_neon(offset, data, buffer_len, unaligned_len)
    }

    #[cfg(not(all(target_arch = "aarch64", target_feature = "sha2")))]
    {
        #[repr(C)]
        struct Sha256Ctx {
            _h: [u32; 8],
            _nl: u32,
            _nh: u32,
            _data: [u8; 64],
            _num: core::ffi::c_uint,
            _md_len: core::ffi::c_uint,
        }

        unsafe extern "C" {
            fn SHA256_Init(sha: *mut Sha256Ctx) -> core::ffi::c_int;
            fn SHA256_Update(
                sha: *mut Sha256Ctx,
                data: *const core::ffi::c_void,
                len: usize,
            ) -> core::ffi::c_int;
            fn SHA256_Final(out: *mut u8, sha: *mut Sha256Ctx) -> core::ffi::c_int;
        }

        let mut ctx = std::mem::MaybeUninit::<Sha256Ctx>::uninit();
        // SAFETY: SHA256_Init initializes the context. data is valid for reading buffer_len bytes.
        unsafe {
            SHA256_Init(ctx.as_mut_ptr());
            let identity = make_identity(unaligned_len, 0, offset);
            SHA256_Update(
                ctx.as_mut_ptr(),
                identity.as_ptr() as *const core::ffi::c_void,
                identity.len(),
            );
            SHA256_Update(ctx.as_mut_ptr(), data.as_ptr() as *const core::ffi::c_void, buffer_len);
            if buffer_len < BLOCK_SIZE {
                const ZEROS: [u8; 4096] = [0; 4096];
                SHA256_Update(
                    ctx.as_mut_ptr(),
                    ZEROS.as_ptr() as *const core::ffi::c_void,
                    BLOCK_SIZE - buffer_len,
                );
            }
            let mut out = [0u8; HASH_SIZE];
            SHA256_Final(out.as_mut_ptr(), ctx.as_mut_ptr());
            Hash::from_array(out)
        }
    }
}

#[cfg(all(target_arch = "aarch64", target_feature = "sha2"))]
mod arm64 {
    use super::{BLOCK_SIZE, make_identity};
    use crate::Hash;
    use core::arch::aarch64::*;
    use storage_ptr_slice::PtrByteSlice;

    #[repr(C, align(16))]
    struct Aligned64U32([u32; 64]);

    #[repr(C, align(16))]
    struct Aligned4U32([u32; 4]);

    static K256: Aligned64U32 = Aligned64U32([
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ]);

    static SHA256_INIT_H_0_3: Aligned4U32 =
        Aligned4U32([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a]);
    static SHA256_INIT_H_4_7: Aligned4U32 =
        Aligned4U32([0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);

    #[derive(Clone, Copy)]
    struct KTable {
        k0: uint32x4_t,
        k1: uint32x4_t,
        k2: uint32x4_t,
        k3: uint32x4_t,
        k4: uint32x4_t,
        k5: uint32x4_t,
        k6: uint32x4_t,
        k7: uint32x4_t,
        k8: uint32x4_t,
        k9: uint32x4_t,
        k10: uint32x4_t,
        k11: uint32x4_t,
        k12: uint32x4_t,
        k13: uint32x4_t,
        k14: uint32x4_t,
        k15: uint32x4_t,
    }

    impl KTable {
        #[inline(always)]
        fn load() -> Self {
            // SAFETY: Reading 64 32-bit constants from 16-byte aligned static array.
            unsafe {
                let p = K256.0.as_ptr();
                Self {
                    k0: vld1q_u32(p),
                    k1: vld1q_u32(p.add(4)),
                    k2: vld1q_u32(p.add(8)),
                    k3: vld1q_u32(p.add(12)),
                    k4: vld1q_u32(p.add(16)),
                    k5: vld1q_u32(p.add(20)),
                    k6: vld1q_u32(p.add(24)),
                    k7: vld1q_u32(p.add(28)),
                    k8: vld1q_u32(p.add(32)),
                    k9: vld1q_u32(p.add(36)),
                    k10: vld1q_u32(p.add(40)),
                    k11: vld1q_u32(p.add(44)),
                    k12: vld1q_u32(p.add(48)),
                    k13: vld1q_u32(p.add(52)),
                    k14: vld1q_u32(p.add(56)),
                    k15: vld1q_u32(p.add(60)),
                }
            }
        }
    }

    struct Sha256VectorState {
        abcd: uint32x4_t,
        efgh: uint32x4_t,
    }

    // Single SHA-256 round with message scheduling:
    // 1. Adds current schedule word + round constant (W_t + K_t).
    // 2. Begins message schedule step (sha256su0) for future round W_{t+4}.
    // 3. Preserves `abcd` in 1 cycle via bitwise OR (`orr tmp, abcd, abcd`) for `sha256h2`.
    // 4. Computes compression rounds (sha256h and sha256h2) across abcd and efgh.
    // 5. Completes message schedule step (sha256su1).
    macro_rules! sched_round {
        ($w0:ident, $w1:ident, $w2:ident, $w3:ident, $k:ident, $vtmp:ident) => {
            concat!(
                "add ",
                stringify!($vtmp),
                ".4s, ",
                stringify!($w0),
                ".4s, {",
                stringify!($k),
                ":v}.4s\n",
                "sha256su0 ",
                stringify!($w0),
                ".4s, ",
                stringify!($w1),
                ".4s\n",
                "orr {tmp:v}.16b, {abcd:v}.16b, {abcd:v}.16b\n",
                "sha256h {abcd:q}, {efgh:q}, ",
                stringify!($vtmp),
                ".4s\n",
                "sha256h2 {efgh:q}, {tmp:q}, ",
                stringify!($vtmp),
                ".4s\n",
                "sha256su1 ",
                stringify!($w0),
                ".4s, ",
                stringify!($w2),
                ".4s, ",
                stringify!($w3),
                ".4s\n",
            )
        };
    }

    // Final SHA-256 round without message scheduling:
    // Rounds 12-15 are the last 4 rounds of a 64-round block. Computing W_{64..67} is
    // unnecessary as those words are never consumed, saving vector instructions.
    macro_rules! final_round {
        ($w:ident, $k:ident, $vtmp:ident) => {
            concat!(
                "add ",
                stringify!($vtmp),
                ".4s, ",
                stringify!($w),
                ".4s, {",
                stringify!($k),
                ":v}.4s\n",
                "orr {tmp:v}.16b, {abcd:v}.16b, {abcd:v}.16b\n",
                "sha256h {abcd:q}, {efgh:q}, ",
                stringify!($vtmp),
                ".4s\n",
                "sha256h2 {efgh:q}, {tmp:q}, ",
                stringify!($vtmp),
                ".4s\n",
            )
        };
    }

    // 4-round quad with cyclic message schedule rotation across v4..v7:
    macro_rules! sched_quad {
        ($k0:ident, $k1:ident, $k2:ident, $k3:ident) => {
            concat!(
                sched_round!(v4, v5, v6, v7, $k0, v16),
                sched_round!(v5, v6, v7, v4, $k1, v17),
                sched_round!(v6, v7, v4, v5, $k2, v16),
                sched_round!(v7, v4, v5, v6, $k3, v17),
            )
        };
    }

    // Final 4-round quad for rounds 12-15 without message scheduling:
    macro_rules! final_quad {
        ($k0:ident, $k1:ident, $k2:ident, $k3:ident) => {
            concat!(
                final_round!(v4, $k0, v16),
                final_round!(v5, $k1, v17),
                final_round!(v6, $k2, v16),
                final_round!(v7, $k3, v17),
            )
        };
    }

    // Single zero-block round:
    // Since all input words W_t are zero, message schedule produces all zeros.
    // Therefore W_t + K_t = K_t, allowing direct compression with the constant register.
    macro_rules! zero_round {
        ($k:ident) => {
            concat!(
                "orr {tmp:v}.16b, {abcd:v}.16b, {abcd:v}.16b\n",
                "sha256h {abcd:q}, {efgh:q}, {",
                stringify!($k),
                ":v}.4s\n",
                "sha256h2 {efgh:q}, {tmp:q}, {",
                stringify!($k),
                ":v}.4s\n",
            )
        };
    }

    // 4 zero rounds grouped together:
    macro_rules! zero_quad {
        ($k0:ident, $k1:ident, $k2:ident, $k3:ident) => {
            concat!(zero_round!($k0), zero_round!($k1), zero_round!($k2), zero_round!($k3),)
        };
    }

    // Macro to execute the 64 rounds of SHA-256 compression on quadwords in v4..v7.
    macro_rules! sha256_compress_asm {
        ($state:ident, $ktable:ident, $load_prefix:expr, $($input_operands:tt)*) => {
            core::arch::asm!(
                $load_prefix,
                "rev32 v4.16b, v4.16b\n",
                "rev32 v5.16b, v5.16b\n",
                "rev32 v6.16b, v6.16b\n",
                "rev32 v7.16b, v7.16b\n",

                sched_quad!(k0, k1, k2, k3),
                sched_quad!(k4, k5, k6, k7),
                sched_quad!(k8, k9, k10, k11),
                final_quad!(k12, k13, k14, k15),

                $($input_operands)*
                abcd = inout(vreg) $state.abcd,
                efgh = inout(vreg) $state.efgh,
                tmp = out(vreg) _,
                k0 = in(vreg) $ktable.k0,
                k1 = in(vreg) $ktable.k1,
                k2 = in(vreg) $ktable.k2,
                k3 = in(vreg) $ktable.k3,
                k4 = in(vreg) $ktable.k4,
                k5 = in(vreg) $ktable.k5,
                k6 = in(vreg) $ktable.k6,
                k7 = in(vreg) $ktable.k7,
                k8 = in(vreg) $ktable.k8,
                k9 = in(vreg) $ktable.k9,
                k10 = in(vreg) $ktable.k10,
                k11 = in(vreg) $ktable.k11,
                k12 = in(vreg) $ktable.k12,
                k13 = in(vreg) $ktable.k13,
                k14 = in(vreg) $ktable.k14,
                k15 = in(vreg) $ktable.k15,
                out("v16") _, out("v17") _,
            )
        };
    }

    macro_rules! zero_compress_quads {
        () => {
            concat!(
                zero_quad!(k0, k1, k2, k3),
                zero_quad!(k4, k5, k6, k7),
                zero_quad!(k8, k9, k10, k11),
                zero_quad!(k12, k13, k14, k15),
            )
        };
    }

    impl Sha256VectorState {
        #[inline(always)]
        fn new() -> Self {
            // SAFETY: Reading 16-byte aligned static constant arrays.
            unsafe {
                Self {
                    abcd: vld1q_u32(SHA256_INIT_H_0_3.0.as_ptr()),
                    efgh: vld1q_u32(SHA256_INIT_H_4_7.0.as_ptr()),
                }
            }
        }

        /// Performs SHA-256 transformation on a 64-byte block loaded from `ptr`.
        ///
        /// # Safety
        ///
        /// `ptr` must be valid for reading 64 bytes.
        #[inline(always)]
        unsafe fn transform_block_from_ptr(&mut self, ktable: &KTable, ptr: *const u8) {
            let saved_abcd = self.abcd;
            let saved_efgh = self.efgh;

            // SAFETY: ARMv8-A SHA-256 block transformation with cycle-accurate instruction pairing.
            unsafe {
                sha256_compress_asm!(
                    self,
                    ktable,
                    "ld1 {{v4.16b, v5.16b, v6.16b, v7.16b}}, [{ptr}]\n",
                    ptr = in(reg) ptr,
                    out("v4") _, out("v5") _, out("v6") _, out("v7") _,
                );
                self.abcd = vaddq_u32(self.abcd, saved_abcd);
                self.efgh = vaddq_u32(self.efgh, saved_efgh);
            }
        }

        /// Performs SHA-256 transformation on a block of all zeros without message
        /// scheduling overhead.
        #[inline(always)]
        fn transform_zero_block(&mut self, ktable: &KTable) {
            let saved_abcd = self.abcd;
            let saved_efgh = self.efgh;

            // SAFETY: For zero input, all W_t are zero and message schedule W_t produces zeros.
            // Therefore W_t + K_t = K_t for all 64 rounds, requiring no scheduling operations.
            unsafe {
                core::arch::asm!(
                    zero_compress_quads!(),

                    abcd = inout(vreg) self.abcd,
                    efgh = inout(vreg) self.efgh,
                    tmp = out(vreg) _,
                    k0 = in(vreg) ktable.k0,
                    k1 = in(vreg) ktable.k1,
                    k2 = in(vreg) ktable.k2,
                    k3 = in(vreg) ktable.k3,
                    k4 = in(vreg) ktable.k4,
                    k5 = in(vreg) ktable.k5,
                    k6 = in(vreg) ktable.k6,
                    k7 = in(vreg) ktable.k7,
                    k8 = in(vreg) ktable.k8,
                    k9 = in(vreg) ktable.k9,
                    k10 = in(vreg) ktable.k10,
                    k11 = in(vreg) ktable.k11,
                    k12 = in(vreg) ktable.k12,
                    k13 = in(vreg) ktable.k13,
                    k14 = in(vreg) ktable.k14,
                    k15 = in(vreg) ktable.k15,
                );
                self.abcd = vaddq_u32(self.abcd, saved_abcd);
                self.efgh = vaddq_u32(self.efgh, saved_efgh);
            }
        }

        /// Performs SHA-256 transformation directly on vector registers.
        #[inline(always)]
        fn transform_words(
            &mut self,
            ktable: &KTable,
            w0: uint8x16_t,
            w1: uint8x16_t,
            w2: uint8x16_t,
            w3: uint8x16_t,
        ) {
            let saved_abcd = self.abcd;
            let saved_efgh = self.efgh;

            // SAFETY: ARMv8-A SHA-256 block transformation with cycle-accurate instruction pairing.
            unsafe {
                sha256_compress_asm!(
                    self,
                    ktable,
                    "",
                    inout("v4") w0 => _,
                    inout("v5") w1 => _,
                    inout("v6") w2 => _,
                    inout("v7") w3 => _,
                );
                self.abcd = vaddq_u32(self.abcd, saved_abcd);
                self.efgh = vaddq_u32(self.efgh, saved_efgh);
            }
        }

        /// Converts final 256-bit vector registers to a big-endian `Hash`.
        #[inline(always)]
        fn into_hash(self) -> Hash {
            // SAFETY: Vector register conversion and storing to aligned stack array.
            unsafe {
                let abcd_be = vrev32q_u8(vreinterpretq_u8_u32(self.abcd));
                let efgh_be = vrev32q_u8(vreinterpretq_u8_u32(self.efgh));
                let mut out = [0u8; 32];
                vst1q_u8(out.as_mut_ptr(), abcd_be);
                vst1q_u8(out.as_mut_ptr().add(16), efgh_be);
                Hash::from_array(out)
            }
        }
    }

    /// ARMv8-A accelerated SHA-256 calculation for page-aligned Merkle blocks (4096 or 8192 bytes).
    ///
    /// Note: Any data in `data` beyond `unaligned_len` (up to `buffer_len`) MUST be zeroed
    /// for the computed hash to match.
    #[inline(always)]
    pub fn hash_block_arm64_neon(
        offset: usize,
        data: PtrByteSlice<'_>,
        buffer_len: usize,
        unaligned_len: usize,
    ) -> Hash {
        assert!(buffer_len == BLOCK_SIZE || buffer_len == 4096);
        assert!(unaligned_len > 0 && unaligned_len <= buffer_len);
        assert!(offset.is_multiple_of(BLOCK_SIZE));
        assert!(data.len() >= buffer_len);

        let ktable = KTable::load();
        let mut state = Sha256VectorState::new();

        // Block 0: 12B identity header + first 52B of data loaded directly into vector registers.
        let mut w0_bytes = [0u8; 16];
        w0_bytes[0..12].copy_from_slice(&make_identity(unaligned_len, 0, offset));
        let src = data.as_ptr();
        // SAFETY: data has at least buffer_len >= 4096 bytes (asserted above), so offsets 0..52
        // are valid for reading.
        unsafe {
            (w0_bytes.as_mut_ptr().add(12) as *mut u32)
                .write_unaligned((src as *const u32).read_unaligned());
            let w0 = vld1q_u8(w0_bytes.as_ptr());
            let w1 = vld1q_u8(src.add(4));
            let w2 = vld1q_u8(src.add(20));
            let w3 = vld1q_u8(src.add(36));
            state.transform_words(&ktable, w0, w1, w2, w3);
        }

        /// Loads 12 unaligned bytes from `src`, sets byte 12 to `pad_byte` (with bytes 13..15 as 0),
        /// and loads the resulting 16-byte buffer into a vector register.
        ///
        /// # Safety
        ///
        /// `src` must be valid for reading 12 bytes.
        #[inline(always)]
        unsafe fn load_12b_padded(src: *const u8, pad_byte: u8) -> uint8x16_t {
            let mut buf = [0u8; 16];
            // SAFETY: `src` is valid for reading 12 bytes, and `buf` is a local 16-byte array.
            unsafe {
                (buf.as_mut_ptr() as *mut u64)
                    .write_unaligned((src as *const u64).read_unaligned());
                (buf.as_mut_ptr().add(8) as *mut u32)
                    .write_unaligned((src.add(8) as *const u32).read_unaligned());
                buf[12] = pad_byte;
                vld1q_u8(buf.as_ptr())
            }
        }

        /// Performs the SHA-256 transformation on the tail padding block (Block 128)
        /// using 8 KiB Merkle length constant (12B header + 8192B payload).
        #[inline(always)]
        fn transform_tail_block(state: &mut Sha256VectorState, ktable: &KTable, w0: uint8x16_t) {
            const TOTAL_BITS_8K: u64 = (12 + 8192) as u64 * 8;
            let mut w3_bytes = [0u8; 16];
            // SAFETY: `w3_bytes` is a 16-byte stack array, and `zero_vec` is a valid vector.
            unsafe {
                (w3_bytes.as_mut_ptr().add(8) as *mut u64).write_unaligned(TOTAL_BITS_8K.to_be());
                let zero_vec = vdupq_n_u8(0);
                let w3 = vld1q_u8(w3_bytes.as_ptr());
                state.transform_words(ktable, w0, zero_vec, zero_vec, w3);
            }
        }

        // Single branch check for 8 KiB (BLOCK_SIZE) vs 4 KiB buffer_len:
        if buffer_len >= BLOCK_SIZE {
            // Fast path for 8 KiB page: 127 full 64-byte vector blocks
            for block_idx in 0..127 {
                let block_offset = 52 + block_idx * 64;
                // SAFETY: data has at least 8192 bytes, 52 + 126 * 64 + 64 = 8180 <= 8192.
                unsafe { state.transform_block_from_ptr(&ktable, data.as_ptr().add(block_offset)) };
            }

            // Tail block (block 128): remaining 12B of data (offset 8180..8192) + SHA-256 padding
            // SAFETY: data has at least 8192 bytes, so offset 8180 is valid for reading 12 bytes.
            let w0 = unsafe { load_12b_padded(data.as_ptr().add(8180), 0x80) };
            transform_tail_block(&mut state, &ktable, w0);
        } else {
            // Fast path for 4 KiB page: 63 full 64-byte vector blocks
            for block_idx in 0..63 {
                let block_offset = 52 + block_idx * 64;
                // SAFETY: data has at least 4096 bytes, 52 + 62 * 64 + 64 = 4084 <= 4096.
                unsafe { state.transform_block_from_ptr(&ktable, data.as_ptr().add(block_offset)) };
            }

            // Intermediate block (block 64): remaining 12B (offset 4084..4096) + 52B of zeros
            // SAFETY: data has at least 4096 bytes, so offset 4084 is valid for reading 12 bytes.
            unsafe {
                let w0 = load_12b_padded(data.as_ptr().add(4084), 0x00);
                let zero_vec = vdupq_n_u8(0);
                state.transform_words(&ktable, w0, zero_vec, zero_vec, zero_vec);
            }

            // Remaining 63 blocks (4096 - 52 bytes) of zeros
            for _ in 0..63 {
                state.transform_zero_block(&ktable);
            }

            // Tail block (block 128): 12B zeros + 0x80 padding + 64-bit length
            let mut w0_bytes = [0u8; 16];
            w0_bytes[12] = 0x80;
            // SAFETY: `w0_bytes` is a local 16-byte stack array.
            let w0 = unsafe { vld1q_u8(w0_bytes.as_ptr()) };
            transform_tail_block(&mut state, &ktable, w0);
        }

        state.into_hash()
    }
}

/// Updates `hasher` with `count` zeros.
pub(crate) fn update_with_zeros(hasher: &mut Sha256, mut count: usize) {
    const BUF_SIZE: usize = 512;
    const ZEROS: [u8; BUF_SIZE] = [0; BUF_SIZE];
    while count >= BUF_SIZE {
        count -= BUF_SIZE;
        hasher.update(&ZEROS);
    }
    if count > 0 {
        hasher.update(&ZEROS[0..count]);
    }
}

/// Creates a new [`Sha256`] for hashing blocks of hashes. The hasher is initialized with the
/// block's identity.
pub fn make_hash_hasher(hashes_per_hash: usize, level: usize, offset: usize) -> Sha256 {
    debug_assert!(level > 0);
    let bytes_per_hash = hashes_per_hash * HASH_SIZE;
    debug_assert!(offset.is_multiple_of(bytes_per_hash));
    let mut hasher = Sha256::new();
    hasher.update(&make_identity(bytes_per_hash, level, offset));
    hasher
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_ptr_slice::PtrByteSlice;

    fn generate_pattern_data(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + 17) % 256) as u8).collect()
    }

    #[test]
    fn test_hash_block_empty() {
        let block = [];
        let hash = hash_block(&block[..], 0);
        let expected =
            "15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b".parse().unwrap();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_block_single() {
        let block = vec![0xFF; 8192];
        let hash = hash_block(&block[..], 0);
        let expected =
            "68d131bc271f9c192d4f6dcd8fe61bef90004856da19d0f2f514a7f4098b0737".parse().unwrap();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_block_aligned_matches_reference_4k() {
        let offsets = [0, BLOCK_SIZE, 16384];
        let unaligned_lens = [1, 12, 51, 52, 53, 100, 500, 4083, 4084, 4085, 4096];

        for &offset in &offsets {
            for &unaligned_len in &unaligned_lens {
                let full_data = generate_pattern_data(unaligned_len);
                let ref_hash = hash_block(&full_data, offset);

                let mut buf = vec![0u8; 4096];
                buf[..unaligned_len].copy_from_slice(&full_data);

                let hash =
                    hash_block_aligned(offset, PtrByteSlice::from(&buf[..]), 4096, unaligned_len);
                assert_eq!(
                    hash, ref_hash,
                    "Mismatch: offset={offset}, unaligned_len={unaligned_len}"
                );
            }
        }
    }

    #[test]
    fn test_hash_block_aligned_matches_reference_8k() {
        let offsets = [0, BLOCK_SIZE, 16384];
        let unaligned_lens = [1, 12, 51, 52, 53, 4096, 8179, 8180, 8181, 8192];

        for &offset in &offsets {
            for &unaligned_len in &unaligned_lens {
                let full_data = generate_pattern_data(unaligned_len);
                let ref_hash = hash_block(&full_data, offset);

                let mut buf = vec![0u8; BLOCK_SIZE];
                buf[..unaligned_len].copy_from_slice(&full_data);

                let hash = hash_block_aligned(
                    offset,
                    PtrByteSlice::from(&buf[..]),
                    BLOCK_SIZE,
                    unaligned_len,
                );
                assert_eq!(
                    hash, ref_hash,
                    "Mismatch: offset={offset}, unaligned_len={unaligned_len}"
                );
            }
        }
    }

    #[test]
    fn test_hash_block_aligned_unzeroed_tail_causes_hash_mismatch() {
        let offset = 0;
        let unaligned_len = 100;
        let full_data = generate_pattern_data(unaligned_len);
        let ref_hash = hash_block(&full_data, offset);

        let mut buf = vec![0xAAu8; 4096];
        buf[..unaligned_len].copy_from_slice(&full_data);

        let hash = hash_block_aligned(offset, PtrByteSlice::from(&buf[..]), 4096, unaligned_len);

        assert_ne!(hash, ref_hash, "Unzeroed tail must result in mismatched hash");
    }

    #[cfg(all(target_arch = "aarch64", target_feature = "sha2"))]
    #[test]
    fn test_arm64_neon_matches_reference() {
        let offsets = [0, BLOCK_SIZE, 32768];
        let buffer_lens = [4096, BLOCK_SIZE];

        for &buffer_len in &buffer_lens {
            let unaligned_lens = if buffer_len == 4096 {
                vec![1, 12, 51, 52, 53, 2000, 4083, 4084, 4085, 4096]
            } else {
                vec![1, 12, 51, 52, 53, 4096, 8179, 8180, 8181, 8192]
            };

            for &offset in &offsets {
                for &unaligned_len in unaligned_lens.iter() {
                    let full_data = generate_pattern_data(unaligned_len);
                    let ref_hash = hash_block(&full_data, offset);

                    let mut buf = vec![0u8; buffer_len];
                    buf[..unaligned_len].copy_from_slice(&full_data);

                    let neon_hash = arm64::hash_block_arm64_neon(
                        offset,
                        PtrByteSlice::from(&buf[..]),
                        buffer_len,
                        unaligned_len,
                    );
                    assert_eq!(
                        neon_hash, ref_hash,
                        "NEON mismatch: buf={buffer_len}, off={offset}, len={unaligned_len}",
                    );
                }
            }
        }
    }
}
