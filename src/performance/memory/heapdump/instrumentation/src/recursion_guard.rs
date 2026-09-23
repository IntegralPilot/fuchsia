// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::Cell;

/// Executes the given function in a special context that cannot be recursively re-entered
/// on the same execution stack.
///
/// It can be used to prevent unwanted recursive function executions.
pub fn with_recursion_guard<T>(f: impl FnOnce() -> T) -> T {
    match with_recursion_guard_impl(f) {
        Ok(result) => result,
        Err(UnwantedRecursionError) => {
            // WARNING! Do not call panic! because it may itself allocate and cause further
            // recursion. This still results in a backtrace in the log.
            std::process::abort()
        }
    }
}

thread_local! {
    /// Whether the current thread is currently in a `with_recursion_guard_impl` call or not.
    static RECURSION_GUARD: Cell<bool> = const { Cell::new(false) };
}

#[derive(Debug)]
struct UnwantedRecursionError;

fn with_recursion_guard_impl<T>(f: impl FnOnce() -> T) -> Result<T, UnwantedRecursionError> {
    RECURSION_GUARD.with(|cell| {
        let was_already_acquired = cell.replace(true);
        if was_already_acquired {
            return Err(UnwantedRecursionError);
        }

        let result = f();

        cell.set(false);

        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    // Verify that executing a non-recursive function succeeds.
    #[test]
    fn test_recursion_guard_ok() {
        let result = with_recursion_guard_impl(|| 42);
        assert_matches!(result, Ok(42));
    }

    // Verify that the inner recursive call fails.
    #[test]
    fn test_recursion_guard_violation() {
        let result = with_recursion_guard_impl(|| with_recursion_guard_impl(|| 42));
        assert_matches!(result, Ok(Err(UnwantedRecursionError)));
    }
}
