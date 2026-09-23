// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "include/vm/attribution_ffi.h"

#include <kernel/ffi.h>
#include <ktl/memory.h>

extern "C" {

FFI_ALWAYS_INLINE void cpp_attribution_counts_zero(vm::AttributionCounts* out_counts) {
  ktl::construct_at(out_counts);
}

FFI_ALWAYS_INLINE void cpp_fractional_bytes_from_whole(uint64_t whole_bytes,
                                                       vm::FractionalBytes* out) {
  ktl::construct_at(out, whole_bytes);
}

FFI_ALWAYS_INLINE void cpp_fractional_bytes_from_fraction(uint64_t numerator, uint64_t denominator,
                                                          vm::FractionalBytes* out) {
  ktl::construct_at(out, numerator, denominator);
}

FFI_ALWAYS_INLINE void cpp_fractional_bytes_add(const vm::FractionalBytes* a,
                                                const vm::FractionalBytes* b,
                                                vm::FractionalBytes* out) {
  vm::FractionalBytes sum = *a + *b;
  ktl::construct_at(out, sum);
}

}  // extern "C"
