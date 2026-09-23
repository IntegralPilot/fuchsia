// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg_attr(not(test), no_std)]

// Allows for the suite macro to work within this crate's test module.
#[cfg(test)]
extern crate self as unittest;

use core::ffi::{CStr, c_char};
use core::slice;

#[doc(hidden)]
pub use zx_status::Status as __Status;
#[doc(hidden)]
pub use zx_status::sys as __sys;

/// Attribute macro defining suite of unit tests defined as module. The
/// attribute may only be used in a cfg(ktest) context.
///
/// Tests are idiomatically modeled as functions, and so modules of such
/// functions make for a natural representation as a test suite.
///
/// Tests defined through this attribute are available to be run via `k ut`.
///
/// A test suite module must meet the following criteria:
/// * It must have a one-line docstring. This line becomes the description of
///   the suite on the kernel command-line.
/// * It must contain at least one test function; it may contain any other items.
///
/// A test function must meet the following criteria:
/// * It must be annotated with #[test].
/// * It too must have a one-line docstring. This line becomes the description
///   of the test on the kernel command-line.
/// * It must have a () -> () signature.
///
/// A test function should make assertions only using the declarative
/// assert_*/expect_* macros defined in the unittest module.
///
/// Disabling a test can be done with a further annotation of #[ignore]. Such a
/// test will still be compiled; it just will not contribute test metadata.
///
/// # Example
/// ```rust
/// /// Brief test suite description.
/// #[cfg(ktest)]
/// #[unittest::suite(name = "optional_name")]
/// mod my_suite {
///     /* non-test items... */
///
///     /// Brief test case description.
///     #[test]
///     fn my_case() {
///         assert_false!(false);
///         expect_true!(1 == 1, "expectation with a message");
///     }
///
///     /// This test case is currently disabled.
///     #[test]
///     #[ignore]
///     fn my_disabled_case() {...}
/// }
/// ```
///
pub use unittest_macro::suite;

use kprint::kprintln;

#[doc(hidden)]
pub fn print_comparison_failure(
    file: &str,
    line: u32,
    expected: &str,
    expected_val: isize,
    op: &str,
    actual: &str,
    actual_val: isize,
    msg: &str,
) {
    kprintln!(
        "\n    [FAILED]\n    {:s}:{}:\n    expected {:s} ({}) {:s} {:s} ({})\n    {:s}",
        file,
        line,
        expected,
        expected_val,
        op,
        actual,
        actual_val,
        msg,
    );
}

#[doc(hidden)]
pub fn print_condition_failure(file: &str, line: u32, desc: &str, actual: &str, msg: &str) {
    kprintln!(
        "\n    [FAILED]\n    {:s}:{}:\n    {:s} {:s}\n    {:s}",
        file,
        line,
        actual,
        desc,
        msg,
    );
}

#[macro_export]
#[doc(hidden)]
macro_rules! check_comparison {
    ($cond:expr, $early_return:expr, $op:literal, $expected:expr, $expected_val:expr, $actual:expr, $actual_val:expr, $msg:expr) => {
        if !$cond {
            record_failure!();
            $crate::print_comparison_failure(
                file!(),
                line!(),
                stringify!($expected),
                $expected_val as isize,
                $op,
                stringify!($actual),
                $actual_val as isize,
                $msg,
            );
            if $early_return {
                return false;
            }
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! check_condition {
    ($cond:expr, $early_return:expr, $desc:literal, $actual:expr, $msg:expr) => {
        if !$cond {
            record_failure!();
            $crate::print_condition_failure(file!(), line!(), $desc, stringify!($actual), $msg);
            if $early_return {
                return false;
            }
        }
    };
}

/// The data structure that statically defines a test suite, intended to be
/// defined via the #[suite] macro to encoded into a special section in
/// the kernel.
#[doc(hidden)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TestSuiteRegistration {
    pub name: *const c_char,
    pub desc: *const c_char,
    pub tests: *const TestCaseRegistration,
    pub test_cnt: usize,
}

unsafe impl Sync for TestSuiteRegistration {}

impl TestSuiteRegistration {
    /// The name of the suite.
    ///
    /// Registrations are emitted by the #[suite] macro as compile-time
    /// constants, so a missing or non-UTF-8 name is a programming error.
    pub fn name(&self) -> &'_ str {
        assert!(!self.name.is_null(), "test suite registration has no name");
        // Safety: the name is a static, NUL-terminated string.
        unsafe { CStr::from_ptr(self.name) }.to_str().expect("test suite name is not valid UTF-8")
    }

    /// The description of the suite, if it has one.
    pub fn desc(&self) -> Option<&'_ str> {
        if self.desc.is_null() {
            return None;
        }
        // Safety: the description is a static, NUL-terminated string.
        let desc = unsafe { CStr::from_ptr(self.desc) };
        Some(desc.to_str().expect("test suite description is not valid UTF-8"))
    }

    /// The test cases of the suite.
    pub fn cases(&self) -> &'_ [TestCaseRegistration] {
        if self.test_cnt == 0 {
            return &[];
        }
        assert!(!self.tests.is_null(), "test suite registration has tests but no test array");
        // Safety: the registration records `test_cnt` contiguous test cases.
        unsafe { slice::from_raw_parts(self.tests, self.test_cnt) }
    }
}

/// The data structure defining a test case within a suite, also intended be
/// defined via the #[suite] macro to encoded into a special section in
/// the kernel
#[doc(hidden)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TestCaseRegistration {
    pub name: *const c_char,
    pub fn_: extern "C" fn() -> bool,
}

unsafe impl Sync for TestCaseRegistration {}

impl TestCaseRegistration {
    /// The name of the test case.
    ///
    /// Registrations are emitted by the #[suite] macro as compile-time
    /// constants, so a missing or non-UTF-8 name is a programming error.
    pub fn name(&self) -> &'_ str {
        assert!(!self.name.is_null(), "test case registration has no name");
        // Safety: the name is a static, NUL-terminated string.
        unsafe { CStr::from_ptr(self.name) }.to_str().expect("test case name is not valid UTF-8")
    }
}

/// Asserts that two expressions are equal, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_eq {
    ($expected:expr, $actual:expr) => {
        $crate::expect_eq!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e == a, false, "==", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that two expressions are equal and short-circuits on failure.
#[macro_export]
macro_rules! assert_eq {
    ($expected:expr, $actual:expr) => {
        $crate::assert_eq!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e == a, true, "==", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that two expressions are not equal, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_ne {
    ($expected:expr, $actual:expr) => {
        $crate::expect_ne!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e != a, false, "!=", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that two expressions are not equal and short-circuits on failure.
#[macro_export]
macro_rules! assert_ne {
    ($expected:expr, $actual:expr) => {
        $crate::assert_ne!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e != a, true, "!=", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is less than the second, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_lt {
    ($expected:expr, $actual:expr) => {
        $crate::expect_lt!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e < a, false, "<", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is less than the second and short-circuits on failure.
#[macro_export]
macro_rules! assert_lt {
    ($expected:expr, $actual:expr) => {
        $crate::assert_lt!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e < a, true, "<", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is less than or equal to the second, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_le {
    ($expected:expr, $actual:expr) => {
        $crate::expect_le!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e <= a, false, "<=", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is less than or equal to the second and short-circuits on failure.
#[macro_export]
macro_rules! assert_le {
    ($expected:expr, $actual:expr) => {
        $crate::assert_le!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e <= a, true, "<=", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is greater than the second, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_gt {
    ($expected:expr, $actual:expr) => {
        $crate::expect_gt!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e > a, false, ">", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is greater than the second and short-circuits on failure.
#[macro_export]
macro_rules! assert_gt {
    ($expected:expr, $actual:expr) => {
        $crate::assert_gt!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e > a, true, ">", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is greater than or equal to the second, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_ge {
    ($expected:expr, $actual:expr) => {
        $crate::expect_ge!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e >= a, false, ">=", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the first expression is greater than or equal to the second and short-circuits on failure.
#[macro_export]
macro_rules! assert_ge {
    ($expected:expr, $actual:expr) => {
        $crate::assert_ge!($expected, $actual, "")
    };
    ($expected:expr, $actual:expr, $msg:expr) => {
        let e = $expected;
        let a = $actual;
        $crate::check_comparison!(e >= a, true, ">=", $expected, e, $actual, a, $msg);
    };
}

/// Asserts that the expression evaluates to true, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_true {
    ($actual:expr) => {
        $crate::expect_true!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(a, false, "is false", $actual, $msg);
    };
}

/// Asserts that the expression evaluates to true and short-circuits on failure.
#[macro_export]
macro_rules! assert_true {
    ($actual:expr) => {
        $crate::assert_true!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(a, true, "is false", $actual, $msg);
    };
}

/// Asserts that the expression evaluates to false, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_false {
    ($actual:expr) => {
        $crate::expect_false!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(!a, false, "is true", $actual, $msg);
    };
}

/// Asserts that the expression evaluates to false and short-circuits on failure.
#[macro_export]
macro_rules! assert_false {
    ($actual:expr) => {
        $crate::assert_false!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(!a, true, "is true", $actual, $msg);
    };
}

/// Asserts that the pointer is null, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_null {
    ($actual:expr) => {
        $crate::expect_null!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(a.is_null(), false, "is non-null!", $actual, $msg);
    };
}

/// Asserts that the pointer is null and short-circuits on failure.
#[macro_export]
macro_rules! assert_null {
    ($actual:expr) => {
        $crate::assert_null!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(a.is_null(), true, "is non-null!", $actual, $msg);
    };
}

/// Asserts that the pointer is non-null, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_nonnull {
    ($actual:expr) => {
        $crate::expect_nonnull!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(!a.is_null(), false, "is null!", $actual, $msg);
    };
}

/// Asserts that the pointer is non-null and short-circuits on failure.
#[macro_export]
macro_rules! assert_nonnull {
    ($actual:expr) => {
        $crate::assert_nonnull!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a = $actual;
        $crate::check_condition!(!a.is_null(), true, "is null!", $actual, $msg);
    };
}

#[doc(hidden)]
pub fn status_name_and_raw<T>(res: Result<T, __Status>) -> (&'static str, core::ffi::c_int) {
    match res {
        Ok(_) => ("OK", __sys::ZX_OK),
        Err(err) => (err.as_str(), err.into_raw()),
    }
}

#[doc(hidden)]
pub fn print_status_comparison_failure<T>(
    file: &str,
    line: u32,
    expected: &str,
    expected_status: Result<T, __Status>,
    actual: &str,
    actual_status: Result<T, __Status>,
    msg: &str,
) {
    let (expected_name, expected_raw) = status_name_and_raw(expected_status);
    let (actual_name, actual_raw) = status_name_and_raw(actual_status);
    kprintln!(
        "\n    [FAILED]\n    {:s}:{}:\n    expected {:s} ({:s}, {}) == {:s} ({:s}, {})\n    {:s}",
        file,
        line,
        expected,
        expected_name,
        expected_raw,
        actual,
        actual_name,
        actual_raw,
        msg,
    );
}

#[macro_export]
#[doc(hidden)]
macro_rules! check_status_comparison {
    (
        $cond:expr,
        $early_return:expr,
        $expected_expr:expr,
        $expected_status:expr,
        $actual_expr:expr,
        $actual_status:expr,
        $msg:expr
    ) => {
        if !$cond {
            record_failure!();
            $crate::print_status_comparison_failure(
                file!(),
                line!(),
                stringify!($expected_expr),
                $expected_status,
                stringify!($actual_expr),
                $actual_status,
                $msg,
            );
            if $early_return {
                return false;
            }
        }
    };
}

/// Asserts that the expression evaluates to OK, but does not short-circuit on failure.
#[macro_export]
macro_rules! expect_ok {
    ($actual:expr) => {
        $crate::expect_ok!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a: Result<(), ::unittest::__Status> = $actual.into();
        $crate::check_status_comparison!(a.is_ok(), false, "Ok(())", Ok(()), $actual, a, $msg);
    };
}

/// Asserts that the expression evaluates to OK and short-circuits on failure.
#[macro_export]
macro_rules! assert_ok {
    ($actual:expr) => {
        $crate::assert_ok!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        let a: Result<(), ::unittest::__Status> = $actual.into();
        $crate::check_status_comparison!(a.is_ok(), true, "Ok(())", Ok(()), $actual, a, $msg);
    };
}

/// Asserts that the expression evaluates to the specified error status, but does not short-circuit
/// on failure.
#[macro_export]
macro_rules! expect_err {
    ($actual:expr, $expected_err:expr) => {
        $crate::expect_err!($actual, $expected_err, "")
    };
    ($actual:expr, $expected_err:expr, $msg:expr) => {
        let a: Result<_, ::unittest::__Status> = $actual.into();
        let e: ::unittest::__Status = $expected_err.into();
        $crate::check_status_comparison!(
            a.as_ref().is_err_and(|err| *err == e),
            false,
            $expected_err,
            Err(e),
            $actual,
            a,
            $msg
        );
    };
}

/// Asserts that the expression evaluates to the specified error status and short-circuits on
/// failure.
#[macro_export]
macro_rules! assert_err {
    ($actual:expr, $expected_err:expr) => {
        $crate::assert_err!($actual, $expected_err, "")
    };
    ($actual:expr, $expected_err:expr, $msg:expr) => {
        let a: Result<_, ::unittest::__Status> = $actual.into();
        let e: ::unittest::__Status = $expected_err.into();
        $crate::check_status_comparison!(
            a.as_ref().is_err_and(|err| *err == e),
            true,
            $expected_err,
            Err(e),
            $actual,
            a,
            $msg
        );
    };
}

/// Asserts that the expression evaluates to Result::Ok and returns the resulting value, otherwise
/// short-circuits.
#[macro_export]
macro_rules! unwrap_ok {
    ($actual:expr) => {
        $crate::unwrap_ok!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        match ($actual) {
            Ok(r) => r,
            Err(err) => {
                let err: ::unittest::__Status = err.into();
                $crate::check_status_comparison!(
                    false,
                    true,
                    "Ok(())",
                    Ok(()),
                    $actual,
                    Err(err),
                    $msg
                );
                return false;
            }
        }
    };
}

/// Asserts that the expression evaluates to Option::Some and returns the resulting value, otherwise
/// short-circuits.
#[macro_export]
macro_rules! unwrap_some {
    ($actual:expr) => {
        $crate::unwrap_some!($actual, "")
    };
    ($actual:expr, $msg:expr) => {
        match ($actual) {
            Option::Some(val) => val,
            Option::None => {
                $crate::check_condition!(false, true, "is None!", $actual, $msg);
                return false;
            }
        }
    };
}

/// Creates an isolated subtest closure with its own failure tracking.
///
/// Within a subtest, assertions (`expect_*!`, `assert_*!`, `unwrap_*!`) record failures to the
/// subtest's local status flag rather than the enclosing test's flag. Hard assertions (`assert_*!`,
/// `unwrap_*!`) return `false` early from the subtest without returning from the enclosing test.
///
/// Returns `bool`: `true` if all assertions in the subtest passed, `false` otherwise.
#[macro_export]
macro_rules! subtest {
    (|$($param:ident : $ty:ty),* $(,)?| $body:block) => {
        |$($param : $ty),*| -> bool {
            // Unused when the subtest body contains no soft assertions.
            #[allow(unused_mut)]
            let mut all_ok = true;
            macro_rules! record_failure {
                () => {
                    // Unused when an early-return assertion mutates all_ok before returning.
                    #[allow(unused_assignments)]
                    {
                        all_ok = false;
                    }
                };
            }
            $body
            all_ok
        }
    };
}

// When building this crate with unit tests we also pass `--cfg ktest` to
// enable the unconditional use of #[suite] below.
#[cfg(test)]
mod tests {
    use core::ffi::CStr;
    use core::{ptr, slice};
    use std::cell::Cell;
    use std::vec::Vec;

    use super::{TestSuiteRegistration, suite};

    unsafe extern "C" {
        static __start_unittest_testcases: TestSuiteRegistration;
        static __stop_unittest_testcases: TestSuiteRegistration;
    }

    // Thread-local since #[test] instances are run in parallel.
    thread_local! {
        static END_REACHED: Cell<bool> = Cell::new(false);
    }

    fn mark_end_as_reached() {
        END_REACHED.with(|cell| cell.set(true));
    }

    fn mark_end_as_not_reached() {
        END_REACHED.with(|cell| cell.set(false));
    }

    fn expect_end_reached() {
        std::assert_eq!(END_REACHED.with(|cell| cell.get()), true);
    }

    fn expect_end_not_reached() {
        std::assert_eq!(END_REACHED.with(|cell| cell.get()), false);
    }

    fn get_suites() -> Vec<TestSuiteRegistration> {
        let start = unsafe { &__start_unittest_testcases as *const TestSuiteRegistration };
        let stop = unsafe { &__stop_unittest_testcases as *const TestSuiteRegistration };

        let count = unsafe { stop.offset_from(start) } as usize;

        let suites_rodata = unsafe { slice::from_raw_parts(start, count) };

        let mut suites = Vec::from(suites_rodata);

        suites.sort_by(|a, b| {
            let a_name = unsafe { CStr::from_ptr(a.name) };
            let b_name = unsafe { CStr::from_ptr(b.name) };
            a_name.cmp(b_name)
        });
        suites
    }

    /// Suite with one function description.
    #[suite(name = "one_function")]
    mod suite_with_one_function {
        /// Empty function description.
        #[test]
        fn empty() {}
    }

    /// Suite with non-test items.
    #[suite]
    mod suite_with_other_items {
        use std::vec;

        trait Countable {
            fn count(&self) -> usize;
        }

        impl<T> Countable for Vec<T> {
            fn count(&self) -> usize {
                self.len()
            }
        }

        fn get_count<T: Countable>(countable: T) -> usize {
            countable.count()
        }

        /// Check use statement.
        #[test]
        fn check_other_items() {
            let v = vec![1, 2, 3];
            expect_eq!(get_count(v), 3);
        }
    }

    /// Suite with ignored test.
    #[suite(name = "with_ignored")]
    mod suite_with_ignored {
        /// Ignored test.
        #[ignore]
        #[test]
        fn ignored_test() {
            assert_true!(false);
        }

        /// Normal test.
        #[test]
        fn normal_test() {}
    }

    /// Assertion tests description.
    #[suite]
    mod assertions {
        /// Success cases.
        #[test]
        fn test_success() {
            assert_eq!(1, 1);
            assert_ne!(1, 2);
            assert_lt!(1, 2);
            assert_le!(1, 1);
            assert_gt!(2, 1);
            assert_ge!(2, 2);

            assert_true!(true);
            assert_false!(false);

            let null_ptr: *const i32 = ptr::null();
            let nonnull_ptr: *const i32 = &42 as *const i32;
            assert_null!(null_ptr);
            assert_nonnull!(nonnull_ptr);

            assert_ok!(Ok::<(), zx_status::Status>(()));
            assert_err!(
                Err::<(), _>(zx_status::Status::INVALID_ARGS),
                zx_status::Status::INVALID_ARGS
            );
            assert_err!(zx_status::Status::INVALID_ARGS, zx_status::Status::INVALID_ARGS);

            let _ = unwrap_ok!(Ok::<(), zx_status::Status>(()));
            let _ = unwrap_some!(Some(42));

            mark_end_as_reached();
        }

        /// Test that assert_eq fails on inequality.
        #[test]
        fn fail_assert_eq() {
            assert_eq!(1, 2);
            mark_end_as_reached();
        }

        /// Test that assert_ne fails on equality.
        #[test]
        fn fail_assert_ne() {
            assert_ne!(1, 1);
            mark_end_as_reached();
        }

        /// Test that assert_lt fails when not less-than.
        #[test]
        fn fail_assert_lt() {
            assert_lt!(2, 1);
            mark_end_as_reached();
        }

        /// Test that assert_le fails when greater.
        #[test]
        fn fail_assert_le() {
            assert_le!(2, 1);
            mark_end_as_reached();
        }

        /// Test that assert_gt fails when not greater-than.
        #[test]
        fn fail_assert_gt() {
            assert_gt!(1, 2);
            mark_end_as_reached();
        }

        /// Test that assert_ge fails when less.
        #[test]
        fn fail_assert_ge() {
            assert_ge!(1, 2);
            mark_end_as_reached();
        }

        /// Test that assert_true fails when value is false.
        #[test]
        fn fail_assert_true() {
            assert_true!(false);
            mark_end_as_reached();
        }

        /// Test that assert_false fails when value is true.
        #[test]
        fn fail_assert_false() {
            assert_false!(true);
            mark_end_as_reached();
        }

        /// Test that assert_null fails when pointer is non-null.
        #[test]
        fn fail_assert_null() {
            assert_null!(&42 as *const i32);
            mark_end_as_reached();
        }

        /// Test that assert_nonnull fails when pointer is null.
        #[test]
        fn fail_assert_nonnull() {
            assert_nonnull!(ptr::null::<i32>());
            mark_end_as_reached();
        }

        /// Test that assert_ok fails when value is non-zero.
        #[test]
        fn fail_assert_ok() {
            assert_ok!(Err::<(), _>(zx_status::Status::INTERNAL));
            mark_end_as_reached();
        }

        /// Test that assert_err fails when error does not match.
        #[test]
        fn fail_assert_err() {
            assert_err!(Ok::<(), _>(()), zx_status::Status::INTERNAL);
            mark_end_as_reached();
        }

        /// Test that unwrap_ok fails when value is an error.
        #[test]
        fn test_unwrap_ok() {
            let _: () = unwrap_ok!(Err(zx_status::Status::INTERNAL));
            mark_end_as_reached();
        }

        /// Test that unwrap_some fails when value is None.
        #[test]
        fn test_unwrap_some() {
            let _: () = unwrap_some!(None::<()>);
            mark_end_as_reached();
        }
    }

    /// Expectation tests description.
    #[suite]
    mod expectations {
        /// Success cases.
        #[test]
        fn test_success() {
            expect_eq!(1, 1);
            expect_eq!(1, 1, "one should be one");
            expect_ne!(1, 2);
            expect_ne!(1, 2, "one should not be two");
            expect_lt!(1, 2);
            expect_lt!(1, 2, "one should be less than two");
            expect_le!(1, 1);
            expect_le!(1, 2);
            expect_gt!(2, 1);
            expect_ge!(2, 2);

            expect_true!(true);
            expect_true!(true, "should be true");
            expect_false!(false);
            expect_false!(false, "should be false");

            let null_ptr: *const i32 = ptr::null();
            let nonnull_ptr: *const i32 = &42 as *const i32;
            expect_null!(null_ptr);
            expect_null!(null_ptr, "should be null");
            expect_nonnull!(nonnull_ptr);
            expect_nonnull!(nonnull_ptr, "should be non-null");

            expect_ok!(Ok::<(), zx_status::Status>(()));
            expect_ok!(Ok::<(), zx_status::Status>(()), "should be OK");
            expect_err!(
                Err::<(), _>(zx_status::Status::INVALID_ARGS),
                zx_status::Status::INVALID_ARGS
            );
            expect_err!(
                zx_status::Status::INVALID_ARGS,
                zx_status::Status::INVALID_ARGS,
                "should be INVALID_ARGS"
            );

            mark_end_as_reached();
        }

        /// Test that expect_eq fails on inequality.
        #[test]
        fn fail_expect_eq() {
            expect_eq!(1, 2);
            mark_end_as_reached();
        }

        /// Test that expect_ne fails on equality.
        #[test]
        fn fail_expect_ne() {
            expect_ne!(1, 1);
            mark_end_as_reached();
        }

        /// Test that expect_lt fails when not less-than.
        #[test]
        fn fail_expect_lt() {
            expect_lt!(2, 1);
            mark_end_as_reached();
        }

        /// Test that expect_le fails when greater.
        #[test]
        fn fail_expect_le() {
            expect_le!(2, 1);
            mark_end_as_reached();
        }

        /// Test that expect_gt fails when not greater-than.
        #[test]
        fn fail_expect_gt() {
            expect_gt!(1, 2);
            mark_end_as_reached();
        }

        /// Test that expect_ge fails when less.
        #[test]
        fn fail_expect_ge() {
            expect_ge!(1, 2);
            mark_end_as_reached();
        }

        /// Test that expect_true fails when value is false.
        #[test]
        fn fail_expect_true() {
            expect_true!(false);
            mark_end_as_reached();
        }

        /// Test that expect_false fails when value is true.
        #[test]
        fn fail_expect_false() {
            expect_false!(true);
            mark_end_as_reached();
        }

        /// Test that expect_null fails when pointer is non-null.
        #[test]
        fn fail_expect_null() {
            expect_null!(&42 as *const i32);
            mark_end_as_reached();
        }

        /// Test that expect_nonnull fails when pointer is null.
        #[test]
        fn fail_expect_nonnull() {
            expect_nonnull!(ptr::null::<i32>());
            mark_end_as_reached();
        }

        /// Test that expect_ok fails when value is non-zero.
        #[test]
        fn fail_expect_ok() {
            expect_ok!(zx_status::Status::INTERNAL);
            mark_end_as_reached();
        }

        /// Test that expect_err fails when error does not match.
        #[test]
        fn fail_expect_err() {
            expect_err!(Ok::<(), _>(()), zx_status::Status::INTERNAL);
            mark_end_as_reached();
        }
    }

    /// Subtests description.
    #[suite(name = "subtests")]
    mod subtests {
        /// Test that record_failure inside a subtest does not fail the outer test.
        #[test]
        fn test_shadowing() {
            let failing = subtest!(|should_fail: bool| {
                if should_fail {
                    record_failure!();
                }
            });
            expect_false!(failing(true));
            expect_true!(failing(false));
        }
    }

    #[test]
    fn check_suite_count() {
        let suites = get_suites();
        std::assert_eq!(suites.len(), 6);
    }

    #[test]
    fn check_suite_assertions() {
        let suites = get_suites();
        std::assert!(suites.len() > 0);
        let suite = &suites[0];

        std::assert_eq!(unsafe { CStr::from_ptr(suite.name) }.to_bytes(), b"assertions");
        std::assert_eq!(suite.test_cnt, 15);

        let cases_rodata = unsafe { slice::from_raw_parts(suite.tests, suite.test_cnt) };
        for case in cases_rodata {
            mark_end_as_not_reached();
            let name = unsafe { CStr::from_ptr(case.name) }.to_str().unwrap();
            let res = (case.fn_)();
            if name == "test_success" {
                std::assert_eq!(res, true, "assertions::test_success should pass");
                expect_end_reached();
            } else {
                std::assert_eq!(res, false, "assertions::{} should fail", name);
                expect_end_not_reached();
            }
        }
    }

    #[test]
    fn check_suite_expectations() {
        let suites = get_suites();
        std::assert!(suites.len() > 1);
        let suite = &suites[1];

        std::assert_eq!(unsafe { CStr::from_ptr(suite.name) }.to_bytes(), b"expectations");
        std::assert_eq!(suite.test_cnt, 13);

        let cases = unsafe { slice::from_raw_parts(suite.tests, suite.test_cnt) };
        for case in cases {
            mark_end_as_not_reached();
            let name = unsafe { CStr::from_ptr(case.name) }.to_str().unwrap();
            let res = (case.fn_)();
            if name == "test_success" {
                std::assert_eq!(res, true, "expectations::test_success should pass");
            } else {
                std::assert_eq!(res, false, "expectations::{} should fail", name);
            }
            expect_end_reached();
        }
    }

    #[test]
    fn check_suite_with_one_function() {
        let suites = get_suites();
        std::assert!(suites.len() > 2);
        let suite = &suites[2];

        std::assert_eq!(unsafe { CStr::from_ptr(suite.name) }.to_bytes(), b"one_function");
        std::assert_eq!(
            unsafe { CStr::from_ptr(suite.desc) }.to_str().unwrap(),
            "Suite with one function description."
        );
        std::assert_eq!(suite.test_cnt, 1);
        let case = unsafe { &*suite.tests };
        std::assert_eq!(unsafe { CStr::from_ptr(case.name) }.to_bytes(), b"empty");
        assert!((case.fn_)());
    }

    #[test]
    fn check_suite_subtests() {
        let suites = get_suites();
        std::assert!(suites.len() > 3);
        let suite = &suites[3];

        std::assert_eq!(unsafe { CStr::from_ptr(suite.name) }.to_bytes(), b"subtests");
        std::assert_eq!(
            unsafe { CStr::from_ptr(suite.desc) }.to_str().unwrap(),
            "Subtests description."
        );
        std::assert_eq!(suite.test_cnt, 1);
        let case = unsafe { &*suite.tests };
        std::assert_eq!(unsafe { CStr::from_ptr(case.name) }.to_bytes(), b"test_shadowing");
        assert!((case.fn_)());
    }

    #[test]
    fn check_suite_with_other_items() {
        let suites = get_suites();
        std::assert!(suites.len() > 4);
        let suite = &suites[4];

        std::assert_eq!(
            unsafe { CStr::from_ptr(suite.name) }.to_bytes(),
            b"suite_with_other_items"
        );
        std::assert_eq!(
            unsafe { CStr::from_ptr(suite.desc) }.to_str().unwrap(),
            "Suite with non-test items."
        );
        std::assert_eq!(suite.test_cnt, 1);
        let case = unsafe { &*suite.tests };
        std::assert_eq!(unsafe { CStr::from_ptr(case.name) }.to_bytes(), b"check_other_items");
        assert!((case.fn_)());
    }

    #[test]
    fn check_suite_with_ignored() {
        let suites = get_suites();
        std::assert!(suites.len() > 5);
        let suite = &suites[5];

        std::assert_eq!(unsafe { CStr::from_ptr(suite.name) }.to_bytes(), b"with_ignored");
        std::assert_eq!(suite.test_cnt, 1);
        let case = unsafe { &*suite.tests };
        std::assert_eq!(unsafe { CStr::from_ptr(case.name) }.to_bytes(), b"normal_test");
    }
}
