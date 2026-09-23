// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_BTI_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_BTI_H_

#include <stdint.h>

#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/utils.h>
#include <dev/iommu/bti.h>
#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <ktl/limits.h>
#include <ktl/unique_ptr.h>

namespace arm_smmu {

class StreamMatchRegGroup;
class ContextBank;
class SmmuPmt;

class SmmuBti final : public iommu::Bti, public fbl::DoublyLinkedListable<fbl::RefPtr<SmmuBti>> {
 public:
  SmmuBti(const SmmuBti&) = delete;
  SmmuBti operator=(const SmmuBti&) = delete;
  SmmuBti(SmmuBti&&) = delete;
  SmmuBti operator=(SmmuBti&&) = delete;

  zx::result<fbl::RefPtr<iommu::Pmt>> Map(PinnedVmObject pinned_vmo, uint32_t perms,
                                          iommu::RequireContiguousMapping req_contig) final;
  void ReleaseQuarantine() final;
  void OnDispatcherZeroHandles() final;
  uint64_t minimum_contiguity() const final;
  uint64_t aspace_size() const final;
  uint64_t pmo_count() const final;
  size_t quarantine_count() const final;
  bool in_fault_state() const final;

  // Create a new BtiContext and have it take control of the (mandatory) SMRG
  // and (optional) Context Bank.  On success, any of the supplied resources
  // (SMRG and CB) will be marked as unavailable in the owning SMMU's
  // bookkeeping.
  static fbl::RefPtr<SmmuBti> Create(Smmu& smmu, ktl::unique_ptr<StreamMatchRegGroup> smrg,
                                     ktl::unique_ptr<ContextBank> context_bank,
                                     BtiMode initial_mode) TA_REQ(smmu.get_lock())
      TA_EXCL(smmu.get_irq_lock());

  void Shutdown(Smmu& smmu) TA_REQ(smmu.get_lock()) TA_EXCL(smmu.get_irq_lock());

  // Test to see if a stream match register's ID/MASK pair could potentially
  // match any of the SMR values for any of the SMRGs who are members of this
  // SmmuBti.
  bool SmrIntersects(SmrValue stream_ids) const TA_EXCL(lock_);

  // Add a new SMRG to the existing set of SMRGs for this BTI.  If SMRG being
  // added is in translate mode, then it must be using the same context bank as
  // any pre-existing SMRGs in this BTI.
  void AddSmrg(Smmu& smmu, ktl::unique_ptr<StreamMatchRegGroup> smrg) TA_REQ(smmu.get_lock())
      TA_EXCL(lock_);

  zx::result<> SetMode(BtiMode mode) TA_EXCL(lock_) {
    Guard<SpinLock, IrqSave> guard{&lock_};
    return SetModeLocked(mode);
  }

  void HandleFaultLocked() TA_REQ(lock_);

  auto& get_lock() const TA_RET_CAP(lock_) { return lock_; }
  auto& get_pmt_lock() const TA_RET_CAP(pmt_lock_) { return pmt_lock_; }
  uint32_t cb_ndx_locked() const TA_REQ(lock_);
  uint32_t cb_ndx() const TA_EXCL(lock_);
  void AssertCanary() const { canary_.Assert(); }
  void AssertOwned(StreamMatchRegGroup& smrg) TA_REQ(lock_);
  void AssertOwned(ContextBank& cb) TA_REQ(lock_) { ZX_DEBUG_ASSERT(&cb == context_bank_.get()); }

  const Smmu& smmu() const {
    ZX_DEBUG_ASSERT(smmu_ != nullptr);
    return *smmu_;
  }

  void ReleaseMapping(DeviceAspace::Allocation mapping) TA_REQ(pmt_lock_);

  // Routines used by PMTs when handling closure and leaking.
  //
  void OnPmtUnpin(SmmuPmt& pmt) TA_EXCL(lock_);
  void OnPmtZeroHandles(SmmuPmt& pmt) TA_EXCL(lock_);

  // This BTI has reached the end of its life and needs to conduct final
  // cleanup.  There are two possible ways to get here:
  //
  // 1) The final user-mode handle to the BTI is closed, and the BTI does not
  //    have any active or quarantined PMTs (OnDispatcherZeroHandles).
  // 2) The BTI is orphaned (its final handle was closed while it still held
  //    PMTs), however the final PMT has now been unpinned and its final handle
  //    closed, and there are no quarantined PMTs (OnPmtZeroHandles)
  void OnEndOfLife() TA_EXCL(lock_);

 private:
  friend class Smmu;
  friend class fbl::RefPtr<SmmuBti>;

  static zx::result<ktl::unique_ptr<DeviceAspace>> CreateAspace();

  // A callback we can inject into a DeviceAspace Map/Unmap/Shutdown operation
  // which it can use to invalidate our TLBs at the proper point in its
  // operation.
  static void TlbInvalThunk(void* thiz, uint64_t base, uint64_t size);

  SmmuBti(fbl::RefPtr<Smmu> smmu, uint64_t bti_id);
  ~SmmuBti();

  zx::result<> InitializeAspace() TA_REQ(pmt_lock_) TA_EXCL(lock_);
  void AttachAspace(ktl::unique_ptr<DeviceAspace> aspace) TA_REQ(pmt_lock_, lock_);

  void AddSmrgLocked(Smmu& smmu, ktl::unique_ptr<StreamMatchRegGroup> smrg)
      TA_REQ(smmu.get_lock(), lock_);
  zx::result<> SetModeLocked(BtiMode mode) TA_REQ(lock_);

  // A thin wrapper over our base class implementation which serves only to
  // carry our lock annotations.
  void PrintQuarantineWarning(BtiPageLeakReason reason, uint64_t total_leaked_pages,
                              size_t total_leaked_vmos) TA_EXCL(lock_) {
    iommu::Bti::PrintQuarantineWarning(reason, total_leaked_pages, total_leaked_vmos);
  }

  // Console support
  void CmdShow(int ilvl, bool show_pmts, bool verbose) const TA_EXCL(pmt_lock_, lock_);
  void CmdLock(uint32_t ndx) TA_EXCL(pmt_lock_, lock_);
  zx::result<> InvalidateSids() TA_EXCL(lock_);
  const char* RenderSidList(ktl::span<char> buffer) const TA_REQ(lock_);
  BtiMode mode() const TA_REQ(lock_) { return mode_; }

  fbl::Canary<fbl::magic("SBTI")> canary_;
  const fbl::RefPtr<Smmu> smmu_;

  mutable DECLARE_SPINLOCK(SmmuBti) lock_;
  mutable DECLARE_MUTEX(SmmuBti) pmt_lock_;

  TA_GUARDED(lock_) BtiMode mode_ { BtiMode::kInvalid };
  TA_GUARDED(lock_) fbl::DoublyLinkedList<ktl::unique_ptr<StreamMatchRegGroup>> smrg_list_;
  TA_GUARDED(lock_) ktl::unique_ptr<ContextBank> context_bank_;
  TA_GUARDED(pmt_lock_) fbl::SizedDoublyLinkedList<fbl::RefPtr<SmmuPmt>> active_pmt_list_;
  // Note, we don't need to actually hold references to "quarantined" PMTs.  If
  // a user leaks a PMT without unpinning it, we simply enter the fault state
  // and lock down this BTI so that the actual initiator hardware cannot access
  // any memory anymore, after which we can return all pinned physical memory to
  // the global pool.  We do need to maintain a count of the number of virtually
  // quarantined PMTs, however, in order to comply with the get_info reporting
  // rules.
  TA_GUARDED(pmt_lock_) uint64_t quarantined_pmt_count_ { 0 };
  TA_GUARDED(pmt_lock_) uint64_t quarantined_page_count_ { 0 };

  // A flag indicating that the final user-mode handle to this BTI was closed,
  // but it still had PMTs (either active or quarantined) under its control.
  TA_GUARDED(pmt_lock_) bool orphaned_ { false };

  // Protected by its own mutexes internally.
  TA_GUARDED(pmt_lock_) ktl::unique_ptr<DeviceAspace> aspace_;
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_BTI_H_
