// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_UTIL_UTILS_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_UTIL_UTILS_H_

#include <lib/magma/util/macros.h>
#include <time.h>
#include <unistd.h>

#include <bit>
#include <cstdint>
#include <limits>

namespace magma {
static inline uint32_t to_uint32(uint64_t val) {
  MAGMA_DASSERT(val <= std::numeric_limits<uint32_t>::max());
  return static_cast<uint32_t>(val);
}

static inline uint32_t page_size() {
#ifdef PAGE_SIZE
  return PAGE_SIZE;
#else
  long page_size = sysconf(_SC_PAGESIZE);
  MAGMA_DASSERT(page_size > 0);
  return to_uint32(page_size);
#endif
}

static inline uint32_t page_shift() { return std::countr_zero(::magma::page_size()); }

static inline bool is_page_aligned(uint64_t val) { return (val & (::magma::page_size() - 1)) == 0; }

static inline uint32_t upper_32_bits(uint64_t n) { return static_cast<uint32_t>(n >> 32); }

static inline uint32_t lower_32_bits(uint64_t n) { return static_cast<uint32_t>(n); }

static inline bool get_pow2(uint64_t val, uint64_t* pow2_out) {
  if (val == 0)
    return MAGMA_DRETF(false, "zero is not a power of two");

  uint64_t result = 0;
  while ((val & 1) == 0) {
    val >>= 1;
    result++;
  }
  if (val >> 1)
    return MAGMA_DRETF(false, "not a power of 2");

  *pow2_out = result;
  return true;
}

static inline bool is_pow2(uint64_t val) {
  uint64_t out;
  return get_pow2(val, &out);
}

// Note, alignment must be a power of 2
template <class T, class U>
static inline T round_up(T val, U alignment) {
  MAGMA_DASSERT(is_pow2(alignment));
  return ((val - 1) | (alignment - 1)) + 1;
}

static inline uint64_t ns_to_ms(uint64_t ns) { return ns / 1000000ull; }

static inline int64_t ms_to_signed_ns(uint64_t ms) {
  if (ms > INT64_MAX / 1000000)
    return INT64_MAX;
  return static_cast<int64_t>(ms) * 1000000;
}

static inline uint64_t get_monotonic_ns() {
  timespec time;
  if (clock_gettime(CLOCK_MONOTONIC, &time) != 0) {
    MAGMA_DASSERT(false);
    return 0;
  }
  return static_cast<uint64_t>(time.tv_sec) * 1000000000ull + time.tv_nsec;
}

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_UTIL_UTILS_H_
