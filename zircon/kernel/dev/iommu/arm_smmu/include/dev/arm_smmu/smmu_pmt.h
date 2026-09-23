// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_PMT_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_PMT_H_

#include <lib/zx/result.h>
#include <stdint.h>
#include <sys/types.h>

#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/utils.h>
#include <dev/iommu/pmt.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>
#include <vm/pinned_vm_object.h>

namespace arm_smmu {

class SmmuBti;

class SmmuPmt final : public iommu::Pmt, public fbl::DoublyLinkedListable<fbl::RefPtr<SmmuPmt>> {
 public:
  enum class State {
    kInitial,      // PMT has been created, but not successfully mapped and added to its owner BTI
    kActive,       // PMT has been created, mapped (or is in the process of mapping), and added to
                   // its owner BTI.
    kReleased,     // PMT has been explicitly unpinned.  Its pinned VMO is released and it is not on
                   // any BTI lists.
    kQuarantined,  // Well... not really.  Since we can enforce BTI access at a HW level, if a PMT
                   // hits zero handle without an explicit unpin, we can still immediately return
                   // the pages to the global page pool.  So, the pinned VMO is released, and to
                   // keep bookkeeping consistent, we still record the PMT state as having been
                   // quarantined.
  };

  static fbl::RefPtr<SmmuPmt> Create(SmmuBti& owner, PinnedVmObject pinned_vmo, BtiMode bti_mode,
                                     DeviceAspace::Allocation map_location)
      TA_REQ(owner.get_pmt_lock());

  SmmuPmt(const SmmuPmt&) = delete;
  SmmuPmt(SmmuPmt&&) = delete;
  SmmuPmt& operator=(const SmmuPmt&) = delete;
  SmmuPmt& operator=(SmmuPmt&&) = delete;

  //////////////////////////////////////////////////////////////////////////////
  //
  // Implementation of iommu::Pmt
  //
  //////////////////////////////////////////////////////////////////////////////
  zx::result<iommu::QueryAddressResult> QueryAddress(uint64_t query_offset, size_t query_size) final
      TA_EXCL(owner_->get_pmt_lock());
  void ReleasePinnedMemory() final TA_EXCL(owner_->get_pmt_lock());
  void OnDispatcherZeroHandles() final TA_EXCL(owner_->get_pmt_lock());
  //
  // end iommu::Pmt implementation

  void AssertOwnerPmtLockHeld() const TA_ASSERT(owner_->get_pmt_lock()) {
    owner_->get_pmt_lock().lock().AssertHeld();
  }

  State state() const TA_REQ(owner_->get_pmt_lock()) { return state_; }
  void set_state(State new_state) TA_REQ(owner_->get_pmt_lock()) { state_ = new_state; }

  PinnedVmObject TakePinnedVmo() TA_REQ(owner_->get_pmt_lock()) { return ktl::move(pinned_vmo_); }
  const PinnedVmObject& pinned_vmo() const TA_REQ(owner_->get_pmt_lock()) { return pinned_vmo_; }

  DeviceAspace::Allocation TakeMapLocation() TA_REQ(owner_->get_pmt_lock()) {
    return ktl::move(map_location_);
  }

  // Note: while PMT instances always have a "map location" which can be
  // queried, the map location instance itself is not always valid.  Callers
  // need to know that the location is valid (which can be tested using an
  // implicit or explicit cast to bool, or a comparison against nullptr, before
  // using it.
  const DeviceAspace::Allocation& map_location() const TA_REQ(owner_->get_pmt_lock()) {
    return map_location_;
  }

 private:
  friend class fbl::RefPtr<SmmuPmt>;  // Only RefPtrs can destroy us.

  SmmuPmt(fbl::RefPtr<SmmuBti> owner, PinnedVmObject pinned_vmo, BtiMode bti_mode);
  ~SmmuPmt() final;

  const fbl::RefPtr<SmmuBti> owner_;
  const BtiMode bti_mode_;
  TA_GUARDED(owner_->get_pmt_lock()) State state_ { State::kInitial };
  TA_GUARDED(owner_->get_pmt_lock()) DeviceAspace::Allocation map_location_ {};
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_PMT_H_
