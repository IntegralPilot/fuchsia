// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_SCUDO_CUSTOM_SCUDO_CONFIG_H_
#define LIB_C_SCUDO_CUSTOM_SCUDO_CONFIG_H_

namespace scudo {

struct DefaultConfig {
  static const bool MaySupportMemoryTagging = false;
  static const bool EnableZeroOnDealloc = true;
  template <class A>
  using TSDRegistryT = TSDRegistrySharedT<A, 8U, 4U>;  // Shared, max 8 TSDs.

  struct Primary {
    using SizeClassMap = FuchsiaSizeClassMap;
#if defined(__riscv)
    // Support 39-bit VMA for riscv-64
    static const uptr RegionSizeLog = 27U;
    static const uptr GroupSizeLog = 19U;
#else
    static const uptr RegionSizeLog = 28U;
    static const uptr GroupSizeLog = 21U;
#endif
    typedef u32 CompactPtrT;
    static const bool EnableRandomOffset = true;
    static const uptr MapSizeIncrement = 1UL << 18;
    static const uptr CompactPtrScale = SCUDO_MIN_ALIGNMENT_LOG;
    static const s32 MinReleaseToOsIntervalMs = INT32_MIN;
    static const s32 MaxReleaseToOsIntervalMs = INT32_MAX;
  };
  template <typename Config>
  using PrimaryT = SizeClassAllocator64<Config>;

  struct Secondary {
    template <typename Config>
    using CacheT = MapAllocatorNoCache<Config>;
  };
  template <typename Config>
  using SecondaryT = MapAllocator<Config>;
};

using Config = DefaultConfig;

}  // namespace scudo

#endif  // LIB_C_SCUDO_CUSTOM_SCUDO_CONFIG_H_
