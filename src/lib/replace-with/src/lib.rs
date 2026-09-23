// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `replace-with` provides the [`replace_with`] function.

use std::{mem, ptr};

/// Uses `f` to replace the referent of `dst` with a new value.
///
/// Reads the current value in `dst`, calls `f` on that value, and overwrites
/// `dst` using the new value returned by `f`. If `f` panics, the process is
/// aborted.
///
/// This is useful for updating a value whose type is not [`Copy`].
///
/// # Examples
///
/// ```rust
/// # use replace_with::replace_with;
/// /// A value that might be stored on the heap (boxed) or on the stack (unboxed).
/// pub enum MaybeBoxed<T> {
///     Boxed(Box<T>),
///     Unboxed(T),
/// }
///
/// impl<T> MaybeBoxed<T> {
///     /// Ensures that `self` is boxed, moving the value to the heap if necessary.
///     pub fn ensure_boxed(&mut self) {
///         replace_with(self, |m| match m {
///             MaybeBoxed::Boxed(b) => MaybeBoxed::Boxed(b),
///             MaybeBoxed::Unboxed(u) => MaybeBoxed::Boxed(Box::new(u)),
///         })
///     }
/// }
/// ```
pub fn replace_with<T, F: FnOnce(T) -> T>(dst: &mut T, f: F) {
    replace_with_and(dst, move |t| (f(t), ()))
}

/// Uses `f` to replace the referent of `dst` and returns a value from the
/// transformation.
///
/// Like [`replace_with`] but the provided function returns a tuple of `(T, R`)
/// where `T` is the new value for `dst` and `R` is returned from
/// `replace_with_and`.
pub fn replace_with_and<T, R, F: FnOnce(T) -> (T, R)>(dst: &mut T, f: F) -> R {
    // This is not necessary today, but it may be necessary if the "strict
    // pointer provenance" model [1] is adopted in the future.
    //
    // [1] https://github.com/rust-lang/rust/issues/95228
    let dst = dst as *mut T;

    // SAFETY:
    // - The initial `ptr::read` is sound because `dst` is derived from a `&mut
    //   T`, and so all of `ptr::read`'s safety preconditions are satisfied:
    //   - `dst` is valid for reads
    //   - `dst` is properly aligned
    //   - `dst` points at a properly initialized value of type `T`
    // - After `ptr::read` is called, we've created a copy of `*dst`. Since `T:
    //   !Copy`, it is not guaranteed that operating on both copies would be
    //   sound. Since we allow `f` to operate on `old`, we have to ensure that
    //   no code operates on `*dst`. This could happen in a few circumstances:
    //   - Code in this function could operate on `*dst`, which it doesn't.
    //   - Code in `f` could operate on `*dst`. Since `dst` is a mutable
    //     reference, and it is borrowed for the duration of this function call,
    //     `f` cannot also access `dst` (code that attempted to do that would
    //     fail to compile).
    //   - The caller could operate on `dst` after the function returns. There
    //     are two cases:
    //     - In the success case, `f` returns without panicking. It returns a
    //       new `T`, and we overwrite `*dst` with this new `T` using
    //       `ptr::write`. At this point, it is sound for code to operate on
    //       `*dst`, and so it is sound for this function to return.
    //     - In the failure case, `f` panics. Since, at the point we call `f`,
    //       we have not overwritten `*dst` yet, it would be unsound if the
    //       panic were to unwind the stack, allowing code from the caller to
    //       run. Since we call `f` within a call to `abort_on_panic`, we are
    //       guaranteed that the process would abort, and no future code could
    //       run.
    // - The call to `ptr::write` itself is sound because, thanks to `dst`
    //   being derived from a `&mut T`, all of `ptr::write`'s preconditions are
    //   satisfied:
    //   - `dst` is valid for writes
    //   - `dst` is properly aligned
    unsafe {
        let old = ptr::read(dst);
        let (new, ret) = abort_on_panic(move || f(old));
        ptr::write(dst, new);
        ret
    }
}

/// Calls `f` or aborts the process if `f` panics.
fn abort_on_panic<T, F: FnOnce() -> T>(f: F) -> T {
    struct CallOnDrop<O, F: Fn() -> O>(F);
    impl<O, F: Fn() -> O> Drop for CallOnDrop<O, F> {
        #[cold]
        fn drop(&mut self) {
            (self.0)();
        }
    }

    let backtrace_and_abort_on_drop = CallOnDrop(|| {
        // SAFETY: This guard ensures that we abort in both of the following two
        // cases:
        // - The code executes normally (the guard is dropped at the end of the
        //   function)
        // - The backtrace code panics (the guard is dropped during unwinding)
        //
        // No functions called from the backtrace code are documented to panic,
        // but this serves as a hedge in case there are undocumented panic
        // conditions.
        let abort_on_drop = CallOnDrop(std::process::abort);

        use std::io::Write as _;
        let backtrace = std::backtrace::Backtrace::force_capture();
        let mut stderr = std::io::stderr().lock();
        // We treat backtrace-printing as best-effort, so we ignore any errors.
        let _ = write!(&mut stderr, "replace_with: callback panicked; backtrace:\n{backtrace}\n");
        let _ = stderr.flush();

        mem::drop(abort_on_drop);
    });

    let t = f();
    mem::forget(backtrace_and_abort_on_drop);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_with() {
        let mut x = 1usize;
        replace_with(&mut x, |x| x * 2);
        assert_eq!(x, 2);
    }
}
