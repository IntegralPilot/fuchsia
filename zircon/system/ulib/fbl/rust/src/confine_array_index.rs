// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! `confine_array_index()` bounds-checks and sanitizes an array index safely in the presence of
//! speculative execution information leak bugs such as Spectre V1. `confine_array_index()` always
//! returns a sanitized index, even in speculative-path execution.
//!
//! Callers need to combine `confine_array_index` with a conventional bounds check; the bounds
//! check will return any necessary errors in the nonspeculative path, `confine_array_index` will
//! confine indexes in the speculative path.
//!
//! # Use
//! `confine_array_index()` returns `index`, if it is < `size`, or 0 if `index` is >= `size`.
//!
//! # Example (may leak table1 contents)
//! ```rust
//! fn lookup(index: usize, table1: &[usize], table2: &[i32]) -> i32 {
//!   if index >= table1.len() {
//!     return -1;
//!   }
//!   let index2 = table1[index];
//!   table2[index2]
//! }
//! ```
//!
//! Converted:
//! ```rust
//! fn lookup(index: usize, table1: &[usize], table2: &[i32]) -> i32 {
//!   if index >= table1.len() {
//!     return -1;
//!   }
//!   let safe_index = confine_array_index(index, table1.len());
//!   let index2 = table1[safe_index];
//!   table2[index2]
//! }
//! ```

/// returns `index` if `index < size`, or `0` if `index >= size`.
/// Immune to speculative execution information leak bugs such as Spectre V1.
#[inline]
pub fn confine_array_index(index: usize, size: usize) -> usize {
    cfg_select! {
        // No mitigations defined for RISC-V.
        //
        // Miri is not run in security-critical contexts and does not support inline assembly.
        any(miri, target_arch = "riscv64") => {
            if index < size { index } else { 0 }
        }
        target_arch = "aarch64" => {
            let safe_index: usize;
            // SAFETY: CSDB barrier and conditional select are safe to use here to prevent
            // speculative execution leaks.
            // See "Cache Speculation Side-channels" whitepaper, section "Software Mitigation".
            // "The combination of both a conditional select/conditional move and the new barrier are
            // sufficient to address this problem on ALL Arm implementations..."
            unsafe {
                core::arch::asm!(
                    "cmp {index}, {size}",
                    "csel {safe_index}, {index}, xzr, lo",
                    "csdb",
                    index = in(reg) index,
                    size = in(reg) size,
                    safe_index = out(reg) safe_index,
                    options(nostack, nomem)
                );
            }
            safe_index
        }
        target_arch = "x86_64" => {
            let mut safe_index: usize = 0;
            // SAFETY: CMOVNZ has data dependency on CMP and is safe to use here to prevent
            // speculative execution leaks.
            // See "Software Techniques for Managing Speculation on AMD Processors", Mitigation V1-2.
            // See "Analyzing potential bounds check bypass vulnerabilities", Revision 002,
            //   Section 5.2 Bounds clipping
            unsafe {
                core::arch::asm!(
                    "cmp {size}, {index}",
                    "cmova {safe_index}, {index}",
                    size = in(reg) size,
                    index = in(reg) index,
                    safe_index = inout(reg) safe_index,
                    options(nostack, nomem)
                );
            }
            safe_index
        }
        _ => {
            compile_error!("Provide implementations of confine_array_indexs for your ARCH here");
            0 // Needed to satisfy return type
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confine_array_index() {
        const LIMIT: usize = 265;

        for i in 0..LIMIT {
            assert_eq!(i, confine_array_index(i, LIMIT));
        }

        for i in LIMIT..((LIMIT as f64 * 1.5) as usize) {
            assert_eq!(0, confine_array_index(i, LIMIT));
        }

        assert_eq!(0, confine_array_index(usize::MAX, LIMIT));
        assert_eq!(0, confine_array_index(usize::MAX - 1, 1));
        assert_eq!(0, confine_array_index(0, 1));
        assert_eq!(0, confine_array_index(1, 1));
    }
}
