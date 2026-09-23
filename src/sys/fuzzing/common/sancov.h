// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_SANCOV_H_
#define SRC_SYS_FUZZING_COMMON_SANCOV_H_

#include <stdint.h>
#include <zircon/compiler.h>

// The following are symbols that SanitizerCoverage expects the runtime to provide.

extern "C" {

// See https://clang.llvm.org/docs/SanitizerCoverage.html#inline-8bit-counters
// NOLINTNEXTLINE(bugprone-reserved-identifier)
__EXPORT void __sanitizer_cov_8bit_counters_init(uint8_t* start, uint8_t* stop);

// See https://clang.llvm.org/docs/SanitizerCoverage.html#pc-table
// NOLINTNEXTLINE(bugprone-reserved-identifier)
__EXPORT void __sanitizer_cov_pcs_init(const uintptr_t* start, const uintptr_t* stop);

}  // extern "C"

// Can be used on a function or a global variable declaration to specify that a
// particular instrumentation should not be applied.
#define NO_SANITIZE(what) __attribute__((no_sanitize(#what)))
#define NO_ASAN NO_SANITIZE(address)
#define NO_MSAN NO_SANITIZE(memory)
#define NO_SANCOV NO_SANITIZE(coverage)
#define NO_SANITIZE_ALL NO_SANITIZE(all)

#endif  // SRC_SYS_FUZZING_COMMON_SANCOV_H_
