// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/riscv64/feature.h>
#include <stddef.h>
#include <stdint.h>

#include <kernel/ffi.h>
#include <phys/arch/arch-handoff.h>

// Accessors for the fields of `ArchPhysHandoff` that the Rust arch code needs.
//
// The struct itself cannot be described in Rust: it has `std::optional` members,
// and the layout of `arch::RiscvFeatures` is an implementation detail of
// `<lib/arch/riscv64/feature.h>`.  These are all read exactly once at boot.

// The Rust side receives the feature set as a bitmask in which bit N means
// `RiscvFeature` N, so the enumerator values are a cross-language contract even
// though <lib/arch/riscv64/feature.h> assigns them implicitly.  Pin them here so
// that inserting a feature is a compile error rather than a silent misread.
static_assert(static_cast<size_t>(arch::RiscvFeature::kSstc) == 0);
static_assert(static_cast<size_t>(arch::RiscvFeature::kSvpbmt) == 1);
static_assert(static_cast<size_t>(arch::RiscvFeature::kVector) == 2);
static_assert(static_cast<size_t>(arch::RiscvFeature::kZicbom) == 3);
static_assert(static_cast<size_t>(arch::RiscvFeature::kZicboz) == 4);
static_assert(static_cast<size_t>(arch::RiscvFeature::kZicntr) == 5);
static_assert(static_cast<size_t>(arch::RiscvFeature::kMax) == 6);

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language
// inlining works.

FFI_ALWAYS_INLINE uint64_t cpp_riscv64_handoff_boot_hart_id(const ArchPhysHandoff* handoff) {
  return handoff->boot_hart_id;
}

// Deliberately not FFI_ALWAYS_INLINE, unlike its neighbours: it loops over the
// feature set and runs once at boot, so inlining it across the language boundary
// would buy nothing.
uint64_t cpp_riscv64_handoff_cpu_feature_bits(const ArchPhysHandoff* handoff) {
  uint64_t bits = 0;
  for (size_t i = 0; i < static_cast<size_t>(arch::RiscvFeature::kMax); ++i) {
    if (handoff->cpu_features[static_cast<arch::RiscvFeature>(i)]) {
      bits |= uint64_t{1} << i;
    }
  }
  return bits;
}

FFI_ALWAYS_INLINE uint16_t cpp_riscv64_handoff_cbom_size(const ArchPhysHandoff* handoff) {
  return handoff->cpu_features.cbom_size();
}

FFI_ALWAYS_INLINE uint16_t cpp_riscv64_handoff_cboz_size(const ArchPhysHandoff* handoff) {
  return handoff->cpu_features.cboz_size();
}

}  // extern "C"
