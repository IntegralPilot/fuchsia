// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/assert.h>

#include <dev/arm_smmu/smmu_bti.h>
#include <dev/arm_smmu/smmu_pmt.h>
#include <dev/arm_smmu/utils.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>

namespace arm_smmu {

fbl::RefPtr<SmmuPmt> SmmuPmt::Create(SmmuBti& owner, PinnedVmObject pinned_vmo, BtiMode bti_mode,
                                     DeviceAspace::Allocation map_location) {
  fbl::AllocChecker ac;
  fbl::RefPtr<SmmuPmt> new_pmt =
      AdoptRef(new (&ac) SmmuPmt(fbl::RefPtr<SmmuBti>{&owner}, ktl::move(pinned_vmo), bti_mode));

  if (!ac.check()) {
    owner.ReleaseMapping(ktl::move(map_location));
    return nullptr;
  }

  new_pmt->AssertOwnerPmtLockHeld();
  new_pmt->map_location_ = ktl::move(map_location);

  return new_pmt;
}

SmmuPmt::SmmuPmt(fbl::RefPtr<SmmuBti> owner, PinnedVmObject pinned_vmo, BtiMode bti_mode)
    : iommu::Pmt{ktl::move(pinned_vmo)}, owner_{ktl::move(owner)}, bti_mode_{bti_mode} {
  ZX_DEBUG_ASSERT((bti_mode == BtiMode::kBypass) || (bti_mode == BtiMode::kTranslation));
}

SmmuPmt::~SmmuPmt() {
  // We should never destruct while we are active.
  ZX_DEBUG_ASSERT(state() != State::kActive);

  // The only legitimate reason for us to still be holding a pinned VMO, is
  // because we took ownership of the VMO while we were in the Initial state,
  // but never managed to make it to the active state.  It's ok if this happens,
  // we've never been exposed to any users, and the pinned memory will be
  // returned as our PinnedVmObject destructs.
  ZX_DEBUG_ASSERT((pinned_vmo().vmo() == nullptr) || (state() != State::kInitial));

  // The only way it should be possible for us to end up with an active map
  // location as we are being destructed would be for us to have successfully
  // created the mapping, but for our BTI to enter the fault state as we are
  // doing so.  Still, it _could_ happen, so be sure to check, just in case.
  if (map_location_) {
    ZX_DEBUG_ASSERT(state() == State::kInitial);
    owner_->ReleaseMapping(ktl::move(map_location_));
  }
}

zx::result<iommu::QueryAddressResult> SmmuPmt::QueryAddress(uint64_t query_offset,
                                                            size_t query_size) {
  if (!IsPageRounded(query_offset) || query_size == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Note, we need hold our lock while we verify that we are still active, and
  // that we still have a pinned VMO.  Once we do, we will hold a reference to
  // the underlying VMO and use it to answer our query after we drop the lock.
  //
  // If (someday) a fault happens which causes the underlying VMO to become
  // un-pinned (it would need to be deferred to a thread, we cannot unpin at
  // interrupt time), the worst case is that we manage to satisfy a request to
  // pin/map only to have the BTI itself enter the fault state and invalidate
  // the PMT we were in the process of creating.
  //
  Guard<Mutex> pmt_guard{&owner_->get_pmt_lock()};
  if (state() != State::kActive) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (pinned_vmo().vmo() == nullptr) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // If we are in translation mode, we can just use our DeviceAspace allocation
  // to figure out what address our hardware should be using.
  if (bti_mode_ == BtiMode::kTranslation) {
    if (!map_location_) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    // Make sure the query fits inside of the mapping in our device's address
    // space.
    const uint64_t size = map_location_->size;
    if ((query_offset >= size) || (query_size > (size - query_offset))) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }

    const uint64_t base = map_location_->base;
    return zx::ok(
        iommu::QueryAddressResult{.device_vaddr = base + query_offset, .size = query_size});
  }

  query_offset += pinned_vmo().offset();
  query_size = RoundUpPageSize(query_size);

  // If we are not in translate mode, we must be in bypass mode and we should
  // simply return the physical address of our pinned VMO.
  ZX_DEBUG_ASSERT(bti_mode_ == BtiMode::kBypass);

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
  for (uint32_t attempt = 0; attempt < 2; ++attempt) {
    paddr_t paddr = iommu::INVALID_PADDR;
    const zx_status_t status =
        pinned_vmo().vmo()->LookupContiguous(query_offset, query_size, &paddr);

    // Tolerate errors other than INVALID_ARGS or OUT_OF_RANGE only if this is
    // the second (and final) attempt.
    if (status != ZX_OK) {
      if ((status == ZX_ERR_INVALID_ARGS || status == ZX_ERR_OUT_OF_RANGE) || attempt) {
        return zx::error(status);
      }
    } else {
      ZX_DEBUG_ASSERT(paddr != iommu::INVALID_PADDR);
      return zx::ok(iommu::QueryAddressResult{.device_vaddr = paddr, .size = query_size});
    }

    // We shouldn't be here after our first attempt.  The second attempt must
    // always either succeed or fail.  If this is our first attempt, reduce our
    // query size to just one page.
    ZX_DEBUG_ASSERT(!attempt);
    query_size = kPageSize;
  }

  // We should never get here.  If we succeeded on our first or second attempt,
  // we would have already returned, and if we received _any_ errors on our
  // second attempt, we should have already bailed out.
  ASSERT(false);
  return zx::error(ZX_ERR_INTERNAL);
}

void SmmuPmt::ReleasePinnedMemory() { owner_->OnPmtUnpin(*this); }
void SmmuPmt::OnDispatcherZeroHandles() { owner_->OnPmtZeroHandles(*this); }

}  // namespace arm_smmu
