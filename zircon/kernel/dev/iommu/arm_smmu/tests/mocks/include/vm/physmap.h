// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PHYSMAP_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PHYSMAP_H_

#include <stdint.h>
#include <sys/types.h>
#include <zircon/assert.h>
#include <zircon/types.h>

inline void* paddr_to_physmap(paddr_t pa) { return reinterpret_cast<void*>(pa); }
inline paddr_t physmap_to_paddr(const void* ptr) { return reinterpret_cast<paddr_t>(ptr); }

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_TESTS_MOCKS_INCLUDE_VM_PHYSMAP_H_
