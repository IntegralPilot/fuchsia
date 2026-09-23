// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_STREAM_MATCH_REG_GROUP_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_STREAM_MATCH_REG_GROUP_H_

#include <lib/zx/result.h>
#include <stdint.h>

#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_bti.h>
#include <dev/arm_smmu/utils.h>
#include <fbl/intrusive_double_list.h>
#include <ktl/unique_ptr.h>

namespace arm_smmu {

class ContextBank;
class SmmuBti;

class StreamMatchRegGroup : public fbl::DoublyLinkedListable<ktl::unique_ptr<StreamMatchRegGroup>> {
 public:
  static zx::result<ktl::unique_ptr<StreamMatchRegGroup>> CreateAndLockdown(Smmu& smmu,
                                                                            uint32_t smrg_ndx,
                                                                            SmrValue stream_ids)
      TA_REQ(smmu.get_lock());

  static zx::result<ktl::unique_ptr<StreamMatchRegGroup>> CreateAndAdopt(Smmu& smmu,
                                                                         uint32_t smrg_ndx,
                                                                         SmrValue stream_ids)
      TA_REQ(smmu.get_lock());

  StreamMatchRegGroup(const StreamMatchRegGroup&) = delete;
  StreamMatchRegGroup operator=(const StreamMatchRegGroup&) = delete;
  StreamMatchRegGroup(StreamMatchRegGroup&&) = delete;
  StreamMatchRegGroup operator=(StreamMatchRegGroup&&) = delete;

  static void Disable(Smmu& smmu, uint32_t smrg_ndx) TA_REQ(smmu.get_lock()) {
    ZX_DEBUG_ASSERT(smrg_ndx < smmu.num_smrgs());
    ZX_DEBUG_ASSERT(smmu.available_smrgs_.TestBit(smrg_ndx));
    DisableRegs(smmu.gr0_base_, smrg_ndx);
  }

  void EnableForContextBank(SmmuBti& owner, uint32_t cb_ndx) TA_REQ(owner.get_lock());
  void Disable(SmmuBti& owner) TA_REQ(owner.get_lock()) {
    owner.AssertOwned(*this);

    // Disable ourselves, and clear out the bookkeeping that indicates that we
    // were enabled.  There is no coming back from this state, and it will be
    // checked as we destruct.
    DisableRegs(gr0_base_, smrg_ndx_);
    gr0_base_ = hwreg::RegisterMmio{0};
    mode_ = S2CR_Type::kInvalid;
    cb_ndx_ = kInvalidCBNdx;
  }

  // Used only by console commands to clear the VALID flag in the match register
  // and force a situation where Stream ID matching for a BTI fails.
  void Invalidate(SmmuBti& owner) TA_REQ(owner.get_lock()) {
    gr0::SMR::Get(smrg_ndx_).ReadFrom(&gr0_base_).set_VALID(0).WriteTo(&gr0_base_);
  }

  // Called when users release quarantine on a BTI.  Typically, when a BTI goes
  // into fault mode the enforcement happens at the context bank level, but this
  // is called as well just in case the reason for the faults is that someone
  // used to console to invalidate the SIDs in a BTI.
  //
  // Note that during normal operation, we would never invalidate a BTI's SMRGs.
  // That would only happen at the end of the operational life of a BTI.  The
  // kernel console can be used to deliberately invalidate a BTI's SIDs for
  // manual testing purposes, or to help with the exploration of a platforms
  // specific behavior during bringup of a new platform.
  //
  void Revalidate(SmmuBti& owner) TA_REQ(owner.get_lock()) {
    gr0::SMR::Get(smrg_ndx_).ReadFrom(&gr0_base_).set_VALID(1).WriteTo(&gr0_base_);
  }

  S2CR_Type mode() const { return mode_; }
  uint32_t smrg_ndx() const { return smrg_ndx_; }
  SmrValue stream_ids() const { return stream_ids_; }
  uint32_t cb_ndx() const { return cb_ndx_; }

 private:
  friend class std::default_delete<StreamMatchRegGroup>;
  friend class SmmuBti;

  static constexpr uint32_t kInvalidCBNdx = 0xFFFFFFFF;

  static ktl::unique_ptr<StreamMatchRegGroup> Create(Smmu& smmu, uint32_t smrg_ndx,
                                                     SmrValue stream_ids) TA_REQ(smmu.get_lock());
  static zx::result<> ValidateNdx(Smmu& smmu, uint32_t smrg_ndx) TA_REQ(smmu.get_lock());
  static void DisableRegs(hwreg::RegisterMmio gr0_base, uint32_t smrg_ndx);

  StreamMatchRegGroup(uint32_t smrg_ndx, SmrValue stream_ids);
  ~StreamMatchRegGroup();

  zx::result<> AdoptRegisterState(Smmu& smmu) TA_REQ(smmu.get_lock());

  const uint32_t smrg_ndx_;
  const SmrValue stream_ids_;

  hwreg::RegisterMmio gr0_base_{0};
  S2CR_Type mode_{S2CR_Type::kInvalid};
  uint32_t cb_ndx_{kInvalidCBNdx};
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_STREAM_MATCH_REG_GROUP_H_
