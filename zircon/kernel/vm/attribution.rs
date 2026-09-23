// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use attribution_bindings as bindings;
use core::mem::MaybeUninit;

pub use bindings::vm_AttributionCounts as AttributionCounts;

/// Structure to store fractional counts of bytes in fixed point with 63 bits of precision. The max
/// fractional value is thus `2-Fraction::Epsilon()`. Counts are strictly unsigned.
///
/// This structure supports accumulation with other fractional counts and the code will handle the
/// internal bookkeeping around moving whole bytes from the fractional sum to the integral sum.
///
/// The structure also supports addition, subtraction, and division with whole integers. If
/// overflow occurs in either direction, an assert is fired. Multiplication is not supported.
///
/// We always check and guarantee that any fractional sums >=1 have the excess byte stripped off
/// and rolled over to the integral value. Thus, since accumulation always operates on fractions
/// `<=1-Fraction::Epsilon()`, it always generates results `<= 2-Fraction::Epsilon()` and there is
/// never overflow in the fractional fields.
pub use bindings::vm_FractionalBytes as FractionalBytes;

/// Returns an `AttributionCounts` initialized to zero.
pub fn zero() -> AttributionCounts {
    let mut counts = MaybeUninit::uninit();
    // SAFETY: `counts.as_mut_ptr()` is valid for writing `AttributionCounts`.
    unsafe {
        bindings::cpp_attribution_counts_zero(counts.as_mut_ptr());
    }
    // SAFETY: `cpp_attribution_counts_zero` certainly populated `counts`.
    unsafe { counts.assume_init() }
}

/// Returns the sum of uncompressed and compressed bytes.
pub fn total_bytes(counts: &AttributionCounts) -> u64 {
    (counts.uncompressed_bytes + counts.compressed_bytes) as u64
}

/// Returns the sum of private uncompressed and private compressed bytes.
pub fn total_private_bytes(counts: &AttributionCounts) -> u64 {
    (counts.private_uncompressed_bytes + counts.private_compressed_bytes) as u64
}

/// Returns the sum of scaled uncompressed and scaled compressed bytes.
pub fn total_scaled_bytes(counts: &AttributionCounts) -> FractionalBytes {
    fractional_bytes_add(counts.scaled_uncompressed_bytes, counts.scaled_compressed_bytes)
}

/// Creates a `FractionalBytes` representing the given number of whole bytes.
pub fn fractional_bytes_from_whole(whole_bytes: u64) -> FractionalBytes {
    let mut out = MaybeUninit::uninit();
    // SAFETY: `out.as_mut_ptr()` is valid for writing `FractionalBytes`.
    unsafe {
        bindings::cpp_fractional_bytes_from_whole(whole_bytes, out.as_mut_ptr());
    }
    // SAFETY: `cpp_fractional_bytes_from_whole` certainly populated `counts`.
    unsafe { out.assume_init() }
}

/// Creates a `FractionalBytes` representing a fractional value from `numerator` and `denominator`.
pub fn fractional_bytes_from_fraction(numerator: u64, denominator: u64) -> FractionalBytes {
    let mut out = MaybeUninit::uninit();
    // SAFETY: `out.as_mut_ptr()` is valid for writing `FractionalBytes`.
    unsafe {
        bindings::cpp_fractional_bytes_from_fraction(numerator, denominator, out.as_mut_ptr());
    }
    // SAFETY: `cpp_fractional_bytes_from_fraction` certainly populated `out`.
    unsafe { out.assume_init() }
}

/// Adds two `FractionalBytes` values.
pub fn fractional_bytes_add(a: FractionalBytes, b: FractionalBytes) -> FractionalBytes {
    let mut out = MaybeUninit::uninit();
    // SAFETY: `&a` and `&b` are valid `FractionalBytes`, and `out.as_mut_ptr()` is valid for
    // writing.
    unsafe {
        bindings::cpp_fractional_bytes_add(&a, &b, out.as_mut_ptr());
    }
    // SAFETY: `cpp_fractional_bytes_add` certainly populated `out`.
    unsafe { out.assume_init() }
}
