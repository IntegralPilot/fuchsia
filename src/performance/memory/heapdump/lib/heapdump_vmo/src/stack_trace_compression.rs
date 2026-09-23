// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Given the size of an uncompressed stack trace (expressed as the number of stack frames), returns
/// an upper bound of its compressed size.
pub const fn max_compressed_size(num_frames: usize) -> usize {
    num_frames * VARINT_MAX_ENCODED_LEN
}

/// Compresses a stack trace into a preallocated buffer.
///
/// The destination buffer's size must be suitable for the input size (see `max_compressed_size` in
/// this crate).
///
/// This function returns the number of bytes actually written into the destination buffer (i.e. the
/// compressed size).
pub fn compress_into(src: &[usize], dest: &mut [u8]) -> usize {
    assert!(dest.len() >= max_compressed_size(src.len()), "dest buffer is not big enough");

    let mut offset = 0;
    let mut prev = 0;
    for value in src {
        let zigzag = zigzag_encode(value.wrapping_sub(prev));
        offset += varint_encode(zigzag, &mut dest[offset..]);
        prev = *value;
    }

    offset
}

/// Compresses a stack trace into a dynamically-allocated vector of bytes.
pub fn compress(src: &[usize]) -> Vec<u8> {
    let mut buf = vec![0; max_compressed_size(src.len())];
    let compressed_size = compress_into(src, &mut buf);
    buf.truncate(compressed_size);
    buf
}

/// Uncompresses a stack trace.
///
/// This function assumes that the uncompressed pointers will fit in usize, which is always true if
/// the stack trace was generated on the same architecture.
pub fn uncompress(src: &[u8]) -> Result<Vec<usize>, crate::Error> {
    let mut result = Vec::new();

    let mut offset = 0;
    let mut value = 0;
    while offset != src.len() {
        let (zigzag, num_bytes) =
            varint_decode(&src[offset..]).ok_or(crate::Error::InvalidInput)?;
        offset += num_bytes;
        value = zigzag_decode(zigzag).wrapping_add(value);
        result.push(value);
    }

    Ok(result)
}

const VARINT_SHIFT: u32 = 7;
const VARINT_VALUE_MASK: u8 = 0x7f;
const VARINT_CONT_BIT: u8 = 0x80;
const VARINT_MAX_ENCODED_LEN: usize = usize::BITS.div_ceil(VARINT_SHIFT) as usize;

/// Encodes a value into the given buffer, returning the number of bytes that were written.
fn varint_encode(mut value: usize, dest: &mut [u8]) -> usize {
    assert!(dest.len() >= VARINT_MAX_ENCODED_LEN, "dest buffer is not big enough");

    let mut offset = 0;
    loop {
        dest[offset] = value as u8 & VARINT_VALUE_MASK;
        value >>= VARINT_SHIFT;

        if value != 0 {
            dest[offset] |= VARINT_CONT_BIT;
            offset += 1;
        } else {
            return offset + 1;
        }
    }
}

/// Tries to decode a value from the given buffer, returning the value and the number of bytes that
/// were consumed.
fn varint_decode(src: &[u8]) -> Option<(usize, usize)> {
    let mut result = 0;
    let mut offset = 0;
    let mut shift = 0;
    loop {
        // Read the next byte or return None if we are at the end of the array.
        let input = src.get(offset)?;

        // Left-shift the value at its final position, then right-shift it back to validate that
        // we didn't lose any bit due to the left-shift overflowing.
        let value = (input & VARINT_VALUE_MASK) as usize;
        let value_shifted = value.checked_shl(shift)?;
        if (value_shifted >> shift) != value {
            return None; // overflow detected
        }

        result |= value_shifted;
        if (input & VARINT_CONT_BIT) != 0 {
            shift += VARINT_SHIFT;
            offset += 1;
        } else {
            return Some((result, offset + 1));
        }
    }
}

const ZIGZAG_VALUE_MASK: usize = !1;
const ZIGZAG_SIGN_BIT: usize = 1;

fn zigzag_encode(mut value: usize) -> usize {
    value = value.rotate_left(1);
    if (value & ZIGZAG_SIGN_BIT) != 0 {
        value ^= ZIGZAG_VALUE_MASK;
    }
    value
}

fn zigzag_decode(mut value: usize) -> usize {
    if (value & ZIGZAG_SIGN_BIT) != 0 {
        value ^= ZIGZAG_VALUE_MASK;
    }
    value.rotate_right(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    // Arbitrary constants, used instead of real code addresses in the tests below:
    const A: usize = 0x0000_008a_b3fe_9821;
    const B: usize = 0x0000_812c_6a4a_682e;
    const C: usize = 0x0048_b5a0_2d45_9e5a;
    const D: usize = 0xcccc_f148_39a5_9c5a;
    const F: usize = 0xffff_ffff_ffff_ffff; // all 1's
    const Z: usize = 0x0000_0000_0000_0000; // all 0's

    #[test_case(&[] ; "empty")]
    #[test_case(&[A] ; "A")]
    #[test_case(&[B] ; "B")]
    #[test_case(&[C] ; "C")]
    #[test_case(&[D] ; "D")]
    #[test_case(&[A, B] ; "AB")]
    #[test_case(&[A, A] ; "AA")]
    #[test_case(&[A, A, B] ; "AAB")]
    #[test_case(&[A, B, C, D] ; "ABCD")]
    #[test_case(&[A, A, B, B, C, C, D, D] ; "AABBCCDD")]
    #[test_case(&[D, D, C, C, B, B, A, A] ; "DDCCBBAA")]
    #[test_case(&[F, F, A, F, F] ; "FFAFF")]
    #[test_case(&[Z, Z, A, Z, Z] ; "ZZAFF")]
    #[test_case(&[F, Z, F] ; "FZF")]
    #[test_case(&[Z, F, Z] ; "ZFZ")]
    fn test_compress_and_uncompress(stack_trace: &[usize]) {
        let compressed_data = compress(stack_trace);
        assert_eq!(stack_trace, &uncompress(&compressed_data).unwrap());
    }

    #[test_case(&[] ; "empty")]
    #[test_case(&[0xff, 0xff] ; "ends with CONT bit set")]
    #[test_case(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02] ;
        "valid encoding but too big (u64::MAX + 1)")]
    #[test_case(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01] ;
        "valid encoding but too big (u64::MAX * 2^14)")]
    fn test_varint_decode_bad(data: &[u8]) {
        assert_eq!(varint_decode(data), None);
    }

    #[test_case(0 ; "0")]
    #[test_case(1 ; "1")]
    #[test_case(-1 ; "negative 1")]
    #[test_case(1000 ; "1000")]
    #[test_case(-1000 ; "negative 1000")]
    #[test_case(isize::MIN ; "min")]
    #[test_case(isize::MAX ; "max")]
    fn test_zigzag(value: isize) {
        let encoded_value = zigzag_encode(value as usize);
        let decoded_value = zigzag_decode(encoded_value) as isize;
        assert_eq!(decoded_value, value, "encoded value: {:x}", encoded_value);
    }
}
