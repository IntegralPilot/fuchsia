// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_STUB_INCLUDE_DEV_IOMMU_STUB_STUB_PMT_H_
#define ZIRCON_KERNEL_DEV_IOMMU_STUB_INCLUDE_DEV_IOMMU_STUB_STUB_PMT_H_

#include <lib/zx/result.h>
#include <stdint.h>
#include <sys/types.h>

#include <dev/iommu/pmt.h>
#include <dev/iommu/stub/stub_bti.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>
#include <vm/pinned_vm_object.h>

namespace iommu {

// TODO(johngro) : Right now we depend on the upper level dispatcher's lock to
// serialize access to this object user-mode facing API.  Consider creating an
// explicit lock at this level so we can make more effective use of static
// annotations here.
class StubPmt final : public Pmt, public fbl::DoublyLinkedListable<fbl::RefPtr<StubPmt>> {
 public:
  enum class State {
    kInitial,      // PMT has been created, but not successfully mapped and added to its owner BTI
    kActive,       // PMT has been created, mapped, and added to its owner BTI.
    kReleased,     // PMT has been explicitly unpinned.  Its pinned VMO is released and it is not on
                   // any BTI lists.
    kQuarantined,  // PMT has been quarantined.  The VMO is still pinned and it is a member of its
                   // owning BTI's quarantine list.
  };

  StubPmt(fbl::RefPtr<StubBti> owner, PinnedVmObject pinned_vmo)
      : Pmt(ktl::move(pinned_vmo)), owner_(ktl::move(owner)) {}

  StubPmt(const StubPmt&) = delete;
  StubPmt(StubPmt&&) = delete;
  StubPmt& operator=(const StubPmt&) = delete;
  StubPmt& operator=(StubPmt&&) = delete;

  //////////////////////////////////////////////////////////////////////////////
  //
  // Implementation of iommu::Pmt
  //
  //////////////////////////////////////////////////////////////////////////////

  // TODO(johngro): These lock exclusion directives are not doing much here. The
  // only place we access this interface is from the dispatcher which owns us
  // through our base PMT's vtable.  At this point in time, the compiler does
  // not know our concrete type and cannot see the annotations. The good news is
  // that the dispatcher-level also does not know about our owner's lock, and
  // cannot be holding it, so we should be safe anyway.
  zx::result<QueryAddressResult> QueryAddress(uint64_t query_offset, size_t query_size) final
      TA_EXCL(owner_->get_collection_lock());
  void ReleasePinnedMemory() final TA_EXCL(owner_->get_collection_lock());
  void OnDispatcherZeroHandles() final TA_EXCL(owner_->get_collection_lock());

  //////////////////////////////////////////////////////////////////////////////
  //
  // Methods used by our StubBti owner to manage quarantine state, and
  // operations like mapping and releasing pinned memory.
  //
  //////////////////////////////////////////////////////////////////////////////

  // Check to make sure that the pin/map request is valid, and if so, transition
  // to the Active state and add ourselves to our owner's active PMT list.
  zx_status_t Map(uint32_t perms, RequireContiguousMapping req_contig)
    TA_EXCL(owner_->get_collection_lock());

  // Unconditionally release the pinned VMO we are holding.  It is illegal to
  // perform this operation more than once, or to do so with interrupts off or
  // while holding any spinlocks.
  void ReleaseQuarantinedVmo() TA_EXCL(owner_->get_collection_lock());

  // state() and set_state() are used used both in logic and DEBUG_ASSERTs by
  // our owner-StubBti.
  State state() const TA_REQ(owner_->get_collection_lock()) { return state_; }
  void set_state(State new_state) TA_REQ(owner_->get_collection_lock()) { state_ = new_state; }

  void AssertOwnerCollectionLockHeld() const TA_ASSERT(owner_->get_collection_lock()) {
    owner_->get_collection_lock().lock().AssertHeld();
  }

 private:
  friend class fbl::RefPtr<StubPmt>;  // Only RefPtrs can destroy us.

  ~StubPmt() final;

  // Notes about StubPmt lifecycles and the cyclical reference from a StubPmt to
  // its owning StubBti held here.
  //
  // StubPmts are always created as a child of a StubBti, and during
  // construction, they take a reference to their StubBti owner (held here).
  // This reference is deliberately const, and is only ever released when the
  // StubPmt instance destructs.
  //
  // During operation, there are up to two references in the system which keep
  // the StubPmt reference alive.  They are:
  //
  // 1) A reference held by the StubBti from either the StubBti's active or
  //    quarantined PMT list.
  // 2) A reference held by the PMT dispatcher.
  //
  // There are two ways that all of these reference can be dropped triggering
  // the destruction of a StubPmt instance, dropping the owner-StubBti reference
  // in the process.
  //
  // 1) A user formally unpins the PMT via the dispatcher interface.  While
  //    holding the dispatcher reference, ReleasePinnedMemory is called, and the
  //    PMT follows its owner link up into its owning BTI where PMT is removed
  //    from the owner's active list, dropping reference #1 in the process.
  //    Later on, the user drops the final handle to the PMT dispatcher.  Once
  //    again, while holding the dispatcher's reference to the StubPmt object,
  //    OnDispatcherZeroHandles is called, but the object has already been
  //    formally unpinned, so not much actually happens (just some
  //    DEBUG_ASSERTs).  The call unwinds, and the PMT dispatcher is destroyed.
  //    This drops the StubPmt reference (ref #2) triggering the destruction of
  //    the StubPmt, dropping the owner reference in the process.
  // 2) A user closes the final handle to the PMT dispatcher without formally
  //    unpinning it.  OnDispatcherZeroHandles is called, and the StubPmt
  //    reference held by the StubBti owner (ref #1) is moved from the active
  //    list to the quarantine list.  The PMT dispatcher now destructs, dropping
  //    ref #2 in the process.  At some point in time later on, the quarantine
  //    protocol is followed, and user-mode eventually calls ReleaseQuarantine
  //    on their BTI.  The reference from the BTI dispatcher to the StubBti
  //    instance is used to clear the StubBti's quarantine list, dropping ref #1
  //    in the process.  The StubPmt then destructs, dropping the StubBti owner
  //    reference in the process.
  //
  const fbl::RefPtr<StubBti> owner_;
  TA_GUARDED(owner_->get_collection_lock()) State state_ { State::kInitial };
};

}  // namespace iommu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_STUB_INCLUDE_DEV_IOMMU_STUB_STUB_PMT_H_
