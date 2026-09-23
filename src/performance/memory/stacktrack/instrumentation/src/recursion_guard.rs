// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::Cell;

thread_local! {
    /// Whether the current thread is currently in a `with_recursion_guard` call or not.
    static RECURSION_GUARD: Cell<bool> = const { Cell::new(false) };
}

/// Executes the given function in a special context that cannot be recursively re-entered
/// on the same execution stack.
///
/// If re-entered, the function is silently not executed.
pub fn with_soft_recursion_guard(f: impl FnOnce()) {
    RECURSION_GUARD.with(|cell| {
        let was_already_acquired = cell.replace(true);
        if was_already_acquired {
            return;
        }

        f();

        cell.set(false);
    })
}

/// Executes the given function in a special context that cannot be recursively re-entered
/// on the same execution stack.
///
/// If re-entered, the function panics.
pub fn with_hard_recursion_guard(f: impl FnOnce()) {
    RECURSION_GUARD.with(|cell| {
        let was_already_acquired = cell.replace(true);
        assert!(!was_already_acquired);

        f();

        cell.set(false);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_soft() {
        let count = AtomicUsize::new(0);
        with_soft_recursion_guard(|| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_soft_then_soft() {
        let count = AtomicUsize::new(0);
        with_soft_recursion_guard(|| {
            count.fetch_add(1, Ordering::SeqCst);
            with_soft_recursion_guard(|| {
                count.fetch_add(1, Ordering::SeqCst);
            });
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic]
    fn test_soft_then_hard() {
        with_soft_recursion_guard(|| {
            with_hard_recursion_guard(|| {
                // Should panic
            });
        });
    }

    #[test]
    fn test_hard() {
        let count = AtomicUsize::new(0);
        with_hard_recursion_guard(|| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_hard_then_soft() {
        let count = AtomicUsize::new(0);
        with_hard_recursion_guard(|| {
            count.fetch_add(1, Ordering::SeqCst);
            with_soft_recursion_guard(|| {
                count.fetch_add(1, Ordering::SeqCst);
            });
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic]
    fn test_hard_then_hard() {
        with_hard_recursion_guard(|| {
            with_hard_recursion_guard(|| {
                // Should panic
            });
        });
    }
}
