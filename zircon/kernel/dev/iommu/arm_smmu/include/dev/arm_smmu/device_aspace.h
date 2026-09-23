// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_DEVICE_ASPACE_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_DEVICE_ASPACE_H_

#include <lib/zx/result.h>
#include <stdint.h>

#include <dev/arm_smmu/page_cache.h>
#include <dev/arm_smmu/translation_table_helper.h>
#include <ktl/memory.h>
#include <ktl/optional.h>
#include <ktl/unique_ptr.h>
#include <ktl/utility.h>
#include <region-alloc/region-alloc.h>
#include <vm/pinned_vm_object.h>

struct vm_page;
using vm_page_t = vm_page;

namespace arm_smmu {
class DeviceAspaceTest;

class DeviceAspace {
 public:
  // The definition of a callback which can be used to invalidate TLBs at the proper
  // point in Map/Unmap operations.
  struct TlbInvalOp {
    // Callback takes the context pointer the user constructed this op with, as
    // well as the base address and size of the region in the device's address
    // space to invalidate.  0 will be passed for size in the case that the
    // entire device address space should be invalidated.
    using Callback = void (*)(void* ctx, uint64_t base, uint64_t size);

    TlbInvalOp(Callback cbk, void* ctx) : cbk(cbk), ctx(ctx) {}

    TlbInvalOp(const TlbInvalOp&) = delete;
    TlbInvalOp(TlbInvalOp&&) = delete;
    TlbInvalOp& operator=(const TlbInvalOp&) = delete;
    TlbInvalOp& operator=(TlbInvalOp&&) = delete;

    void Invalidate(uint64_t base, uint64_t size) { cbk(ctx, base, size); }
    void InvalidateAll() { Invalidate(0, 0); }

    bool is_valid() const { return (cbk != nullptr) && (ctx != nullptr); }

    const Callback cbk;
    void* const ctx;
  };

  // We currently assume and only support 4k pages.
  static constexpr uint32_t kPageSize = 4096;
  static constexpr uint32_t kPageMask = kPageSize - 1;
  static constexpr uint32_t kPageShift = 12;

  // An allocation in a device's aspace is a unique_ptr to a region from our
  // region allocator.
  using Allocation = RegionAllocator::Region::UPtr;

  // Note, the inclusive length of the address space should always fit within a
  // 64 bit unsigned integer since the maximum coverage of a set of VMSAv8-64
  // page tables is 48 bits.
  static zx::result<ktl::unique_ptr<DeviceAspace>> Create(
      uint64_t aspace_start, uint64_t aspace_len,
      uint32_t max_cache_pages = kDefaultMaxPageCacheEntries);

  DeviceAspace(const DeviceAspace&) = delete;
  DeviceAspace(DeviceAspace&&) = delete;
  DeviceAspace& operator=(const DeviceAspace&) = delete;
  DeviceAspace& operator=(DeviceAspace&&) = delete;

  paddr_t GetRootPaddr() const;
  zx::result<Allocation> Map(const PinnedVmObject& pinned_vmo, uint32_t perms,
                             TlbInvalOp& tlb_inval_op,
                             ktl::optional<uint64_t> location = ktl::nullopt);
  void Unmap(Allocation alloc, TlbInvalOp& tlb_inval_op);

  // Recursively walk all translation table pages and return them to the PMM.
  //
  // This must be done exactly once in the life of a DeviceAspace object, just
  // before shutdown.
  void FreeTranslationTables(TlbInvalOp& tlb_inval_op);

  const PageCache& page_cache() const { return page_cache_; }
  uint32_t granule_size_bits() const { return kPageShift; }
  uint64_t first_valid_address() const { return aspace_start_; }
  uint64_t last_valid_address() const {
    ZX_DEBUG_ASSERT(aspace_len_ > 0);
    return aspace_start_ + aspace_len_ - 1;
  }

 private:
  friend class ktl::default_delete<DeviceAspace>;  // unique_ptrs can delete us.

  // Our TT helper can access our internals, like our page cache and our allocated regions.
  friend class TranslationTableHelper;
  friend class DeviceAspaceTest;

  // Try to keep around 4 pages in our page cache at all times.  This way, if
  // someone is frequently pinning and unpinning a reasonably sized region (less
  // than 2^18 pages), we'll always find the pages we need in the cache (once
  // the cache is hot), even if the mapping ends up straddling a level 3
  // boundary.  Note, this is assuming that our allocation does not also
  // straddle a level 2 or level 1 boundary.
  //
  // If that is ever a problem, one option would be to to increase the cache to
  // 6 pages total to avoid the worst case thrash.
  static constexpr uint32_t kDefaultMaxPageCacheEntries = 4;

  DeviceAspace(uint64_t aspace_start, uint64_t aspace_len, uint32_t max_cache_pages)
      : aspace_start_(aspace_start),
        aspace_len_(aspace_len),
        max_cache_pages_(max_cache_pages),
        tt_helper_(*this) {}
  ~DeviceAspace();

  // The internal helper which performs the actual recursive walk.  Note that
  // this is true recursion (actual stack levels and everything), but is
  // considered to be OK in the kernel as the max recursion depth is only 4.
  void FreeTranslationTablesHelper(vm_page_t* table, uint32_t level);

  const uint64_t aspace_start_;
  const uint64_t aspace_len_;
  const uint32_t max_cache_pages_;

  // TODO(johngro): This is currently doing all of its bookkeeping using direct
  // heap allocations. Consider switching it to use a RegionPool slab allocator
  // instead.
  RegionAllocator avail_regions_{};
  PageCache page_cache_{};

  // An instance of a helper class which holds state for us when we are
  // performing operations like mapping/unmapping.
  TranslationTableHelper tt_helper_;

  // We currently only use the VMSAv8-64 translation table format with 4k
  // granules.  This means that we have 4 levels of translation table, each with
  // 512 64-bit table entries.
  //
  // As long as we are operating, we always hold onto (at least) the root page,
  // which we model as a pointer to uint64_t's, each of which represent an entry
  // in the translation table.
  vm_page_t* root_tt_page_{nullptr};
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_DEVICE_ASPACE_H_
