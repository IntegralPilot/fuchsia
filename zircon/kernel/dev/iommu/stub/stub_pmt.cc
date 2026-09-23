// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <stdint.h>
#include <sys/types.h>

#include <dev/iommu/stub/stub_pmt.h>

namespace iommu {

zx::result<QueryAddressResult> StubPmt::QueryAddress(uint64_t query_offset, size_t query_size) {
  if (!IsPageRounded(query_offset) || query_size == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // If we are no longer active, then we have no pinned+mapped addresses to report.
  {
    Guard<SpinLock, IrqSave> guard(&owner_->get_collection_lock());
    if (state() != State::kActive) {
      return zx::error(ZX_ERR_BAD_STATE);
    }
  }

  query_offset += pinned_vmo_.offset();
  query_size = RoundUpPageSize(query_size);

  // The user wants to know the device's base virtual address for the continuous
  // range [query_offset, query_offset + query_size) in the pinned VMO.  Start
  // by attempting to find a continuous region which matches the user's query
  // size.  If this fails because the arguments are completely invalid, or
  // because the query goes outside of the possible range of the pinned VMO
  // (even if it was already completely continuous), then fail out immediately.
  //
  // Otherwise, try again, but this time query a region starting at the user's
  // offset, but which is only a page long (the minimum possible continuous
  // region).
  //
  // TODO(johngro) : This should be a const reference, but LookupContiguous is
  // not flagged as const.  Fix this.
  VmObject& vmo = *pinned_vmo_.vmo();
  for (uint32_t attempt = 0; attempt < 2; ++attempt) {
    paddr_t paddr = INVALID_PADDR;
    const zx_status_t status = vmo.LookupContiguous(query_offset, query_size, &paddr);

    // Tolerate errors other than INVALID_ARGS or OUT_OF_RANGE only if this is
    // the second (and final) attempt.
    if (status != ZX_OK) {
      if ((status == ZX_ERR_INVALID_ARGS || status == ZX_ERR_OUT_OF_RANGE) || attempt) {
        return zx::error(status);
      }
    } else {
      DEBUG_ASSERT(paddr != INVALID_PADDR);
      return zx::ok(QueryAddressResult{.device_vaddr = paddr, .size = query_size});
    }

    // We shouldn't be here after our first attempt.  The second attempt must
    // always either succeed or fail.  If this is our first attempt, reduce our
    // query size to just one page.
    DEBUG_ASSERT(!attempt);
    query_size = kPageSize;
  }

  // We should never get here.  If we succeeded on our first or second attempt,
  // we would have already returned, and if we received _any_ errors on our
  // second attempt, we should have already bailed out.
  ASSERT(false);
  return zx::error(ZX_ERR_INTERNAL);
}

void StubPmt::ReleasePinnedMemory() {
  // If we've not already been explicitly unpinned, move our pinned VMO outside
  // of the locked scope so that the underlying VMO is unpinned and our reference
  // to it is deleted as the local PinnedVmObject goes out of scope.
  PinnedVmObject maybe_destroy;
  {
    Guard<SpinLock, IrqSave> guard(&owner_->get_collection_lock());
    if (state() == State::kActive) {
      DEBUG_ASSERT(pinned_vmo_.vmo() != nullptr);
      maybe_destroy = ktl::move(pinned_vmo_);

      // It should be safe to remove ourselves from our BTI's active collection
      // and drop the reference it held.  We are currently being kept alive by
      // the reference held by our PMT dispatcher.
      owner_->RemoveFromActiveList(*this, StubBti::MoveToQuarantine::No);
    } else {
      // If we are no longer active, then we should be able to assert that we
      // are either in the Released state.  It should be impossible for us to be
      // any other state, including the Quarantined state.
      //
      // Even if a `zx_pmt_unpin` operation were racing with a `zx_handle_close`
      // operation, this should be impossible.
      //
      // Unlike most syscalls which operate on handles, `zx_pmt_unpin` does not
      // just validate the handle and obtain a reference to the Dispatcher, it
      // actually removes the handle from the handle table and takes ownership
      // of the HandleOwner.  It is this HandleOwner who holds the handle
      // reference on the Dispatcher, and the Dispatcher cannot reach zero
      // handles while it is still living.  The only way for a PMT to become
      // quarantined is for it to reach OnZeroHandles before becoming unpinned.
      //
      // So, consider the following two cases.
      //
      // Case 1: The two operations are racing and using the same handle value,
      //         `H`, which is the final handle to the object.
      //
      // One of these operations wins the race and removes `H` from the handle
      // table, and the other operation fails to find `H` in the table.  If the
      // unpin wins, then it keeps the dispatcher above a zero-handle count
      // until after the unpin is completed, meaning that it cannot hit zero
      // handles and cannot become quarantined.  If the handle close wins, then
      // the pin operation will not find the handle in the handle table, and
      // ReleasePinnedMemory will never be called.  Meanwhile, the final handle
      // will close and the PMT will be quarantined.
      //
      // Case 2: The two operations are racing and using different handle values,
      //         `H_close` and `H_unpin`
      //
      // It really does not matter who wins the race in this case.  `H_unpin` is
      // going find and the HandleOwner alive until the unpin complete, ensuring
      // that the object cannot hit zero-handles (and therefore cannot become
      // quarantined) before the unpin is finished.  The outcome of the race
      // only determines who ends up calling on_zero_handles on the dispatcher,
      // the unpin operation if the handle close operation finishes first, or
      // the handle close operation if the unpin finishes first.
      DEBUG_ASSERT(state() == State::kReleased);
    }
  }
}

void StubPmt::OnDispatcherZeroHandles() {
  // Our dispatcher has hit zero handles, which means user-mode is finished with
  // us.  If we have are still active (and have not been explicitly released),
  // then we need to add ourselves to our owner's quarantine list.
  bool report_bti_quarantine{false};
  size_t quarantined_pmt_count{0};
  uint64_t quarantined_page_count{0};
  {
    Guard<SpinLock, IrqSave> guard(&owner_->get_collection_lock());
    if (state() == State::kActive) {
      DEBUG_ASSERT(InContainer());
      DEBUG_ASSERT(pinned_vmo_.vmo() != nullptr);

      // If our BTI has been orphaned by user-mode, and this is the first PMT
      // being leaked, then we need to report a BTI quarantine event in addition
      // to a PMT quarantine event.
      report_bti_quarantine = owner_->is_orphaned() && !owner_->quarantined_pmt_count();

      // Move ourselves from our owner's active list to its quarantine list, then record the
      // quarantine statistics for the quarantine warning(s) we are about to generate.
      owner_->RemoveFromActiveList(*this, StubBti::MoveToQuarantine::Yes);
      quarantined_pmt_count = owner_->quarantined_pmt_count();
      quarantined_page_count = owner_->quarantined_page_count();
    } else {
      // We are not active right now meaning that we must have been explicitly
      // released with a call to `zx_pmt_unpin`.  Assert this.
      DEBUG_ASSERT(state() == State::kReleased);
      DEBUG_ASSERT(!InContainer());
      DEBUG_ASSERT(pinned_vmo_.vmo() == nullptr);
    }
  }

  if (quarantined_pmt_count) {
    if (report_bti_quarantine) {
      owner_->PrintQuarantineWarning(Bti::BtiPageLeakReason::PmtQuarantinedWhenBtiOrphaned,
                                     quarantined_pmt_count, quarantined_page_count);
    } else {
      owner_->PrintQuarantineWarning(Bti::BtiPageLeakReason::PmtQuarantined, quarantined_pmt_count,
                                     quarantined_page_count);
    }
  } else {
    DEBUG_ASSERT(report_bti_quarantine == false);
    DEBUG_ASSERT(quarantined_page_count == 0);
  }
}

// Stub IOMMUs cannot actually perform any "mapping", so really all we can do
// here is check to make sure that request is valid and can be satisfied by the
// pinned VMO.  If anything goes wrong with our consistency checks, make sure to
// release our base class's PinnedVmObject.  Our destructors demand that we are
// no longer holding onto one by the time we hit end of life.
zx_status_t StubPmt::Map(uint32_t perms, RequireContiguousMapping req_contig) {
  const uint64_t vmo_offset = pinned_vmo_.offset();
  const uint64_t vmo_size = pinned_vmo_.size();

  DEBUG_ASSERT(pinned_vmo_.vmo() != nullptr);
  auto cleanup = fit::defer([this]() {
    {
      Guard<SpinLock, IrqSave> guard(&owner_->get_collection_lock());
      this->set_state(StubPmt::State::kReleased);
    }
    this->ReleaseQuarantinedVmo();
  });

  if (!IsPageRounded(vmo_offset) || vmo_size == 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (perms & ~(IOMMU_FLAG_PERM_READ | IOMMU_FLAG_PERM_WRITE | IOMMU_FLAG_PERM_EXECUTE)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (perms == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  // If the user requires a continuous mapping, then our pinned VMO has to be
  // contiguous as we have no MMU to perform HW translation for us.
  if (req_contig == RequireContiguousMapping::Yes) {
    // TODO(johngro) : This should be a const reference, but LookupContiguous is
    // not flagged as const.  Fix this.
    VmObject& vmo = *pinned_vmo_.vmo();
    paddr_t paddr{INVALID_PADDR};
    if (const zx_status_t status = vmo.LookupContiguous(vmo_offset, vmo_size, &paddr);
        status != ZX_OK) {
      return status;
    }
  }

  // Things went well.  Cancel the cleanup of our pinned vmo.
  cleanup.cancel();
  return ZX_OK;
}

void StubPmt::ReleaseQuarantinedVmo() {
  // This method is only used in two places.
  //
  // 1) When a BTI is releasing its quarantine.
  // 2) When a PMT fails the checks in Map and a user-mode pin operation fails.
  //    Note that this usage only occurs in a transient cleanup path during PMT
  //    setup which results in a failed `zx_bti_pin` call.  User-mode will never
  //    see an actually quarantined PMT.
  //
  // In either case, the BTI code should have already set our state to
  // kReleased so that we pass our destructor debug checks.
#ifdef DEBUG_ASSERT_IMPLEMENTED
  {
    Guard<SpinLock, IrqSave> guard(&owner_->get_collection_lock());
    DEBUG_ASSERT(state() == State::kReleased);
  }
#endif
  DEBUG_ASSERT(pinned_vmo_.vmo() != nullptr);
  pinned_vmo_.reset();
}

StubPmt::~StubPmt() {
  // By the time that we destruct, we should be able to assert that we have been
  // released, and that we are no longer on any of our owner's lists.  If we
  // were leaked, our BTI should continue to hold a reference to us until the
  // point where we the quarantine becomes released, which will explicitly set
  // the state of the PMT back to Released before allowing it to destruct.
  DEBUG_ASSERT(!InContainer());
  DEBUG_ASSERT(state() == State::kReleased);
}

}  // namespace iommu
