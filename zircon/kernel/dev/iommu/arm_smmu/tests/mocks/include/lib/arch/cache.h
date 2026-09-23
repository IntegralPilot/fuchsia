// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_LIB_ARCH_CACHE_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_LIB_ARCH_CACHE_H_

#include <stddef.h>
#include <stdint.h>

namespace arch {

inline void CleanDataCacheRange(uint64_t addr, size_t size) {}

}  // namespace arch

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_LIB_ARCH_CACHE_H_
