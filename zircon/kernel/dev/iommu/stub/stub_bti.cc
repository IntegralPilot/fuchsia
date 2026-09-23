// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page/size.h>
#include <lib/zx/result.h>
#include <stdint.h>

#include <dev/iommu/stub/stub.h>
#include <dev/iommu/stub/stub_bti.h>
#include <dev/iommu/stub/stub_pmt.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <ktl/limits.h>
#include <ktl/utility.h>
#include <vm/vm_object.h>

namespace iommu {

StubBti::OrphanList StubBti::global_orphan_list_;

zx::result<fbl::RefPtr<Bti>> StubBti::Create(fbl::RefPtr<StubIommu> iommu, uint64_t bti_id) {
  fbl::AllocChecker ac;
  fbl::RefPtr<StubBti> ret = fbl::AdoptRef(new (&ac) StubBti{ktl::move(iommu), bti_id});

  if (ac.check()) {
    return zx::ok(ktl::move(ret));
  } else {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
}

StubBti::StubBti(fbl::RefPtr<StubIommu> iommu, uint64_t bti_id)
    : Bti{bti_id}, iommu_{ktl::move(iommu)} {}

StubBti::~StubBti() {
  ASSERT(active_pmts_.is_empty());
  ASSERT(quarantined_pmts_.is_empty());
}

zx::result<fbl::RefPtr<Pmt>> StubBti::Map(PinnedVmObject pinned_vmo, uint32_t perms,
                                          RequireContiguousMapping req_contig) {
  fbl::AllocChecker ac;
  fbl::RefPtr<StubPmt> new_pmt =
      fbl::AdoptRef(new (&ac) StubPmt(fbl::RefPtr{this}, ktl::move(pinned_vmo)));
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  if (const zx_status_t map_status = new_pmt->Map(perms, req_contig); map_status != ZX_OK) {
    // Looks like we failed the map call.  The failed call to map should have
    // already released the pinned vmo and set our state to Released so that we
    // pass our destructor asserts as we unwind. All we need to do here is
    // unwind.
    return zx::error(map_status);
  }

  // Success, add the new PMT to the active PMT list, transitioning the state
  // from kInitial to kActive in the process.
  {
    Guard<SpinLock, IrqSave> guard(&collection_lock_);
    AddToActiveList(*new_pmt);
  }

  return zx::ok(ktl::move(new_pmt));
}

void StubBti::ReleaseQuarantine() {
  // Move the quarantine list outside of the scope of the collection lock, then
  // release all of the quarantined memory.  Explicitly set the state of all of
  // the PMTs to kReleased as we go so that we don't fail any of our bookkeeping
  // asserts when the PMTs are finally destroyed.
  PmtList was_quarantined_list;
  {
    Guard<SpinLock, IrqSave> guard(&collection_lock_);
    uint64_t released_pages{0};
    for (StubPmt& pmt : quarantined_pmts_) {
      pmt.AssertOwnerCollectionLockHeld();
      DEBUG_ASSERT(pmt.state() == StubPmt::State::kQuarantined);
      released_pages += pmt.pages();
      pmt.set_state(StubPmt::State::kReleased);
    }

    DEBUG_ASSERT(released_pages == quarantined_pages_);
    was_quarantined_list = ktl::move(quarantined_pmts_);
    quarantined_pages_ = 0;
  }

  while (!was_quarantined_list.is_empty()) {
    fbl::RefPtr<StubPmt> release_me = was_quarantined_list.pop_front();
    release_me->ReleaseQuarantinedVmo();
  }
}

void StubBti::OnDispatcherZeroHandles() {
  size_t quarantined_pmt_count{0};
  uint64_t quarantined_page_count{0};

  {
    Guard<SpinLock, IrqSave> guard(&collection_lock_);

    // The only way to become orphaned is for our handle count to hit zero.  If
    // we are already flagged as orphaned, then someone has called
    // on-zero-handles twice, which should be impossible.
    DEBUG_ASSERT(!is_orphaned_);

    // We've hit zero handles, so user-mode no longer has any access to us.  If
    // we are still holding PMTs, then we are now "orphaned".  Hold an explicit
    // reference to ourselves on the global orphan list, and take a snapshot of
    // the quarantine stats before dropping our lock.  If the last reference to
    // us was dropped while we had quarantined PMTs, we need to report an OOPS
    // as user-mode has managed to both leak PMTs, and close their BTI making it
    // impossible for them to signal that they have taken control of their HW
    // using `zx_bti_release_quarantine`.
    if (!active_pmts_.is_empty() || !quarantined_pmts_.is_empty()) {
      Guard<SpinLock, IrqSave> orphan_guard(&global_orphan_lock_);

      global_orphan_list_.push_front(fbl::RefPtr{this});
      is_orphaned_ = true;
      quarantined_pmt_count = quarantined_pmts_.size();
      quarantined_page_count = quarantined_pages_;
    }
  }

  if (quarantined_pmt_count) {
    PrintQuarantineWarning(BtiPageLeakReason::BtiOrphanedWithQuarantinedPmts, quarantined_pmt_count,
                           quarantined_page_count);
  } else {
    DEBUG_ASSERT(quarantined_page_count == 0);
  }
}

uint64_t StubBti::minimum_contiguity() const { return kPageSize; }
uint64_t StubBti::aspace_size() const { return ktl::numeric_limits<uint64_t>::max(); }

uint64_t StubBti::pmo_count() const {
  Guard<SpinLock, IrqSave> guard(&collection_lock_);
  return active_pmts_.size() + quarantined_pmts_.size();
}

size_t StubBti::quarantine_count() const {
  Guard<SpinLock, IrqSave> guard(&collection_lock_);
  return quarantined_pmts_.size();
}

bool StubBti::in_fault_state() const {
  Guard<SpinLock, IrqSave> guard(&collection_lock_);
  return quarantined_pmts_.size() > 0;
}

void StubBti::AddToActiveList(StubPmt& pmt) {
  pmt.AssertOwnerCollectionLockHeld();
  DEBUG_ASSERT(!pmt.InContainer());
  DEBUG_ASSERT(pmt.state() == StubPmt::State::kInitial);
  active_pmts_.push_back(fbl::RefPtr{&pmt});
  pmt.set_state(StubPmt::State::kActive);
}

// Note: It is assumed that something at a higher level (eg; at the Dispatcher
// level) is holding a reference to our stub PMT.  If not, it might be possible
// for us to accidentally drop the last PMT reference while we are still inside
// of our collection lock, which would be bad as we cannot return the StubPmt
// memory to the heap while holding a spinlock.
void StubBti::RemoveFromActiveList(StubPmt& pmt, MoveToQuarantine mtq, ReportLeaks report_leaks) {
  pmt.AssertOwnerCollectionLockHeld();
  DEBUG_ASSERT(pmt.InContainer());
  DEBUG_ASSERT(pmt.state() == StubPmt::State::kActive);

  if (mtq == MoveToQuarantine::No) {
    active_pmts_.erase(pmt);
    pmt.set_state(StubPmt::State::kReleased);

    // If we are no longer managing _any_ PMTs (quarantined or otherwise) and we
    // are currently on the orphan list, we can now remove ourselves from the
    // list.  We should not need to worry about our ref-count hitting zero here,
    // as our caller must be holding a reference to us.
    if (is_orphaned() && active_pmts_.is_empty() && quarantined_pmts_.is_empty()) {
      Guard<SpinLock, IrqSave> guard(&global_orphan_lock_);
      global_orphan_list_.erase(*this);

      // Note: we deliberately do not clear the is_orphaned_ flag here.  A BTI
      // can only become orphaned exactly once, if it is still holding PMTs when
      // it hits zero-handles.  After that, the only way off of the list is
      // here, when the last PMT is formally unpinned instead of being leaked.
      // When that happens, we should be on the way to destruction.  The orphan
      // list no longer hold a reference to us, nor does any Dispatcher.  The
      // final references to us only exist in PMTs which have now been unpinned,
      // and will go away when those PMTs are closed.
      //
      // Keeping the flag set even though we are not in the orphan list
      // container allows us to continue to assert that no one has accidentally
      // called on-zero-handles more than once in the (hopefully) brief period
      // of time between now and destruction.
    }

  } else {
    quarantined_pages_ += pmt.pages();
    pmt.set_state(StubPmt::State::kQuarantined);
    quarantined_pmts_.push_back(active_pmts_.erase(pmt));
  }
}

}  // namespace iommu
