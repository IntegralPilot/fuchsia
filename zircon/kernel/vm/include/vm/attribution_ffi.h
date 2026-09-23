// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_ATTRIBUTION_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_ATTRIBUTION_FFI_H_

#include <stdint.h>
#include <zircon/compiler.h>

#include <vm/attribution.h>

__BEGIN_CDECLS

void cpp_attribution_counts_zero(vm::AttributionCounts* out_counts);

void cpp_fractional_bytes_from_whole(uint64_t whole_bytes, vm::FractionalBytes* out);
void cpp_fractional_bytes_from_fraction(uint64_t numerator, uint64_t denominator,
                                        vm::FractionalBytes* out);
void cpp_fractional_bytes_add(const vm::FractionalBytes* a, const vm::FractionalBytes* b,
                              vm::FractionalBytes* out);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_ATTRIBUTION_FFI_H_
