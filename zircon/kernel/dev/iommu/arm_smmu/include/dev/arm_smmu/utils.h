// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_UTILS_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_UTILS_H_

#include <assert.h>
#include <lib/boot-options/arm64.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>

#include <dev/arm_smmu/constants.h>
#include <dev/arm_smmu/smmu_registers.h>
#include <ktl/array.h>
#include <ktl/bit.h>
#include <ktl/limits.h>

namespace arm_smmu {

const char* ArmSmmuModeToString(ArmSmmuMode mode);
const char* ArmCbarTypeToString(CBAR_Type type);
const char* ArmS2crTypeToString(S2CR_Type type);
const char* AddrModeToString(AddrMode mode);
const char* BtiModeToString(BtiMode mode);

class SmrValue {
 public:
  class iterator {
   public:
    uint16_t operator*() const {
      ZX_DEBUG_ASSERT(state_ndx_ < state_count());

      uint16_t ret = value_ & ~mask_;
      uint32_t mask_ndx = ktl::countr_zero(mask_);

      for (uint32_t i = 0; i < log2_state_cnt_; ++i) {
        if (state_ndx_ & (1u << i)) {
          ret |= 1u << mask_ndx;
        }

        mask_ndx += 1;
        mask_ndx += ktl::countr_zero(uint32_t{mask_} >> (mask_ndx));
      }

      return ret;
    }

    iterator& operator++() {
      if (state_ndx_ < state_count()) {
        ++state_ndx_;
      }
      return *this;
    }

    iterator operator++(int) {
      iterator ret{*this};
      ++(*this);
      return ret;
    }

    bool operator==(const iterator& other) const {
      return (other.mask_ == mask_) && (other.value_ == value_) && (other.state_ndx_ == state_ndx_);
    }

    bool operator!=(const iterator& other) const { return !(*this == other); }

   private:
    friend class SmrValue;

    iterator(uint32_t reg_value, bool is_end)
        : mask_{uint16_t(reg_value >> 16)},
          value_{uint16_t(reg_value & 0xFFFF)},
          log2_state_cnt_{static_cast<uint32_t>(ktl::popcount(mask_))},
          state_ndx_{is_end ? state_count() : 0} {}

    uint32_t state_count() const { return 1u << log2_state_cnt_; }

    const uint16_t mask_;
    const uint16_t value_;
    const uint32_t log2_state_cnt_;
    uint32_t state_ndx_{0};
  };

  uint32_t value() const { return value_; }
  uint32_t id() const { return value_ & 0xFFFF; }
  uint32_t mask() const { return value_ >> 16; }

  bool Intersects(const SmrValue& other) {
    const uint32_t fixed_bits = (~value_ >> 16) & (~other.value_ >> 16);
    return (value_ & fixed_bits) == (other.value_ & fixed_bits);
  }

  iterator begin() const { return iterator{value_, false}; }
  iterator end() const { return iterator{value_, true}; }

 private:
  friend class Smmu;

  // Note, the only place we would ever expect to see a |reg_value| with bits
  // set outside of the valid mask would be when using the kernel console to
  // execute a `show` command with a target specified by SID in order to create
  // a SmrValue instance to use when searching for a BTI target.  Aside from
  // that, the reg_value given here should always exist within the valid mask as
  // determined by HW configuration.
  SmrValue(uint32_t reg_val, uint32_t valid_mask) : value_(reg_val & valid_mask) {}

  const uint32_t value_;
};

// Tests to see if two SMRs have any potential to match the same specific
// stream ID based on the encoded id/mask pair.
inline bool SmrIntersects(uint32_t smr1, uint32_t smr2) {
  // Compute |fixed_bits|, which is the set of bits that both SMRs care about.
  // (as opposed to the `don't-care` bits defined by the SMR mask).  All of the
  // bits set to 1 in 'fixed_bits' should be the bits which are zero in *both*
  // of the SMR's "mask" fields.
  const uint32_t fixed_bits = (~smr1 >> 16) & (~smr2 >> 16);

  // If the "value" fields of each SMR masked by the `fixed_bits` we just
  // computed match, then there must exist at least one Stream ID which is
  // considered to be valid by both of the SMRs, and the two SMRs are said to
  // "intersect".
  return (smr1 & fixed_bits) == (smr2 & fixed_bits);
}

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_UTILS_H_
