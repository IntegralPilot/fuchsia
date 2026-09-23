// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "third_party/github.com/ridiculousfish/libdivide/include/libdivide.h"

// Return value of the __aeabi_uldivmod function
typedef struct {
  uint64_t quotient;
  uint64_t remainder;
} ULdivmodResult;

// Divides a 64-bit number `n` by a constant `d`, and returns the quotient
// and remainder.
//
// This function uses libdivide to ensure an efficient operation.
template <uint64_t d>
static inline ULdivmodResult div_rem(uint64_t n) {
  constexpr libdivide::branchfree_divider<uint64_t> divider(d);
  uint64_t q = n / divider;
  return {
      .quotient = q,
      .remainder = n - q * d,
  };
}

// Convert the ULdivmodResult into a __uint128_t
// This is required because the return value needs to be returned into the r0,
// r1, r2, r3 registers and structs are not.
// This function is actually a no-op and only used to force the compiler to use
// the correct calling convention.
static inline __uint128_t C(ULdivmodResult v) {
  return static_cast<__uint128_t>(v.quotient) | (static_cast<__uint128_t>(v.remainder) << 64);
}

// Specific constant to improve the division
// Used for the conversion between nanoseconds and seconds.
constexpr uint64_t k1e9 = 1'000'000'000;
// Used to convert cpu ticks to nanoseconds.
constexpr uint64_t k_19200000 = 19'200'000;

extern "C" {

__uint128_t __aeabi_uldivmod(uint64_t dividend, uint64_t divisor) {
  switch (divisor) {
    case 1:
      return C({
          .quotient = dividend,
          .remainder = 0,
      });
    case k1e9:
      return C(div_rem<k1e9>(dividend));
    case k_19200000:
      return C(div_rem<k_19200000>(dividend));
  }

  // Shortcut special cases
  if (divisor == 0) {
    // div-by-0.
    return C({
        .quotient = UINT64_MAX,
        .remainder = 0,
    });
  }
  if (divisor > dividend) {
    return C({
        .quotient = 0,
        .remainder = dividend,
    });
  }
  if (divisor == dividend) {
    return C({
        .quotient = 1,
        .remainder = 0,
    });
  }

  // If not, we want to move the divisor as far left as we can,
  // and then compare against an accumulator of the dividend's left
  // bits. If the accumulator is larger, then we subtract it out, set
  // the quotient bit and keep going.  The quotient bit can be set
  // and shifted because it can't be larger than the divisor was.

  uint32_t count = 1;
  // Find the first bit in the divisor and shift it left,
  // so we can test at each step.
  while ((divisor >> 63) == 0) {
    count++;
    divisor <<= 1;
  }
  ULdivmodResult result = {
      .quotient = 0,
      .remainder = dividend,
  };
  while (count) {
    result.quotient <<= 1;  // shift here so our last bit is available.
    if (result.remainder >= divisor) {
      result.quotient |= 1;
      result.remainder -= divisor;
    }
    count -= 1;
    divisor >>= 1;
  }
  return C(result);
}
}
