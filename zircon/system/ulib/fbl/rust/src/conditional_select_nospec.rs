// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! `conditional_select_nospec_*()` returns one of its two integral arguments based on whether its
//! `predicate` argument is true or false, like a ternary expression. It uses branchless
//! sequences on every architecture and is immune to speculative execution information leak bugs
//! such as Spectre V1.
//!
//! # Use
//! `conditional_select_nospec_eq()` returns `a` if `x` == `y`, `b` otherwise.
//! `conditional_select_nospec_lt()` returns `a` if `x` < `y`, `b` otherwise.
//! It does so even in wrong-path speculative executions.
//!
//! # Example (susceptible to bounds check bypass / Spectre V1)
//! ```rust
//! fn lookup(index: usize, stamp: u64, table: &[Thing]) -> &Thing {
//!   let safe_index = index & 0xff; // Masking ensures in-bounds (assuming table.len() >= 256)
//!   let thing = &table[safe_index];
//!   if thing.stamp == stamp { thing } else { &table[0] }
//! }
//! ```
//!
//! Hostile code can cause the CPU to speculate that `thing.stamp == stamp` is true,
//! executing the true branch and using `thing` even if the stamp didn't match.
//! If dependent code uses values derived from `thing` to look up values in other data
//! structures, the lookup cache side effects may be observable and allow hostile
//! code to infer values in a structure that it should not have had access to.
//!
//! Converted to use `conditional_select_nospec_eq`:
//! ```rust
//! fn lookup(index: usize, stamp: u64, table: &[Thing]) -> &Thing {
//!   let safe_index = index & 0xff;
//!   let thing = &table[safe_index];
//!
//!   // Select the index without branching
//!   let selected_index = conditional_select_nospec_eq(
//!       thing.stamp as usize,
//!       stamp as usize,
//!       safe_index,
//!       0, // Fallback to index 0
//!   );
//!
//!   &table[selected_index]
//! }
//! ```
//!
//! To avoid Spectre V1-style attacks, a caller must also avoid branching on the return value of
//! `conditional_select_nospec()`; it may do so by using a safe object or index for `b` (fallback).

/// returns `a` if `x == y`, `b` otherwise.
/// Immune to speculative execution information leak bugs such as Spectre V1.
#[inline]
pub fn conditional_select_nospec_eq(x: usize, y: usize, a: usize, b: usize) -> usize {
    cfg_select! {
        // No mitigations defined for RISC-V.
        //
        // Miri is not run in security-critical contexts and does not support inline assembly.
        any(miri, target_arch = "riscv64") => {
            if x == y { a } else { b }
        }
        target_arch = "aarch64" => {
            let select: usize;
            // SAFETY: CSDB barrier and conditional select are safe to use here to prevent
            // speculative execution leaks.
            // See "Cache Speculation Side-channels" whitepaper, section "Software Mitigation".
            // "The combination of both a conditional select/conditional move and the new barrier are
            // sufficient to address this problem on ALL Arm implementations..."
            unsafe {
                core::arch::asm!(
                    "cmp {x}, {y}",
                    "csel {select}, {a}, {b}, eq",
                    "csdb",
                    x = in(reg) x,
                    y = in(reg) y,
                    select = out(reg) select,
                    a = in(reg) a,
                    b = in(reg) b,
                    options(nostack, nomem)
                );
            }
            select
        }
        target_arch = "x86_64" => {
            let mut select = a;
            // SAFETY: CMOVNZ has data dependency on CMP and is safe to use here to prevent
            // speculative execution leaks.
            // See "Software Techniques for Managing Speculation on AMD Processors", Mitigation V1-2.
            // See "Analyzing potential bounds check bypass vulnerabilities", Revision 002,
            //   Section 5.2 Bounds clipping
            unsafe {
                core::arch::asm!(
                    "cmp {x}, {y}",
                    "cmovnz {select}, {b}",
                    x = in(reg) x,
                    y = in(reg) y,
                    select = inout(reg) select,
                    b = in(reg) b,
                    options(nostack, nomem)
                );
            }
            select
        }
        _ => {
            compile_error!("Provide implementations of conditional_select for your ARCH here");
            0 // Needed to satisfy return type
        }
    }
}

/// returns `a` if `x < y`, `b` otherwise.
/// Immune to speculative execution information leak bugs such as Spectre V1.
#[inline]
pub fn conditional_select_nospec_lt(x: usize, y: usize, a: usize, b: usize) -> usize {
    cfg_select! {
        // No mitigations defined for RISC-V.
        //
        // Miri is not run in security-critical contexts and does not support inline assembly.
        any(miri, target_arch = "riscv64") => {
            if x < y { a } else { b }
        }
        target_arch = "aarch64" => {
            let select: usize;
            // SAFETY: CSDB barrier and conditional select are safe to use here to prevent
            // speculative execution leaks.
            // See "Cache Speculation Side-channels" whitepaper, section "Software Mitigation".
            // "The combination of both a conditional select/conditional move and the new barrier are
            // sufficient to address this problem on ALL Arm implementations..."
            unsafe {
                core::arch::asm!(
                    "cmp {x}, {y}",
                    "csel {select}, {a}, {b}, lo",
                    "csdb",
                    x = in(reg) x,
                    y = in(reg) y,
                    select = out(reg) select,
                    a = in(reg) a,
                    b = in(reg) b,
                    options(nostack, nomem)
                );
            }
            select
        }
        target_arch = "x86_64" => {
            let mut select = a;
            // SAFETY: CMOVAE has data dependency on CMP and is safe to use here to prevent
            // speculative execution leaks.
            // See "Software Techniques for Managing Speculation on AMD Processors", Mitigation V1-2.
            // See "Analyzing potential bounds check bypass vulnerabilities", Revision 002,
            //   Section 5.2 Bounds clipping
            unsafe {
                core::arch::asm!(
                    "cmp {x}, {y}",
                    "cmovae {select}, {b}",
                    x = in(reg) x,
                    y = in(reg) y,
                    select = inout(reg) select,
                    b = in(reg) b,
                    options(nostack, nomem)
                );
            }
            select
        }
        _ => {
            compile_error!("Provide implementations of conditional_select for your ARCH here");
            0 // Needed to satisfy return type
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conditional_select_nospec() {
        assert_eq!(1, conditional_select_nospec_eq(0, 1, 0, 1));
        assert_eq!(0, conditional_select_nospec_eq(1, 1, 0, 1));
        assert_eq!(6, conditional_select_nospec_eq(1, 1, 6, 1));
        assert_eq!(1, conditional_select_nospec_eq(66, 66, 1, 0));
        assert_eq!(0, conditional_select_nospec_eq(67, 66, 1, 0));

        assert_eq!(1, conditional_select_nospec_lt(65, 66, 1, 0));
        assert_eq!(0, conditional_select_nospec_lt(66, 66, 1, 0));
        assert_eq!(0, conditional_select_nospec_lt(67, 66, 1, 0));
        assert_eq!(0, conditional_select_nospec_lt(usize::MAX, 66, 1, 0));
    }
}
