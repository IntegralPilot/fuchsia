// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Rust implementation of hexdump utilities, matching zircon/system/ulib/pretty.

use core::fmt::Write;

/// Unsanitized copy helper that copies `chunk_len` bytes from `src` to `dest` without KASan checks.
///
/// # Safety
/// The caller must ensure that the memory range `[src, src + chunk_len)` is mapped and readable by the process.
//
// TODO(https://fxbug.dev/521834554): KAsan doesn't work at all for Rust code currently, so all
// Rust accesses are unsanitized. When KAsan is enabled this routine should be annotated to disable
// instrumentation.
#[inline(never)]
unsafe fn unsanitized_copy(src: *const u8, dest: *mut u8, chunk_len: usize) {
    for i in 0..chunk_len {
        unsafe {
            *dest.add(i) = *src.add(i);
        }
    }
}

/// Do a hex dump against a writer, formatting data as 32-bit words (host endianness)
/// alongside an 8-bit ASCII panel on the side, displaying up to 16 bytes per line.
///
/// The "very" in the name follows the Zircon kernel naming scheme.
pub fn hexdump_very_ex_rs<W: Write>(
    writer: &mut W,
    data: &[u8],
    disp_addr: u64,
) -> core::fmt::Result {
    // SAFETY: A standard safe slice reference is guaranteed to point to mapped, valid, and unpoisoned memory.
    unsafe { hexdump_very_ex_raw(writer, data.as_ptr(), data.len(), disp_addr) }
}

/// Do a hex dump against a writer, formatting data as 32-bit words (host endianness)
/// alongside an 8-bit ASCII panel, displaying up to 16 bytes per line, using suspect raw pointers.
///
/// # Safety
/// The caller must ensure that the memory range `[ptr, ptr + len)` is mapped and readable.
/// KASan instrumentation is bypassed during pointer accesses to prevent panic loops.
pub unsafe fn hexdump_very_ex_raw<W: Write>(
    writer: &mut W,
    ptr: *const u8,
    len: usize,
    disp_addr: u64,
) -> core::fmt::Result {
    for count in (0..len).step_by(16) {
        let chunk_len = core::cmp::min(len - count, 16);
        // Round up to next multiple of 4 to match C++ word-based printing.
        let s = chunk_len.next_multiple_of(4);

        // Copy available bytes to a local buffer, padded with 0.
        let mut buf = [0u8; 16];
        // SAFETY: The range [ptr + count, ptr + count + chunk_len) is guaranteed mapped by caller.
        unsafe {
            unsanitized_copy(ptr.add(count), buf.as_mut_ptr(), chunk_len);
        }

        if disp_addr + len as u64 > 0xFFFFFFFF {
            core::write!(writer, "0x{:016x}: ", disp_addr + count as u64)?;
        } else {
            core::write!(writer, "0x{:08x}: ", disp_addr + count as u64)?;
        }

        let words = s / 4;
        let (chunks, _) = buf[..s].as_chunks::<4>();
        for chunk in chunks {
            // C++ reads as uint32_t and prints with %08x.
            // On little endian, uint32_t of [A, B, C, D] is 0xDDCCBBAA.
            let val = u32::from_ne_bytes(*chunk);
            core::write!(writer, "{:08x} ", val)?;
        }
        for _ in words..4 {
            core::write!(writer, "         ")?;
        }
        core::write!(writer, "|")?;

        for i in 0..16 {
            let c = buf[i];
            if i < s && (c.is_ascii_graphic() || c == b' ') {
                core::write!(writer, "{}", c as char)?;
            } else {
                core::write!(writer, ".")?;
            }
        }
        core::write!(writer, "|\n")?;
    }
    Ok(())
}

/// Do a hex dump against a writer, formatting data as individual 8-bit bytes
/// alongside an 8-bit ASCII panel, displaying up to 16 bytes per line.
pub fn hexdump8_very_ex_rs<W: Write>(
    writer: &mut W,
    data: &[u8],
    disp_addr: u64,
) -> core::fmt::Result {
    // SAFETY: A standard safe slice reference is guaranteed to point to mapped, valid, and unpoisoned memory.
    unsafe { hexdump8_very_ex_raw(writer, data.as_ptr(), data.len(), disp_addr) }
}

/// Do a hex dump against a writer, formatting data as individual 8-bit bytes
/// alongside an 8-bit ASCII panel, displaying up to 16 bytes per line, using suspect raw pointers.
///
/// # Safety
/// The caller must ensure that the memory range `[ptr, ptr + len)` is mapped and readable.
/// KASan instrumentation is bypassed during pointer accesses to prevent panic loops.
pub unsafe fn hexdump8_very_ex_raw<W: Write>(
    writer: &mut W,
    ptr: *const u8,
    len: usize,
    disp_addr: u64,
) -> core::fmt::Result {
    for count in (0..len).step_by(16) {
        let chunk_len = core::cmp::min(len - count, 16);

        // Copy available bytes to a local buffer, padded with 0.
        let mut buf = [0u8; 16];
        // SAFETY: The range [ptr + count, ptr + count + chunk_len) is guaranteed mapped by caller.
        unsafe {
            unsanitized_copy(ptr.add(count), buf.as_mut_ptr(), chunk_len);
        }

        if disp_addr + len as u64 > 0xFFFFFFFF {
            core::write!(writer, "0x{:016x}: ", disp_addr + count as u64)?;
        } else {
            core::write!(writer, "0x{:08x}: ", disp_addr + count as u64)?;
        }

        for i in 0..chunk_len {
            core::write!(writer, "{:02x} ", buf[i])?;
        }
        for _ in chunk_len..16 {
            core::write!(writer, "   ")?;
        }

        core::write!(writer, "|")?;

        for i in 0..chunk_len {
            let c = buf[i];
            if c.is_ascii_graphic() || c == b' ' {
                core::write!(writer, "{}", c as char)?;
            } else {
                core::write!(writer, ".")?;
            }
        }
        core::write!(writer, "\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::string::String;

    #[test]
    fn test_hexdump_very_ex() {
        let input = [0u8, 1, 2, 3, b'a', b'b', b'c', b'd'];
        let test_display_addr = 0x1000;
        let expected = "0x00001000: 03020100 64636261                   |....abcd........|\n";

        let mut output = String::new();
        hexdump_very_ex_rs(&mut output, &input, test_display_addr).unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn test_hexdump_very_ex_raw() {
        let input = [0u8, 1, 2, 3, b'a', b'b', b'c', b'd'];
        let test_display_addr = 0x1000;
        let expected = "0x00001000: 03020100 64636261                   |....abcd........|\n";

        let mut output = String::new();
        // SAFETY: The static test array memory range is mapped and valid.
        unsafe {
            hexdump_very_ex_raw(&mut output, input.as_ptr(), input.len(), test_display_addr)
                .unwrap();
        }
        assert_eq!(output, expected);
    }

    #[test]
    fn test_hexdump8_very_ex() {
        let input = [0u8, 1, 2, 3, b'a', b'b', b'c', b'd'];
        let test_display_addr = 0x1000;
        let expected = "0x00001000: 00 01 02 03 61 62 63 64                         |....abcd\n";

        let mut output = String::new();
        hexdump8_very_ex_rs(&mut output, &input, test_display_addr).unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn test_hexdump8_very_ex_raw() {
        let input = [0u8, 1, 2, 3, b'a', b'b', b'c', b'd'];
        let test_display_addr = 0x1000;
        let expected = "0x00001000: 00 01 02 03 61 62 63 64                         |....abcd\n";

        let mut output = String::new();
        // SAFETY: The static test array memory range is mapped and valid.
        unsafe {
            hexdump8_very_ex_raw(&mut output, input.as_ptr(), input.len(), test_display_addr)
                .unwrap();
        }
        assert_eq!(output, expected);
    }
}
