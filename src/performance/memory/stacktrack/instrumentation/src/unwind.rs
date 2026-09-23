// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use stacktrack_vmo::threads_table_v1::Frame;
use std::num::NonZeroUsize;

// Implemented in helper.cc.
unsafe extern "C" {
    fn stacktrack_unwind_if_deeper(
        threshold_fp: u64,
        out_frames: *mut Frame,
        max_frames: usize,
    ) -> usize;
}

/// Unwinds the stack if it the bottom frame pointer is lower than a threshold.
///
/// Returns:
/// - `None` if the stack was not deeper than the threshold.
/// - Otherwise `Some(count)` where `count` is the number of unwound frames.
pub fn unwind_if_deeper(threshold_fp: u64, buffer: &mut [Frame]) -> Option<NonZeroUsize> {
    // SAFETY: We are passing a valid pointer to the destination buffer and its size.
    let count =
        unsafe { stacktrack_unwind_if_deeper(threshold_fp, buffer.as_mut_ptr(), buffer.len()) };

    // Return None if count == 0, or Some(count) otherwise.
    NonZeroUsize::new(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zx_libc::sanitizer::fast_backtrace;

    #[inline(never)]
    fn test_unwind_inner_3() {
        let mut local_buf = [Frame::default(); 128];
        let count = unwind_if_deeper(0, &mut local_buf).expect("unwind failed");
        let local_buf = &local_buf[..count.get()];

        // Capture using __sanitizer_fast_backtrace as a reference.
        let mut reference_buf = [0; 128];
        let reference_buf = fast_backtrace(&mut reference_buf);

        // Count matching frames at the top of the stack (longest common suffix).
        let mut match_count = 0;
        for i in 1..=std::cmp::min(local_buf.len(), reference_buf.len()) {
            let local_pc = local_buf[local_buf.len() - i].pc;
            let ref_pc = reference_buf[reference_buf.len() - i] as u64;

            if local_pc == ref_pc {
                match_count += 1;
            } else {
                break;
            }
        }

        assert!(
            match_count > 3,
            "Stack traces do not have a long enough common suffix: {:x?} vs {:x?}",
            local_buf,
            reference_buf
        );
    }

    #[inline(never)]
    fn test_unwind_inner_2() {
        test_unwind_inner_3();
    }

    #[inline(never)]
    fn test_unwind_inner_1() {
        test_unwind_inner_2();
    }

    #[test]
    fn test_unwind() {
        // Run the real test function under three levels of call frames to make sure there are at le.
        test_unwind_inner_1();
    }
}
