// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_ATTRIBUTION_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_ATTRIBUTION_H_

#include <cassert>

#include <ffl/fixed.h>

namespace vm {

// Structure to store fractional counts of bytes in fixed point with 63 bits of precision. The max
// fractional value is thus `2-Fraction::Epsilon()`. Counts are strictly unsigned.
//
// This structure supports accumulation with other fractional counts and the code will handle the
// internal bookkeeping around moving whole bytes from the fractional sum to the integral sum.
//
// The structure also supports addition, subtraction, and division with whole integers. If overflow
// occurs in either direction, an assert is fired. Multiplication is not supported.
//
// We always check and guarantee that any fractional sums >=1 have the excess byte stripped off
// and rolled over to the integral value. Thus, since accumulation always operates on fractions
// `<=1-Fraction::Epsilon()`, it always generates results `<= 2-Fraction::Epsilon()` and there is
// never overflow in the fractional fields.
struct FractionalBytes {
  using Fraction = ffl::Fixed<uint64_t, 63>;
  constexpr static Fraction kOneByte = Fraction(1);
  FractionalBytes() = default;

  constexpr explicit FractionalBytes(uint64_t whole_bytes) : integral(whole_bytes) {}
  constexpr explicit FractionalBytes(uint64_t numerator, uint64_t denominator)
      : integral(numerator / denominator),
        fractional((kOneByte / denominator) * (numerator % denominator)) {}

  FractionalBytes operator+(const uint64_t& other) const {
    FractionalBytes ret{*this};
    ret += other;
    return ret;
  }
  FractionalBytes operator-(const uint64_t& other) const {
    FractionalBytes ret{*this};
    ret -= other;
    return ret;
  }
  FractionalBytes operator/(const uint64_t& other) const {
    FractionalBytes ret{*this};
    ret /= other;
    return ret;
  }
  FractionalBytes& operator+=(const uint64_t& other) {
    [[maybe_unused]] bool overflow = __builtin_add_overflow(integral, other, &integral);
    DEBUG_ASSERT(!overflow);
    return *this;
  }
  FractionalBytes& operator-=(const uint64_t& other) {
    [[maybe_unused]] bool overflow = __builtin_sub_overflow(integral, other, &integral);
    DEBUG_ASSERT(!overflow);
    return *this;
  }
  FractionalBytes& operator/=(const uint64_t& other) {
    // Input fraction must always be <1 to guard against overflow.
    // If this is true, the sum of fractions must be <1:
    // The sum is:
    //  `(fractional / other) + (1 / other) * remainder`
    // which we can rewrite as
    //  `(fractional + remainder) / other`
    // We know that fractional < 1 and remainder < other, thus (fractional + remainder) < other and
    // the rewritten sum cannot be >=1.
    DEBUG_ASSERT(fractional < kOneByte);
    const uint64_t remainder = integral % other;
    const Fraction scaled_remainder = (kOneByte / other) * remainder;
    fractional = (fractional / other) + scaled_remainder;
    DEBUG_ASSERT(fractional < kOneByte);
    integral /= other;

    return *this;
  }

  FractionalBytes operator+(const FractionalBytes& other) const {
    FractionalBytes ret{*this};
    ret += other;
    return ret;
  }
  FractionalBytes& operator+=(const FractionalBytes& other) {
    // Input fractions must always be <1 to guard against overflow.
    // If the fractional sum is >=1, then roll that overflow byte into the integral part.
    DEBUG_ASSERT(fractional < kOneByte);
    DEBUG_ASSERT(other.fractional < kOneByte);
    fractional += other.fractional;
    if (fractional >= kOneByte) {
      [[maybe_unused]] bool overflow = __builtin_add_overflow(integral, 1, &integral);
      DEBUG_ASSERT(!overflow);
      fractional -= kOneByte;
    }
    [[maybe_unused]] bool overflow = __builtin_add_overflow(integral, other.integral, &integral);
    DEBUG_ASSERT(!overflow);

    return *this;
  }

  bool operator==(const FractionalBytes& other) const {
    return integral == other.integral && fractional == other.fractional;
  }
  bool operator!=(const FractionalBytes& other) const { return !(*this == other); }

  size_t integral = 0;
  Fraction fractional = Fraction::FromRaw(0);
};

// Structure to store counts of memory attributed to VMOs or portions thereof.
//
// These counts can be accumulated to support attributing memory across composite objects such as
// address spaces or processes.
//
// The `scaled_bytes` fields may contain a fractional number of bytes, and the structure stores the
// fractional counts in fixed point with 63 bits of precision.
struct AttributionCounts {
  size_t total_bytes() const { return uncompressed_bytes + compressed_bytes; }
  size_t total_private_bytes() const {
    return private_uncompressed_bytes + private_compressed_bytes;
  }
  FractionalBytes total_scaled_bytes() const {
    return scaled_uncompressed_bytes + scaled_compressed_bytes;
  }

  AttributionCounts& operator+=(const AttributionCounts& other) {
    uncompressed_bytes += other.uncompressed_bytes;
    compressed_bytes += other.compressed_bytes;
    private_uncompressed_bytes += other.private_uncompressed_bytes;
    private_compressed_bytes += other.private_compressed_bytes;
    scaled_uncompressed_bytes += other.scaled_uncompressed_bytes;
    scaled_compressed_bytes += other.scaled_compressed_bytes;
    return *this;
  }

  bool operator==(const AttributionCounts& other) const {
    return uncompressed_bytes == other.uncompressed_bytes &&
           compressed_bytes == other.compressed_bytes &&
           private_uncompressed_bytes == other.private_uncompressed_bytes &&
           private_compressed_bytes == other.private_compressed_bytes &&
           scaled_uncompressed_bytes == other.scaled_uncompressed_bytes &&
           scaled_compressed_bytes == other.scaled_compressed_bytes;
  }
  bool operator!=(const AttributionCounts& other) const { return !(*this == other); }

  size_t uncompressed_bytes = 0;
  size_t compressed_bytes = 0;
  size_t private_uncompressed_bytes = 0;
  size_t private_compressed_bytes = 0;
  FractionalBytes scaled_uncompressed_bytes;
  FractionalBytes scaled_compressed_bytes;
};

}  // namespace vm

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_ATTRIBUTION_H_
