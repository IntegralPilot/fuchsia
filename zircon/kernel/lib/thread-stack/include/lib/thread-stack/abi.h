// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_THREAD_STACK_INCLUDE_LIB_THREAD_STACK_ABI_H_
#define ZIRCON_KERNEL_LIB_THREAD_STACK_INCLUDE_LIB_THREAD_STACK_ABI_H_

#include <lib/page/size.h>
#include <stdint.h>

#include <phys/zircon-abi-spec.h>

namespace internal {

// A common, convenience value for machine and unsafe stack sizes. Not intended
// to be a part of the public API.
constexpr uint32_t kDefaultStackSize =
#ifdef CUSTOM_DEFAULT_STACK_SIZE
    CUSTOM_DEFAULT_STACK_SIZE
#else
    2 * kPageSize
#endif
    ;

// The would-be unsafe stack size, if enabled.
constexpr uint32_t kUnsafeStackSize = kDefaultStackSize;

constexpr uint32_t kMachineStackSize =
    // If there is no unsafe stack on x86, extend the machine stack by the
    // would-be unsafe stack size for consistency.
    kDefaultStackSize +
#if defined(__x86_64__) && !__has_feature(safe_stack)
    kUnsafeStackSize;
#else
    0;
#endif

// The would-be unsafe shadow call stack size, if enabled.
constexpr uint32_t kShadowCallStackSize = kPageSize;

// The size of the unmapped 'guard' region to ensure lies above or below the
// mapped stack.
constexpr uint32_t kStackGuardRegionSize = kPageSize;

}  // namespace internal

constexpr ZirconAbiSpec::Stack kMachineStack = {
    .size_bytes = internal::kMachineStackSize,
    .lower_guard_size_bytes = internal::kStackGuardRegionSize,
    .upper_guard_size_bytes = internal::kStackGuardRegionSize,
};

#if __has_feature(safe_stack)
constexpr ZirconAbiSpec::Stack kUnsafeStack = {
    .size_bytes = internal::kUnsafeStackSize,
    .lower_guard_size_bytes = internal::kStackGuardRegionSize,
    .upper_guard_size_bytes = internal::kStackGuardRegionSize,
};
#endif

#if __has_feature(shadow_call_stack)
constexpr ZirconAbiSpec::Stack kShadowCallStack = {
    .size_bytes = internal::kShadowCallStackSize,
    .lower_guard_size_bytes = internal::kStackGuardRegionSize,
    .upper_guard_size_bytes = internal::kStackGuardRegionSize,
};
#endif

#endif  // ZIRCON_KERNEL_LIB_THREAD_STACK_INCLUDE_LIB_THREAD_STACK_ABI_H_
