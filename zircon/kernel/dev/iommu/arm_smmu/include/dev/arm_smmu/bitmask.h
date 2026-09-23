// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_BITMASK_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_BITMASK_H_

#include <assert.h>
#include <lib/boot-options/arm64.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>

#include <dev/arm_smmu/smmu_registers.h>
#include <ktl/array.h>
#include <ktl/limits.h>

namespace arm_smmu {

// TODO(johngro): Look into whether or not we can use std::bitset in the kernel,
// and if we could efficiently use it here instead of rolling our own bitmask.
//
// Right now, it might be a bit tough.  Simple operations like setting and
// clearing bits are built into bitset, but it does not support efficient
// implementations of operations like "find the first bit set" as there is no
// bitset specialization for stuff like `std::count[lr]_(zero|one)`, and the
// opaque nature of the implementation of bitset makes it more or less
// impossible for us to extend the class to support such a thing.
template <uint32_t Bits, typename StorageType = uint64_t>
class Bitmask {
 public:
  constexpr Bitmask() = default;

  void SetBit(uint32_t ndx) {
    const uint32_t sndx = ndx / kBitsPerStorage;
    const uint32_t bndx = ndx % kBitsPerStorage;
    const StorageType mask = StorageType{1} << bndx;
    bits_[sndx] |= mask;
  }

  void ClrBit(uint32_t ndx) {
    const uint32_t sndx = ndx / kBitsPerStorage;
    const uint32_t bndx = ndx % kBitsPerStorage;
    const StorageType mask = StorageType{1} << bndx;
    bits_[sndx] &= ~mask;
  }

  void AssignBit(uint32_t ndx, bool do_set) {
    if (do_set) {
      SetBit(ndx);
    } else {
      ClrBit(ndx);
    }
  }

  bool TestBit(uint32_t ndx) const {
    ZX_DEBUG_ASSERT(ndx <= Bits);
    const uint32_t sndx = ndx / kBitsPerStorage;
    const uint32_t bndx = ndx % kBitsPerStorage;
    const StorageType mask = StorageType{1} << bndx;
    return ((bits_[sndx] & mask) != 0);
  }

  void SetLowestNBits(uint32_t N) {
    ZX_DEBUG_ASSERT(N <= Bits);

    uint32_t sndx = 0;
    while (N >= kBitsPerStorage) {
      bits_[sndx++] = ktl::numeric_limits<StorageType>::max();
      N -= kBitsPerStorage;
    }

    bits_[sndx] |= ((StorageType{1} << N) - 1);
  }

  // Find the first (least significant) bit with a value of 1 in the bitmask and
  // return its index. Return nullopt if no such bit exists.
  ktl::optional<uint32_t> FindFirstSetBit() {
    for (uint32_t i = 0; i < bits_.size(); ++i) {
      if (const uint32_t ndx = ktl::countr_zero(bits_[i]); ndx < kBitsPerStorage) {
        return (i * kBitsPerStorage) + ndx;
      }
    }
    return ktl::nullopt;
  }

  // Find the first (least significant) bit with a value of 0 in the bitmask and
  // return its index. Return nullopt if no such bit exists.
  ktl::optional<uint32_t> FindFirstClrBit() {
    for (uint32_t i = 0; i < bits_.size(); ++i) {
      if (const uint32_t ndx = ktl::countr_one(bits_[i]); ndx < kBitsPerStorage) {
        return (i * kBitsPerStorage) + ndx;
      }
    }
    return ktl::nullopt;
  }

 private:
  static inline constexpr uint32_t kBitsPerStorage = sizeof(StorageType) << 3;
  static inline constexpr uint32_t kStorageCount = (Bits + kBitsPerStorage - 1) / kBitsPerStorage;

  ktl::array<StorageType, kStorageCount> bits_{0};
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_BITMASK_H_
