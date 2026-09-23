// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_COMMON_H_
#define ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_COMMON_H_

#include <stdint.h>

#include <ktl/limits.h>

#define IOMMU_FLAG_PERM_READ (1 << 0)
#define IOMMU_FLAG_PERM_WRITE (1 << 1)
#define IOMMU_FLAG_PERM_EXECUTE (1 << 2)

// Type used to refer to virtual addresses presented in virtual address space
// presented to a device by the IOMMMU.
using dev_vaddr_t = uint64_t;

namespace iommu {

// The sentinel value used to indicate that a physical address is invalid.
// IOMMUs, just like CPU MMUs, work in memory units of pages which are at least
// 4KB in length and 4KB aligned.  Page sizes can go up from there (16KB, 64KB,
// etc) but are always "page aligned" implying that a valid page address always
// has the lowest N bits cleared.  This address is all 1s, meaning that it will
// never be a valid physical page address for any system unless the page size
// was a single byte (which just does not happen).
constexpr uint64_t INVALID_PADDR = ktl::numeric_limits<uint64_t>::max();

// A strongly typed enum-style-bool which determines whether a request for a PMT
// mapping needs to be contiguous in dev_vaddr_t.
enum class RequireContiguousMapping { No, Yes };

struct QueryAddressResult {
  const dev_vaddr_t device_vaddr;
  const size_t size;
};

}  // namespace iommu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_INCLUDE_DEV_IOMMU_COMMON_H_
