// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_INTERNAL_ABI_H_
#define ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_INTERNAL_ABI_H_

// This file describes how the kernel exposes time values to userland.
// This is a PRIVATE UNSTABLE ABI that may change at any time!
// Note that this header is used in both the kernel and in userland in the libfasttime library.
// Therefore, it must be compatible with both the kernel and user header environments.

#include <zircon/time.h>
#include <zircon/types.h>

#include <atomic>
#include <type_traits>

namespace fasttime::internal {

// Many members of this struct are marked const to force folks who initialize the structure to
// explicitly declare values at the time of instantiation. The primary use case for this is the
// test code. Note that all accesses of this structure should still be done with a const reference.
struct TimeValues {
  // A version number to check against the version of libfasttime.
  const uint64_t version;

  // Conversion factor for zx_ticks_get return values to seconds.
  const zx_ticks_t ticks_per_second;

  // Offset for converting from the raw system timer to boot ticks.
  const zx_ticks_t boot_ticks_offset;

  // Offset for converting from the raw system timer to monotonic ticks.
  std::atomic<zx_ticks_t> mono_ticks_offset{0};

  // Ratio which relates ticks (zx_ticks_get) to clock monotonic (zx_clock_get_monotonic).
  // Specifically...
  //
  // ClockMono(ticks) = (ticks * N) / D
  //
  const uint32_t ticks_to_time_numerator;
  const uint32_t ticks_to_time_denominator;

  // True if usermode can access ticks.
  const uint8_t usermode_can_access_ticks;

  // Whether the A73 errata mitigation must be used when getting ticks.
  // Should always be false on x86 and RISC-V.
  const uint8_t use_a73_errata_mitigation;

  // Whether (on ARM64) the physical counter should be sampled instead of the
  // virtual counter.
  //
  // Should always be false on x86 and RISC-V.
  const uint8_t use_pct_instead_of_vct;

  // Explicitly pad this structure out to an 8 byte boundary and fill this
  // padding with 0 by default.
  //
  // During VDSO initialization, this structure will be instantiated on the
  // stack and populated with proper values, and then copied into the shared
  // page which will be used to publish the values to user-mode.  It is
  // important that all of the bytes of the object have been explicitly
  // initialized in order to avoid any chance of accidentally leaking
  // information about the kernel stack at the time of construction.
  const uint8_t _padding[5]{0};
};

// TimeValues can't have fancy stuff like vtables or multiple inheritance (it
// must have a standard layout), and all of its bytes must be explicitly
// initialized at the time of construction (it must have a "unique object
// representation").
static_assert(std::is_standard_layout_v<TimeValues>);

// TODO(https://fxbug.dev/515456595)
//
// The structure above *does* have only unique representations, but Clang 23.0.0
// seems to think that std::atomic<int64_t> fails this test, causing the
// structure to fail as well.  Clang 22.0.1 disagrees, as does GCC.  We'd like
// to statically assert this, but until we can sort out the toolchain issues, we
// need to leave this commented out.
// static_assert(std::has_unique_object_representations_v<TimeValues>);

}  // namespace fasttime::internal

// PA_VMO_KERNEL_FILE with this name holds the global instance of the TimeValuesVmo.
static constexpr const char kTimeValuesVmoName[] = "time_values";

#endif  // ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_INTERNAL_ABI_H_
