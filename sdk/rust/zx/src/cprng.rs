// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for the Zircon kernel's CPRNG.
//!
use crate::{ok, Status};
use std::mem::MaybeUninit;

/// Draw random bytes from the kernel's CPRNG to fill the provided buffer. This function always
/// fills the buffer.
///
/// # Safety
///
/// The provided pointer must be valid to write to for the provided number of bytes.
///
/// Wraps the
/// [zx_cprng_draw](https://fuchsia.dev/fuchsia-src/reference/syscalls/cprng_draw.md)
/// syscall.
pub use crate::sys::zx_cprng_draw as cprng_draw_raw;

/// Draw random bytes from the kernel's CPRNG to fill `buffer`. This function
/// always fills the buffer.
///
/// Wraps the
/// [zx_cprng_draw](https://fuchsia.dev/fuchsia-src/reference/syscalls/cprng_draw.md)
/// syscall.
pub fn cprng_draw(buffer: &mut [u8]) {
    // SAFETY: &[u8] and &[MaybeUninit<u8>] have the same layout.
    let buffer = unsafe {
        std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<MaybeUninit<u8>>(), buffer.len())
    };

    cprng_draw_uninit(buffer);
}

/// Draw random bytes from the kernel's CPRNG to fill `buffer`. This function
/// always fills the buffer.
///
/// Wraps the
/// [zx_cprng_draw](https://fuchsia.dev/fuchsia-src/reference/syscalls/cprng_draw.md)
/// syscall.
///
/// Returns the filled buffer.
pub fn cprng_draw_uninit(buffer: &mut [MaybeUninit<u8>]) -> &mut [u8] {
    // SAFETY: `buffer` is a well formed slice of (maybe) uninitialized `u8`s.
    // `MaybeUninit<T>` and `T` have the same layout.
    unsafe { cprng_draw_raw(buffer.as_mut_ptr().cast::<u8>(), buffer.len()) };

    // SAFETY: We're converting &mut [MaybeUninit<u8>] back to &mut [u8], which is only
    // valid to do if all elements of `buffer` have actually been initialized. Here we
    // have to trust that the kernel didn't lie when it said it wrote to the entire
    // buffer, but as long as that assumption is valid them it's safe to assume this
    // slice is init.
    //
    // TODO(https://fxbug.dev/42079723) use MaybeUninit::slice_assume_init_mut when stable
    unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len()) }
}

/// Mix the given entropy into the kernel CPRNG.
///
/// The buffer must have length less than `ZX_CPRNG_ADD_ENTROPY_MAX_LEN`.
///
/// Wraps the
/// [zx_cprng_add_entropy](https://fuchsia.dev/fuchsia-src/reference/syscalls/cprng_add_entropy.md)
/// syscall.
pub fn cprng_add_entropy(buffer: &[u8]) -> Result<(), Status> {
    let status = unsafe { crate::sys::zx_cprng_add_entropy(buffer.as_ptr(), buffer.len()) };
    ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_buffer(buffer: &mut [u8]) {
        let mut first_zero = 0;
        let mut last_zero = 0;
        for _ in 0..30 {
            cprng_draw(buffer);
            if buffer[0] == 0 {
                first_zero += 1;
            }
            if buffer.len() > 1 && buffer[buffer.len() - 1] == 0 {
                last_zero += 1;
            }
        }
        assert_ne!(first_zero, 30);
        assert_ne!(last_zero, 30);
    }

    #[test]
    fn cprng() {
        let mut buffer = [0; 20];
        check_buffer(&mut buffer);
    }

    #[test]
    fn cprng_large() {
        // The kernel currently chunks the buffer if it's larger than 256.
        const SIZE: usize = 257;
        let mut buffer = [0; SIZE];
        check_buffer(&mut buffer);

        for mut s in buffer.chunks_mut(SIZE / 3) {
            check_buffer(&mut s);
        }
    }

    #[test]
    fn cprng_add() {
        let buffer = [0, 1, 2];
        assert_eq!(cprng_add_entropy(&buffer), Ok(()));
    }
}
