// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A guard that executes an action when dropped, unless cancelled.
///
/// This provides equivalent functionality to C++ `fit::deferred_action` and `fit::defer`.
#[must_use = "if unused the deferred action will execute immediately"]
pub struct Deferred<F: FnOnce()> {
    action: Option<F>,
}

impl<F: FnOnce()> Deferred<F> {
    /// Creates a new `Deferred` action that will run when dropped.
    #[inline]
    pub const fn new(action: F) -> Self {
        Self { action: Some(action) }
    }

    /// Cancels the deferred action so that it will not run upon drop.
    #[inline]
    pub fn cancel(&mut self) {
        self.action = None;
    }

    /// Executes the deferred action immediately and cancels it.
    #[inline]
    pub fn call(&mut self) {
        if let Some(action) = self.action.take() {
            action();
        }
    }
}

impl<F: FnOnce()> Drop for Deferred<F> {
    #[inline]
    fn drop(&mut self) {
        if let Some(action) = self.action.take() {
            action();
        }
    }
}

/// Schedules an action to be executed when the returned guard is dropped.
///
/// If the returned guard is dropped, the closure `action` will be executed.
/// Call [`Deferred::cancel`] on the returned guard to prevent execution.
#[inline]
pub fn defer<F: FnOnce()>(action: F) -> Deferred<F> {
    Deferred::new(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defer_runs_on_drop() {
        let mut executed = false;
        {
            let _guard = defer(|| {
                executed = true;
            });
            assert!(!executed);
        }
        assert!(executed);
    }

    #[test]
    fn test_defer_cancel() {
        let mut executed = false;
        {
            let mut guard = defer(|| {
                executed = true;
            });
            guard.cancel();
        }
        assert!(!executed);
    }

    #[test]
    fn test_defer_call_early() {
        let mut count = 0;
        {
            let mut guard = defer(|| {
                count += 1;
            });
            assert_eq!(count, 0);
            guard.call();
            assert_eq!(count, 1);
        }
        // Should not run again on drop
        assert_eq!(count, 1);
    }
}
