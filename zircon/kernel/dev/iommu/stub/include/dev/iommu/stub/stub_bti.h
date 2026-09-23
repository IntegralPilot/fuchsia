// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_STUB_INCLUDE_DEV_IOMMU_STUB_STUB_BTI_H_
#define ZIRCON_KERNEL_DEV_IOMMU_STUB_INCLUDE_DEV_IOMMU_STUB_STUB_BTI_H_

#include <lib/zx/result.h>
#include <stdint.h>

#include <dev/iommu/bti.h>
#include <dev/iommu/common.h>
#include <dev/iommu/pmt.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/ref_ptr.h>
#include <kernel/spinlock.h>
#include <ktl/utility.h>
#include <vm/pinned_vm_object.h>

class VmObject;

namespace iommu {

class StubPmt;
class StubIommu;

class StubBti final : public Bti, public fbl::DoublyLinkedListable<fbl::RefPtr<StubBti>> {
 public:
  enum class MoveToQuarantine { No, Yes };
  enum class ReportLeaks { No, Yes };

  StubBti(const StubBti&) = delete;
  StubBti(StubBti&&) = delete;
  StubBti& operator=(const StubBti&) = delete;
  StubBti& operator=(StubBti&&) = delete;

  static zx::result<fbl::RefPtr<Bti>> Create(fbl::RefPtr<StubIommu> iommu, uint64_t bti_txn_id);

  void ReleaseQuarantine() final;

  // Implementation of iommu::Bti
  zx::result<fbl::RefPtr<Pmt>> Map(PinnedVmObject pinned_vmo, uint32_t perms,
                                   RequireContiguousMapping req_contig) final;

  void OnDispatcherZeroHandles() final;

  // The min amt. of contiguous mapping this BTI can guarantee.
  uint64_t minimum_contiguity() const final;
  uint64_t aspace_size() const final;     // The total size of the device's address space.
  uint64_t pmo_count() const final;       // The number of active Pinned Memory Objects (PMTs).
  size_t quarantine_count() const final;  // The number of quarantined PMO/PMTs

  // Indicates that this BTI has encountered a "fault" such as a protocol
  // violation like a leaked PMT, or a HW translation fault.  The BTI will
  // refuse new pin operations until the driver takes control of its hardware,
  // and calls ReleaseQuarantine on the BTI to indicate that the fault has been
  // handled.
  bool in_fault_state() const final;

  Lock<SpinLock>& get_collection_lock() const TA_RET_CAP(collection_lock_) {
    return collection_lock_;
  }

  void AddToActiveList(StubPmt& pmt) TA_REQ(collection_lock_);
  void RemoveFromActiveList(StubPmt& pmt, MoveToQuarantine mtq,
                            ReportLeaks report_leaks = ReportLeaks::Yes) TA_REQ(collection_lock_)
      TA_EXCL(global_orphan_lock_);

  // Right now, we cannot print quarantine warnings with any spinlocks held.
  // The reasons for this are not the best.  The primary reason is that the
  // Print routine attempts to access the current thread's name via the
  // ThreadDispatcher, and not the kernel thread object.  The thread dispatcher
  // needs to hold its mutex in order to determine if it is still attached to
  // its kernel thread, which is where the thread's name storage exists.
  //
  // Why not simply access the name from the current _kernel_ Thread interface?
  // Because the name-storage for kernel threads is just a simple char array,
  // and has no locks to synchronize access to it.
  //
  // A simple fix for this would be to stop using just a char array for the
  // name, and switch to using a `fbl::Name` instead which is automatically
  // protected by a spinlock.  Until then, however, we need to avoid holding
  // any spinlocks when reporting quarantine warnings.
  void PrintQuarantineWarning(BtiPageLeakReason reason, size_t pmt_count, uint64_t page_count)
      TA_EXCL(global_orphan_lock_, collection_lock_) {
    Bti::PrintQuarantineWarning(reason, page_count, pmt_count);
  }

  // Utilities used by our child PMTs to manage orphaned state and OOPS reporting.
  bool is_orphaned() const TA_REQ(collection_lock_) { return is_orphaned_; }
  size_t quarantined_pmt_count() const TA_REQ(collection_lock_) { return quarantined_pmts_.size(); }
  uint64_t quarantined_page_count() const TA_REQ(collection_lock_) { return quarantined_pages_; }

 private:
  friend class StubIommu;             // Only StubIommus can create us.
  friend class fbl::RefPtr<StubBti>;  // Only RefPtrs can destroy us.

  using PmtList = fbl::SizedDoublyLinkedList<fbl::RefPtr<StubPmt>>;

  StubBti(fbl::RefPtr<StubIommu> iommu, uint64_t bti_id);
  ~StubBti() final;

  // The "global orphan" list holds a list of BTIs which have had their last
  // user-mode handle closed, but which still had actively pinned memory (either
  // quarantined or not) when they were closed.
  //
  // If all of the active PMTs held by an orphaned BTI are formally unpinned via
  // a call to `zx_pmt_unpin` before being closed, the BTI can be removed from
  // the orphan list and finally destroyed.
  //
  // However, if any of the PMTs managed by the BTI are leaked (closed before
  // calling `zx_pmt_unpin`, the BTI will effectively be stuck on the orphan
  // list forever.  It is not safe to return the pages from the leaked PMT to
  // the PMM, and the user no longer has a way to signal to the kernel that it
  // has taken control of its hardware via a call to `zx_bti_release_quarantine`
  // since it no longer has access to the BTI object anymore.
  //
  using OrphanList = fbl::DoublyLinkedList<fbl::RefPtr<StubBti>>;
  TA_ACQ_AFTER(collection_lock_) static inline DECLARE_SPINLOCK(StubPmt) global_orphan_lock_;
  TA_GUARDED(global_orphan_lock_) static OrphanList global_orphan_list_;

  // A cached copy of our orphaned status protected by the collection lock
  // instead of the global orphan lock.  Maintaining this cached copy allows us
  // to test to see if we have become orphaned without needing to acquire the
  // global orphan lock.  We only need to hold the orphan lock when we are
  // actively mutating the orphan list.
  TA_GUARDED(collection_lock_) bool is_orphaned_ { false };

  fbl::RefPtr<StubIommu> iommu_;
  TA_ACQ_BEFORE(global_orphan_lock_) mutable DECLARE_SPINLOCK(StubBti) collection_lock_;
  TA_GUARDED(collection_lock_) PmtList active_pmts_;
  TA_GUARDED(collection_lock_) PmtList quarantined_pmts_;
  TA_GUARDED(collection_lock_) uint64_t quarantined_pages_ { 0 };
};

}  // namespace iommu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_STUB_INCLUDE_DEV_IOMMU_STUB_STUB_BTI_H_
