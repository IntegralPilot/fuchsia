// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_TRANSLATION_TABLE_HELPER_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_TRANSLATION_TABLE_HELPER_H_

#include <lib/zx/result.h>
#include <stdint.h>

class RegionAllocator;
struct vm_page;
using vm_page_t = vm_page;

namespace arm_smmu {

class DeviceAspace;

class TranslationTableHelper {
 public:
  zx::result<> InitializeForMap(uint64_t address) { return Initialize(Op::Map, address); }
  zx::result<> InitializeForUnmap(uint64_t address) { return Initialize(Op::Unmap, address); }

  zx::result<> Advance();
  void AssignPageEntry(uint64_t page_entry);
  void FinishOperation();
  bool CurrentPageEntryValid();

 private:
  // The TranslationTableHelper is pretty much an inner class helper for
  // DeviceAspace.  It is the only thing allowed to instantiate us.
  friend class DeviceAspace;

  static constexpr uint32_t kLevels = 4;
  static constexpr uint32_t kAddrBitsPerLevel = 9;
  static constexpr uint64_t kAddrBitsMask = (uint64_t{1} << kAddrBitsPerLevel) - 1;
  static constexpr uint32_t kPageShift = 12;

  enum class Op { Invalid, Map, Unmap };

  struct Level {
    uint64_t* table{nullptr};
    uint32_t ndx{0};
    bool dirty{false};
  };

  explicit TranslationTableHelper(DeviceAspace& aspace) : aspace_(aspace) {}
  ~TranslationTableHelper() = default;

  zx::result<> Initialize(Op op, uint64_t address);
  void FinishLevel(uint32_t level_ndx);
  zx::result<> FindPageForLevel(uint32_t level);

  Op op_{Op::Invalid};
  DeviceAspace& aspace_;
  Level levels_[kLevels];
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_TRANSLATION_TABLE_HELPER_H_
