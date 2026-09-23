// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/assert.h>

#include <dev/arm_smmu/context_bank.h>
#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_bti.h>
#include <dev/arm_smmu/smmu_pmt.h>
#include <dev/arm_smmu/stream_match_reg_group.h>
#include <dev/arm_smmu/utils.h>
#include <dev/iommu/pmt.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>

namespace arm_smmu {

SmmuBti::SmmuBti(fbl::RefPtr<Smmu> smmu, uint64_t bti_id)
    : iommu::Bti{bti_id}, smmu_{ktl::move(smmu)} {
  ZX_DEBUG_ASSERT(smmu_.get() != nullptr);
}

SmmuBti::~SmmuBti() {
  // By the time we destruct, we should have been shut down already and all of
  // our resources explicitly released.
  ZX_DEBUG_ASSERT(smmu_ == nullptr);
  ZX_DEBUG_ASSERT(context_bank_ == nullptr);
  ZX_DEBUG_ASSERT(smrg_list_.is_empty());
}

zx::result<fbl::RefPtr<iommu::Pmt>> SmmuBti::Map(PinnedVmObject pinned_vmo, uint32_t perms,
                                                 iommu::RequireContiguousMapping req_contig) {
  const uint64_t vmo_offset = pinned_vmo.offset();
  const uint64_t vmo_size = pinned_vmo.size();

  // Start by checking the basics of our request.  IOW - the stuff we can
  // validate without needing to lock anything.
  if (!IsPageRounded(vmo_offset) || vmo_size == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (perms & ~(IOMMU_FLAG_PERM_READ | IOMMU_FLAG_PERM_WRITE | IOMMU_FLAG_PERM_EXECUTE)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (perms == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // TODO(https://fxbug.dev/520619827): Resolve this.  Right now, DWC3 tries to
  // make write only mappings.  If we tell them no, things spiral out of control
  // rapidly.
  //
  // 1) The driver does not come up.  This is obvious and expected.
  // 2) Someone ends up closing their BTI.  This is not right.  DF is supposed
  //    to be escrowing these things, no matter what happens with the driver who
  //    uses the BTI.
  // 3) Then, something goes wrong in the driver system such that an attempt to
  //    `dm reboot` just hangs the shell.
  //
  // We need to figure out the proper policy here.  If someone wants a
  // particular level of enforcement, and we cannot provide it, what should we
  // be doing?  Deny the request and allow them to retry?  Silently accept the
  // request and give them a less strict level of access?  Allow them to pass a
  // flag which determines if they want strict or lax enforcement?
#if 0
  // It is not possible for an SMMU using VMSAv8-64 translation tables to create
  // permissions which allow write, but do not allow read.  If someone asks for
  // this configuration, reject the request so the driver knows that this is not
  // possible, and does not accidentally end up providing read access to a
  // device it didn't intend to provide.
  if (((perms & IOMMU_FLAG_PERM_READ) == 0) && ((perms & IOMMU_FLAG_PERM_WRITE) != 0)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
#endif

  // If there has been a request for a contiguous mapping, then check to see if
  // our pinned VMO is (in fact) contiguous before attempting obtaining our
  // lock.  Once we are holding our lock, we can safely check to see if we are
  // operating in Bypass mode, where we will need to fail the operation if the
  // user desires a contiguous mapping, but the pinned VMO is not contiguous.
  const zx_status_t contiguous_vmo_status = [&]() {
    if (req_contig == iommu::RequireContiguousMapping::No) {
      return ZX_OK;
    }

    paddr_t paddr{iommu::INVALID_PADDR};
    VmObject& vmo = *pinned_vmo.vmo();
    return vmo.LookupContiguous(vmo_offset, vmo_size, &paddr);
  }();

  // Hold our spinlock while we check to make sure we are in a state where
  // mapping is possible. IOW - we should be in either Bypass or Translation
  // mode.  If are in something like fault mode, adopted mode, or shutdown mode,
  // we are likely to stay there for a long time (if not forever), and it is
  // worth it to make a quick check to be sure that there is a chance that this
  // request can succeed before going further.
  //
  // After that, speculatively allocate a SmmuPmt object.  We have to do that
  // with our lock dropped (can't hold a spinlock and touch the heap).
  //
  // After this, however, we need to re-enter our lock and re-validate our mode.
  // It is possible that while we were off fetching memory from the heap, we
  // took a context interrupt and entered the fault state.  In a situation like
  // this, we need to deny the map request as we required to do when a BTI is
  // faulted.
  BtiMode observed_mode;
  {
    Guard<SpinLock, IrqSave> guard{&lock_};

    if ((mode() != BtiMode::kBypass) && (mode() != BtiMode::kTranslation)) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    // If we are not in translation mode, and the user both wants a contiguous
    // mapping but the underlying pinned VMO is not contiguous (computed in
    // `contiguous_vmo_status`, above) then we need to bail out here.
    if ((mode() != BtiMode::kTranslation) && (contiguous_vmo_status != ZX_OK)) {
      return zx::error(contiguous_vmo_status);
    }

    observed_mode = mode();
  }

  // Perform our mapping if we are in translation mode.  Then, then create our
  // PMT, giving it ownership of any new mapping we just created.
  Guard<Mutex> pmt_guard{&pmt_lock_};
  DeviceAspace::Allocation mapped_location;

  if (observed_mode == BtiMode::kTranslation) {
    ZX_DEBUG_ASSERT(aspace_ != nullptr);

    DeviceAspace::TlbInvalOp tlb_inval_op{TlbInvalThunk, this};
    zx::result<DeviceAspace::Allocation> maybe_location =
        aspace_->Map(pinned_vmo, perms, tlb_inval_op);
    if (!maybe_location.is_ok()) {
      return maybe_location.take_error();
    }

    // Our DeviceAspace has updated our page tables, flushed the updates out of
    // the CPU data cache, and invalidated the TLBs.  Take ownership of our new
    // mapping and pass it along to the PMT we are about to create.
    mapped_location = ktl::move(maybe_location.value());
  }

  // Create our PMT and give it our mapped location (if any).  If something goes wrong, the
  // factory method will take care of removing the mapping via our ReleaseMapping method.
  fbl::RefPtr<SmmuPmt> pmt =
      SmmuPmt::Create(*this, ktl::move(pinned_vmo), observed_mode, ktl::move(mapped_location));
  if (pmt == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  {
    // Now enter our lock and attempt to finish the mapping operation.  We need to
    // hold the state of the BTI constant during the rest of the process.
    Guard<SpinLock, IrqSave> guard{&lock_};

    // We made it this far because we observed our BTI as being in either the
    // Bypass or Translation state.  There is no legal way for a BTI to change from
    // Bypass to Translation (or vice-versa) after being created, so if the
    // current state does not match our observed state, then we must have either
    // faulted or been shut down and will need to bail out.  The VMO which had
    // been pinned has been transferred to our PMT and will be unpinned
    // automatically as the PMT destructs.
    //
    // Note that this assumption is predicated on the notion that the Dispatcher
    // level is holding locks which prevent us from:
    //
    // 1) Faulting at an IRQ level and having the driver level BTI enter the fault
    //    state, then
    // 2) Recovering from the fault with a syscall call to ReleaseQuarantine while
    //    we were creating the PMT object.
    if (mode() != observed_mode) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    // Add the PMT to the list of active PMTs, and mark it as being in the kActive
    // state.
    [&]() TA_REQ(pmt_lock_) {
      pmt->AssertOwnerPmtLockHeld();
      pmt->set_state(SmmuPmt::State::kActive);
      active_pmt_list_.push_back(pmt);
    }();
  }

  return zx::ok(ktl::move(pmt));
}

void SmmuBti::ReleaseQuarantine() {
  // The mode we will want to return to depends on the mode our SMMU is
  // operating in.
  const BtiMode target_mode = [&]() -> BtiMode {
    switch (smmu().op_mode()) {
      case ArmSmmuMode::kPassthru:
        return BtiMode::kBypass;

      case ArmSmmuMode::kEnforced:
        return BtiMode::kTranslation;

      case ArmSmmuMode::kDisabled:
      default:
        ASSERT(false);
        return BtiMode::kFault;
    }
  }();

  {
    Guard<Mutex> pmt_guard{&pmt_lock_};
    Guard<SpinLock, IrqSave> guard{&lock_};

    // No one should be calling ReleaseQuarantine on us unless we are in one of
    // the 3 operational states:  Fault, Bypass, or Translation.
    ZX_DEBUG_ASSERT((mode() == BtiMode::kFault) || (mode() == BtiMode::kBypass) ||
                    (mode() == BtiMode::kTranslation));

    // ReleaseQuarantine should be idempotent.  If we are already operating in
    // Bypass or Translation, there is nothing to do.
    if (mode() == BtiMode::kFault) {
      // Reset our "quarantine" bookkeeping.
      quarantined_pmt_count_ = 0;
      quarantined_page_count_ = 0;

      zx::result<> set_mode_result = SetModeLocked(target_mode);
      ZX_DEBUG_ASSERT(set_mode_result.is_ok());
    }
  }

  // Re-enable our context bank interrupt if needed.
  smmu_->ReenableContextBankIrq(*this);
}

void SmmuBti::OnDispatcherZeroHandles() {
  // Start by checking to see if we still have any PMTs, either active or
  // quarantined.  If we do, we cannot actually fully shut down yet.  Instead,
  // we have become "orphaned".
  //
  // If we still have active PMTs (nothing quarantined) we have to stick around,
  // allowing our HW to access the memory it has pinned until it formally unpins
  // the last PMT it had pinned.  Only then can we fully shut down.
  //
  // If we have quarantined memory, the situation is slightly worse, at least if
  // we are in passthru mode.  If a PMT has been leaked, and its BTI is
  // orphaned, there is no way for user-mode to tell the kernel that it has
  // taken control of its hardware and that it is now safe to un-quarantine its
  // quarantined PMTs.  Typically that would happen with a call to
  // `zx_bti_release_quarantine`, but the last handle to the BTI has been closed
  // preventing user-mode from properly following the protocol.
  //
  // In passthru mode, we can only grant or deny a device access to *all* of
  // physical memory, not only portions of it.  This is good in-so-much as the
  // hardware can be prevented from accessing any memory (allowing us to return
  // quarantined memory to the physical pool) however we need to stay in
  // deny/fault mode pretty much forever as there is no way for user-mode to
  // give us an all-clear sign given that we are orphaned.
  //
  // We should be able to do better in enforced mode.  Once all of the active
  // PMTs have either been formally unpinned, or even leaked and quarantined, we
  // should be able to destroy the BTI instance even if it has been orphaned.
  // The PTE entries will already have been removed and the HW will not be able
  // to access any memory.  If a new instance happens to be re-created, newly
  // pinned memory will only be addressable on a per-physical-page basis, and
  // the newly created BTI will not have any valid PTEs, so the hardware will
  // not be able to access any memory, at least not until it successfully pins
  // new pages.  It is very important for a driver to make certain that has
  // stopped all DMA before pinning any new memory.  Failure to do so risks the
  // driver either leaking or corrupting its newly pinned memory because of its
  // out of control DMA.
  //
  uint64_t quarantined_page_count = 0;
  uint64_t quarantined_pmt_count = 0;
  bool was_orphaned = false;
  {
    Guard<Mutex> pmt_guard{&pmt_lock_};
    Guard<SpinLock, IrqSave> guard{&lock_};
    ZX_DEBUG_ASSERT(!orphaned_);

    quarantined_pmt_count = quarantined_pmt_count_;
    quarantined_page_count = quarantined_page_count_;

    // These must either both be zero, or both be non-zero.
    ZX_DEBUG_ASSERT((quarantined_pmt_count == 0) == (quarantined_page_count == 0));

    if (quarantined_page_count && (smmu_->op_mode() != ArmSmmuMode::kEnforced)) {
      // If we have quarantined PMTs, we should already be in the kFault state.
      ZX_DEBUG_ASSERT(mode() == BtiMode::kFault);
      orphaned_ = true;
    } else {
      // If we have no (logically) quarantined PMTs, or we are operating in
      // Enforced mode, then we are orphaned iff we have active PMTs which are
      // still alive.
      orphaned_ = !active_pmt_list_.is_empty();
    }

    was_orphaned = orphaned_;
  }

  if (!was_orphaned) {
    // If we were not orphaned, then we have no active PMTs and no quarantined
    // PMTs, and user-mode no longer has any handles to us.  We can now safely
    // shut down and remove ourselves from our parent SMMU instance.
    OnEndOfLife();
  } else {
    // If we are now orphaned because we have quarantined PMTs, we need to log a
    // DRIVER OOPS warning.
    PrintQuarantineWarning(BtiPageLeakReason::BtiOrphanedWithQuarantinedPmts,
                           quarantined_page_count, quarantined_pmt_count);
  }
}

void SmmuBti::OnEndOfLife() {
  // We should never be calling OnEndOfLife twice.
  ZX_DEBUG_ASSERT(smmu_ != nullptr);
  smmu_->ShutdownBti(*this);

  // The Smmu ref pointer is const member of SmmuBti to prevent us from
  // accidentally releasing it.  That said, here is the only place where we do
  // want to explicitly release the pointer, so take care of it using a
  // const-cast.
  const_cast<fbl::RefPtr<Smmu>&>(smmu_).reset();
}

uint64_t SmmuBti::minimum_contiguity() const {
  switch (smmu_->op_mode()) {
    // If we are operating in passthru mode, then we are not actually doing any
    // translation.  We cannot guarantee any contiguity beyond the CPUs' page
    // size.
    case ArmSmmuMode::kPassthru:
      return kPageSize;

    // If we are operating in fully-enforced mode, then we currently guarantee
    // that all of our mappings will be made contiguous from the perspective of
    // the initiator HW.
    case ArmSmmuMode::kEnforced:
      return ktl::numeric_limits<uint64_t>::max();

    // It should be impossible for our SMMU to be in Disabled mode.  If the
    // bootloaders configured the SMMUs to operate in Disabled mode, then we
    // should never have created an Smmu instance, and therefore should not have
    // been able to create any SmmuBti instances either.  We should have ended
    // up (under the hood) creating a StubIommu instance instead.
    case ArmSmmuMode::kDisabled:
    default:
      ZX_DEBUG_ASSERT(false);
      return 0;
  }
}

uint64_t SmmuBti::aspace_size() const {
  switch (smmu_->op_mode()) {
    // If we are operating in passthru mode, then the address space that
    // initiator HW sees is going to be be the physical address space of the
    // system, which is functionally unlimited in size.
    case ArmSmmuMode::kPassthru:
      return ktl::numeric_limits<uint64_t>::max();

    // If we are operating in fully-enforced mode, then the address space that
    // initiator HW sees depends on this BTI's context bank's configuration.
    case ArmSmmuMode::kEnforced: {
      Guard<SpinLock, IrqSave> guard{&lock_};
      return context_bank_->aspace_size();
    }

    // See above, it should be impossible to get here.
    case ArmSmmuMode::kDisabled:
    default:
      ZX_DEBUG_ASSERT(false);
      return 0;
  }
}

uint64_t SmmuBti::pmo_count() const {
  Guard<Mutex> pmt_guard{&pmt_lock_};
  return active_pmt_list_.size() + quarantined_pmt_count_;
}

size_t SmmuBti::quarantine_count() const {
  Guard<Mutex> pmt_guard{&pmt_lock_};
  return quarantined_pmt_count_;
}

bool SmmuBti::in_fault_state() const {
  Guard<SpinLock, IrqSave> guard{&lock_};
  // It should not be possible to get here unless we are in an operational state.
  ZX_DEBUG_ASSERT((mode() == BtiMode::kFault) || (mode() == BtiMode::kBypass) ||
                  (mode() == BtiMode::kTranslation));
  return mode() == BtiMode::kFault;
}

fbl::RefPtr<SmmuBti> SmmuBti::Create(Smmu& smmu, ktl::unique_ptr<StreamMatchRegGroup> smrg,
                                     ktl::unique_ptr<ContextBank> context_bank,
                                     BtiMode initial_mode) {
  // We always use the raw value of our first SMRG's match register as our
  // opaque "bti id".
  ZX_DEBUG_ASSERT(smrg != nullptr);
  const uint64_t bti_id = smrg->stream_ids().value();

  fbl::AllocChecker ac;
  fbl::RefPtr<SmmuBti> bti = fbl::AdoptRef<SmmuBti>(new (&ac) SmmuBti(fbl::RefPtr{&smmu}, bti_id));
  if (!ac.check()) {
    return nullptr;
  }

  {
    Guard<SpinLock, IrqSave> guard{&bti->lock_};

    // If this hardware is being adopted, set our initial mode to adopted before
    // proceeding.  This will prevent SetModeLocked from attempting to
    // reconfigure the hardware.  Adopted hardware is supposed to stay in the
    // state it was in when handed to us by the bootloader, and should not even
    // actually change state.
    if (initial_mode == BtiMode::kAdopted) {
      ZX_DEBUG_ASSERT(!context_bank || context_bank->mode() == BtiMode::kAdopted);
      bti->mode_ = BtiMode::kAdopted;
    }

    if (context_bank != nullptr) {
      ZX_DEBUG_ASSERT(smmu.available_cbs_.TestBit(context_bank->cb_ndx()));
      smmu.available_cbs_.ClrBit(context_bank->cb_ndx());
      bti->context_bank_ = ktl::move(context_bank);
    }

    bti->AddSmrgLocked(smmu, ktl::move(smrg));
  }

  // If we are supposed to be operating in translation mode, we need to allocate
  // and initialize a device address space in order to implement the bookkeeping
  // we need for mapping memory.
  //
  // Note that we had to drop our spinlock to (potentially) initialize our
  // address space, and re-acquire it afterwards.  We cannot hold our spinlock
  // during aspace initialization as we will need to interact with the heap and
  // the PMM in the process of doing so.
  //
  // We are in the process of constructing this BTI instance.  No other threads
  // (nor any interrupt handlers) can know about it yet, so there are no
  // concurrency issues we need to worry about, at least not yet.  The first
  // time some other concurrent execution context could possibly become aware of
  // us will be when we enabled context bank interrupts, at the very end of the
  // second locked-scope below.
  if (initial_mode == BtiMode::kTranslation) {
    Guard<Mutex> pmt_guard{&bti->pmt_lock_};
    if (zx::result<> res = bti->InitializeAspace(); !res.is_ok()) {
      dprintf(INFO, "ERROR - failed to construct aspace for BTI (%d)\n", res.error_value());
      return nullptr;
    }
  }

  {
    Guard<SpinLock, IrqSave> guard{&bti->lock_};
    // In theory, setting the initial mode can never fail.  All of the resources
    // which might be needed (the SMRG and Context Bank) have already been
    // allocated, we just need to set up their registers.  No PTE pages ever
    // have to be allocated at this point in time as we will (initially) have no
    // pinned memory.
    [[maybe_unused]] zx::result<> res = bti->SetModeLocked(initial_mode);
    ZX_DEBUG_ASSERT(!res.is_error());

    // If we have a context bank, associate ourselves with our context bank's
    // interrupt, then enable the interrupt at the context bank level.
    if (bti->context_bank_ != nullptr) {
      const uint32_t cb_ndx = bti->context_bank_->cb_ndx();
      smmu.AssociateBtiIrq(*bti, cb_ndx);

      hwreg::RegisterMmio cb_base = smmu.get_cb_base(cb_ndx);
      s1cbr::SCTLR::Get().ReadFrom(&cb_base).set_CFIE(1).WriteTo(&cb_base);
    }
  }

  return bti;
}

void SmmuBti::Shutdown(Smmu& smmu) {
  fbl::SizedDoublyLinkedList<fbl::RefPtr<SmmuPmt>> local_pmt_list;
  fbl::DoublyLinkedList<ktl::unique_ptr<StreamMatchRegGroup>> local_smrg_list;
  ktl::unique_ptr<ContextBank> local_context_bank;

  {
    Guard<Mutex> pmt_guard{&pmt_lock_};

    {
      Guard<SpinLock, IrqSave> guard{&lock_};
      ZX_DEBUG_ASSERT(mode() != BtiMode::kShutdown);

      // Lock down all of the hardware, and place ourselves in the shutdown
      // state first. This should never fail as we should never be shutting down
      // any adopted configuration.
      zx::result mode_res = SetModeLocked(BtiMode::kShutdown);
      ASSERT(mode_res.is_ok());

      // Mark our resources in the SMMU bookkeeping as being available for
      // allocation once again.  We are finished messing with the registers; they
      // belong to our SMMU instance once again.
      for (const StreamMatchRegGroup& smrg : smrg_list_) {
        ZX_DEBUG_ASSERT(!smmu.available_smrgs_.TestBit(smrg.smrg_ndx()));
        smmu.available_smrgs_.SetBit(smrg.smrg_ndx());
      }

      if (context_bank_ != nullptr) {
        // At this point in time, we should be certain that we no longer have any
        // registered context bank IRQs, and that there are not any context bank
        // interrupts in flight targeting this BTI.  This was taken care of for us
        // by our SMMU instance during Smmu::ShutdownBti.
        ZX_DEBUG_ASSERT(!smmu.available_cbs_.TestBit(context_bank_->cb_ndx()));
        smmu.available_cbs_.SetBit(context_bank_->cb_ndx());
      }

      // If we are shutting down, we should not have any active or quarantined PMTs left.
      ZX_DEBUG_ASSERT(active_pmt_list_.is_empty());
      ZX_DEBUG_ASSERT(quarantined_pmt_count_ == 0);
      ZX_DEBUG_ASSERT(quarantined_page_count_ == 0);

      // Move our resources outside of our spinlock's scope so that they can
      // return to the heap after the lock has been dropped.
      local_smrg_list = ktl::move(smrg_list_);
      local_context_bank = ktl::move(context_bank_);
    }

    // If we have a device address space, clear mappings it may still have.
    //
    // Note: We do not need to actually perform any TLB invalidating during this
    // process.  When we shutdown all of our hardware (above) we cleared and
    // disabled the TTBRs in our context bank, and then invalidated all of our
    // TLB entries using our CB's ASID.  The TLB cache cannot have any valid
    // entries in it referring to any part of our PTEs, and there is no way for
    // hardware to establish any new TLB entries since both of our TTBRs are
    // disabled.
    //
    // TODO(johngro): Consider (as an optimization pass) changing the way that
    // FreeTranslationTables works.  Right now it is doing stuff like flushing
    // dirty PTEs to physical memory, and requiring a TLB invalidate callback.
    // Provided we make the invariant for calling FreeTranslationTables "You
    // must have disabled your HW and invalidated your TLBs before calling
    // this), we don't need to do this stuff anymore.  We can optimize the
    // method, remove the need to pass a callback, and even auto-invoke it from
    // the destructor of the object instead of needing to manually call it.
    // Then, all we need to do here, is reset our pointer.
    //
    if (aspace_ != nullptr) {
      ZX_DEBUG_ASSERT(local_context_bank != nullptr);
      auto thunk = [](void*, uint64_t, uint64_t) -> void {};
      DeviceAspace::TlbInvalOp no_op{thunk, local_context_bank.get()};
      aspace_->FreeTranslationTables(no_op);
      aspace_ = nullptr;
    }
  }

  // Now go ahead and destroy the resource bookkeeping.
  local_smrg_list.clear();
  local_context_bank.reset();
}

bool SmmuBti::SmrIntersects(SmrValue stream_ids) const {
  Guard<SpinLock, IrqSave> guard{&lock_};

  for (const StreamMatchRegGroup& smrg : smrg_list_) {
    if (smrg.stream_ids().Intersects(stream_ids)) {
      return true;
    }
  }

  return false;
}

void SmmuBti::AssertOwned(StreamMatchRegGroup& smrg) {
  // Right now, in all of the practical systems we have encountered so far, a
  // BTI's list of SMRGs is pretty much only one, or at most 2, entries long.
  // It seems reasonable to perform this O(n) validation in systems with
  // ZX_DEBUG_ASSERT implemented if N is expected to be a low as this.
  if constexpr (ZX_DEBUG_ASSERT_IMPLEMENTED) {
    for (const StreamMatchRegGroup& x : smrg_list_) {
      if (&x == &smrg) {
        return;
      }
    }
    ZX_DEBUG_ASSERT(false);
  }
}

void SmmuBti::TlbInvalThunk(void* _thiz, uint64_t base, uint64_t size) {
  ZX_DEBUG_ASSERT(_thiz != nullptr);
  SmmuBti& thiz = *reinterpret_cast<SmmuBti*>(_thiz);

  // This callback is being invoked from DeviceAspace code after it has made
  // some number of modifications of the page tables as they currently exist in
  // RAM.  Before it calls into our thunk to invalidate TLBs, it must flush the
  // entries it modified from the CPU cache out to physical memory.  It does
  // this by using an arch-agnostic calls to arch::CleanDataCacheRange for the
  // regions which need to be flushed.
  //
  // Currently, on ARM, calls to arch::CleanDataCacheRange always end with a
  // `dsb sy` instruction, which ensures that the cache flushes have all
  // finished before proceeding, instead of potentially having the instruction
  // lingering in the execution pipeline waiting to finish.  With all of the
  // changes propagated to physical memory, we should be OK to begin the process
  // of invalidating the TLB entries for the region of the device's address
  // space which were updated.

  // We should always have a context bank whenever we are being asked to
  // invalidate our TLBs by our DeviceAspace.
  Guard<SpinLock, IrqSave> guard{&thiz.lock_};
  ZX_DEBUG_ASSERT(thiz.context_bank_ != nullptr);

  // No size means flush everything.  Otherwise, just flush the requested range.
  if (!size) {
    thiz.context_bank_->TLBInvalidateByAsid();
  } else {
    thiz.context_bank_->TLBInvalidateRegion(base, size);
  }
}

zx::result<ktl::unique_ptr<DeviceAspace>> SmmuBti::CreateAspace() {
  // Allocations start at the second L3 page (index == 1 at L2) in the page
  // tables. This is 2MB when page size is 4KB.
  //
  // All of our mappings are going to be done using the page tables specified by
  // TTBR0, we are not going to be using TTBR1.  By default, given the way that
  // we configure the hardware, this means that TTBR0 will control the mappings
  // in the device address space on the range from `[0, kDefaultTTBR0AddrSpaceSz)`
  //
  // The actual range controlled by the DeviceAspace instance, however, will be
  // `[2MB, kDefaultTTBR0AddrSpaceSz)` == `[lower_aspace_bound, kDefaultTTBR0AddrSpaceSz)`
  //
  // To construct the DeviceAspace object, we need to pass the base address and
  // the size of the region to manage.  The base address is
  // `lower_aspace_bound`, but the size of the managed space ends up being
  // `kDefaultTTBR0AddrSpaceSz - lower_aspace_bound`.  Statically assert that
  // this is computable before passing the value on to DeviceAspace::Create.
  //
  static_assert(kPageSize == 4096, "SMMU Device Address Space manager assumes 4k pages");
  constexpr uint64_t lower_aspace_bound = DeviceAspace::kPageSize * 512;

  static_assert(lower_aspace_bound < ContextBank::kDefaultTTBR0AddrSpaceSz);
  constexpr uint64_t dev_aspace_size = ContextBank::kDefaultTTBR0AddrSpaceSz - lower_aspace_bound;

  return DeviceAspace::Create(lower_aspace_bound, dev_aspace_size);
}

void SmmuBti::AttachAspace(ktl::unique_ptr<DeviceAspace> aspace) {
  ZX_DEBUG_ASSERT(aspace_ == nullptr);
  ZX_DEBUG_ASSERT(context_bank_->ttbrs_[0].ttbr_paddr == 0);

  // Take ownership of the address space, and fill out its details in our TTBR0 bookkeeping.
  aspace_ = ktl::move(aspace);
  context_bank_->ttbrs_[0].granule_size_bits = aspace_->granule_size_bits();
  context_bank_->ttbrs_[0].first_valid_addr = aspace_->first_valid_address();
  context_bank_->ttbrs_[0].last_valid_addr = aspace_->last_valid_address();
  context_bank_->ttbrs_[0].ttbr_paddr = aspace_->GetRootPaddr();
}

zx::result<> SmmuBti::InitializeAspace() {
  zx::result<ktl::unique_ptr<DeviceAspace>> maybe_aspace = CreateAspace();
  if (!maybe_aspace.is_ok()) {
    return maybe_aspace.take_error();
  }

  Guard<SpinLock, IrqSave> guard{&lock_};
  AttachAspace(ktl::move(maybe_aspace.value()));
  return zx::ok();
}

void SmmuBti::AddSmrg(Smmu& smmu, ktl::unique_ptr<StreamMatchRegGroup> smrg) {
  Guard<SpinLock, IrqSave> guard{&lock_};
  AddSmrgLocked(smmu, ktl::move(smrg));
}

void SmmuBti::AddSmrgLocked(Smmu& smmu, ktl::unique_ptr<StreamMatchRegGroup> smrg) {
  // Currently, there are only three places that we ever add an SMRG to a BTI.
  //
  // 1) As we create the BTI for a user's new BtiDispatcher.
  // 2) As we adopt the SMRG from initial hardware state and create a new BTI to
  //    contain it.
  // 3) During adoption when we find a another SMRG to adopt which is in
  //    translation mode and whose CBNdx matches this BTI's currently configured
  //    context bank.
  //
  // In case #1, the SMRG we just created should have come from the pool of free
  // SMRGs, and should be in the Invalid state with an invalid context bank
  // index.
  //
  // For case #2, the SMRG may or may not have an associated context bank, but
  // if it does, the SMRG should be in translation mode, and the BTI we created
  // for the adopted SMRG should already have an adopted the ContextBank as
  // specified by the SMRG's register state.
  //
  // Finally, for case #3, the only reason the adoption code should be adding
  // another SMRG to this BTI would be because it discovered one in Translation
  // mode who's cb_ndx matches the index of this BTI's context bank.
  //
  // So, if this SMRG is in translation mode, assert that we have a context
  // bank, and the index of the context bank matches that of the adopted SMRG.
  //
  // TODO(johngro): When we get around to allowing user-mode to adding a new
  // SMRG to an existing BTI, we need make sure that the SMRG (which will be in
  // a locked down state) is reconfigured to match the BTI's current state.  IOW
  // - If the BTI is operating, the SMRG will need to be configured for
  // translation, pointed at this BTI's CB, and enabled.
  //
  if (smrg->mode() == S2CR_Type::kTranslation) {
    ZX_DEBUG_ASSERT(context_bank_ != nullptr);
    ZX_DEBUG_ASSERT(context_bank_->cb_ndx() == smrg->cb_ndx());
  }

  // The SMRG we are adding should not be marked as in-use at the SMMU level
  // (yet).
  ZX_DEBUG_ASSERT(smmu.available_smrgs_.TestBit(smrg->smrg_ndx()));

  // Everything checks out.  Add the SMRG to our list, and mark it as in use in
  // our parent SMMU.
  smmu.available_smrgs_.ClrBit(smrg->smrg_ndx());
  smrg_list_.push_back(ktl::move(smrg));
}

zx::result<> SmmuBti::SetModeLocked(BtiMode target_mode) {
  // Are we already there?
  if (target_mode == mode_) {
    return zx::ok();
  }

  // Adopted and Shutdown BTIs can never change mode.
  if ((mode_ == BtiMode::kAdopted) || (mode_ == BtiMode::kShutdown)) {
    return zx::error{ZX_ERR_BAD_STATE};
  }

  // Record our new mode.
  mode_ = target_mode;

  // If we are changing to Fault mode while our SMMU is operating in Enforced
  // mode, we are basically finished.  Access control is handled at the
  // per-address level in our address space, we don't need to do anything
  // special to protect ourselves against rogue hardware.
  if ((mode_ == BtiMode::kFault) && (smmu_->op_mode() == ArmSmmuMode::kEnforced)) {
    return zx::ok();
  }

  // We were not adopted, so we should be able to assert that we have a
  // context bank.  Do so, then configure our context bank for its new mode.
  ZX_DEBUG_ASSERT(context_bank_ != nullptr);
  context_bank_->SetMode(*this, mode_);

  for (StreamMatchRegGroup& smrg : smrg_list_) {
    // Except for when we are shutting down (and we disable access at all
    // levels), we control all of our enforcement policy through only the
    // context bank.  So, if we are shutting down, disable all of the match
    // registers, otherwise enable them and point them at our context bank.
    if (mode_ == BtiMode::kShutdown) {
      smrg.Disable(*this);
    } else {
      smrg.EnableForContextBank(*this, context_bank_->cb_ndx());
    }
  }

  return zx::ok();
}

void SmmuBti::HandleFaultLocked() {
  // TODO(johngro) currently, the only thing for us to do here is to enter
  // "Fault" mode. Some day, we'd like to define a signal that we can raise on
  // our BTI object so that user-mode is aware that this has happened.  See the
  // section "BTI Dispatchers should have a 'faulted' signal" in the top-level
  // README.md for more details.
  //
  // There should be nothing for us to do if we are already shut down.
  if (mode() != BtiMode::kShutdown) {
    // The only way this should ever be the case is if we have adopted hardware
    // whose SMRG registers didn't specify a valid context bank, but secretly
    // actually had one which was configured using virtualised register
    // techniques by the system at either EL2/sEL3.  In theory, we can promote
    // this to an ASSERT, but for now it is just a warning.
    if (context_bank_ == nullptr) {
      dprintf(INFO,
              "%s: WARNING - Context fault received, but BTI has no associated context bank.\n",
              smmu_->name());
    }

    const zx::result<> result = SetModeLocked(BtiMode::kFault);
    ktl::array<char, 128> sid_buffer{0};
    if (result.is_error()) {
      dprintf(INFO,
              "%s: WARNING - Failed to place BTI controlling StreamID(s) [%s] into "
              "fault mode (err %d).\n",
              smmu_->name(), RenderSidList(sid_buffer), result.error_value());
    } else {
      dprintf(INFO, "%s: BTI controlling StreamID(s) [%s] has entered fault mode.\n", smmu_->name(),
              RenderSidList(sid_buffer));
    }
  }
}

uint32_t SmmuBti::cb_ndx_locked() const {
  return context_bank_ ? context_bank_->cb_ndx() : ktl::numeric_limits<uint32_t>::max();
}

uint32_t SmmuBti::cb_ndx() const {
  Guard<SpinLock, IrqSave> guard{&lock_};
  return cb_ndx_locked();
}

zx::result<> SmmuBti::InvalidateSids() {
  Guard<SpinLock, IrqSave> guard{&lock_};

  // Adopted and Shutdown BTIs can never invalidate their Stream IDs.
  if ((mode_ == BtiMode::kAdopted) || (mode_ == BtiMode::kShutdown)) {
    return zx::error{ZX_ERR_BAD_STATE};
  }

  for (StreamMatchRegGroup& smrg : smrg_list_) {
    smrg.Invalidate(*this);
  }

  return zx::ok();
}

const char* SmmuBti::RenderSidList(ktl::span<char> buffer) const {
  // StringFile will stop accumulating characters and automatically truncate the
  // string if it runs out of room in |buffer|.
  StringFile f{buffer};
  bool first{true};

  for (const StreamMatchRegGroup& smrg : smrg_list_) {
    for (const uint16_t id : smrg.stream_ids()) {
      fprintf(&f, "%s0x%04hx", first ? "" : " ", id);
      first = false;
    }
  }

  return ktl::move(f).take().data();
}

void SmmuBti::ReleaseMapping(DeviceAspace::Allocation mapping) {
  if (mapping) {
    // Give the mapping back to our DeviceAspace, which will manage the process
    // of updating the translation table PTEs and flushing the CPU caches as
    // well as invalidating the TLBs (using the callback we provide).
    DeviceAspace::TlbInvalOp tlb_inval_op{TlbInvalThunk, this};
    aspace_->Unmap(ktl::move(mapping), tlb_inval_op);
  }
}

void SmmuBti::OnPmtUnpin(SmmuPmt& pmt) {
  // If there is still a PMO to unpin, move it outside of the scope of the lock
  // before doing so.
  //
  // Use a local lambda to prevent the AssertOwnerPmtLockHeld from escaping the
  // scope of the Guard.
  PinnedVmObject released_vmo = [&]() {
    // Release the mapping for this PMT, if any.  If the PMT has no mapping in
    // the device's address space (as would be the case of this BTI is operating
    // in passthru mode), then this will be a no-op.
    Guard<Mutex> pmt_guard{&pmt_lock_};
    pmt.AssertOwnerPmtLockHeld();
    ReleaseMapping(pmt.TakeMapLocation());

    Guard<SpinLock, IrqSave> guard{&lock_};
    if (pmt.state() == SmmuPmt::State::kActive) {
      ZX_DEBUG_ASSERT(pmt.InContainer());
      active_pmt_list_.erase(pmt);
      pmt.set_state(SmmuPmt::State::kReleased);
    } else {
      ZX_DEBUG_ASSERT(!pmt.InContainer());
      ZX_DEBUG_ASSERT(pmt.pinned_vmo().vmo() == nullptr);
    }

    return pmt.TakePinnedVmo();
  }();

  // Now that the lock is dropped, if we took ownership of the PMO, we can let
  // it go out of scope now, releasing the pages back to the PMM as we do.
  released_vmo.reset();
}

void SmmuBti::OnPmtZeroHandles(SmmuPmt& pmt) {
  uint64_t quarantined_page_count{0};
  uint64_t quarantined_pmt_count{0};
  bool has_active_pmts{false};
  bool was_orphaned{false};

  // If there is still a PMO to unpin, move it outside of the scope of the lock
  // before doing so.
  //
  // Use a small local lambda to prevent the AssertOwnerPmtLockHeld from
  // escaping the scope of the Guard.
  PinnedVmObject quarantined_vmo = [&]() {
    // Release the mapping for this PMT, if any.
    Guard<Mutex> pmt_guard{&pmt_lock_};
    pmt.AssertOwnerPmtLockHeld();
    ReleaseMapping(pmt.TakeMapLocation());

    Guard<SpinLock, IrqSave> guard{&lock_};
    ZX_DEBUG_ASSERT(pmt.state() != SmmuPmt::State::kInitial);
    if (pmt.state() == SmmuPmt::State::kActive) {
      // If a PMT has its last handle closed before being formally unpinned,
      // then we have leaked the PMT and will need to quarantine it.
      //
      // Start by entering fault mode.
      zx::result<> set_fault_mode_result = SetModeLocked(BtiMode::kFault);
      ASSERT(set_fault_mode_result.is_ok());

      // Remove the PMT from the active set of PMTs and flag it as quarantined.
      ZX_DEBUG_ASSERT(pmt.InContainer());
      active_pmt_list_.erase(pmt);
      pmt.set_state(SmmuPmt::State::kQuarantined);

      // Update our stats and take note of what they are so we can log an
      // appropriate warning.
      quarantined_page_count = (quarantined_page_count_ += pmt.pages());
      quarantined_pmt_count = (quarantined_pmt_count_ += 1);
    } else {
      // If this PMT was not active, then it should be in the Released state and
      // not on the active list.
      ZX_DEBUG_ASSERT(pmt.state() == SmmuPmt::State::kReleased);
      ZX_DEBUG_ASSERT(!pmt.InContainer());
      ZX_DEBUG_ASSERT(pmt.pinned_vmo().vmo() == nullptr);
    }

    has_active_pmts = !active_pmt_list_.is_empty();
    was_orphaned = orphaned_;
    return pmt.TakePinnedVmo();
  }();

  // Now that we are outside of the locks, we can handle any final cleanup
  // tasks.
  //
  // 1) If this PMT was leaked, we need to log a warning.
  // 2) If we were orphaned, and this was the last active PMT, and we either
  //    have no quarantined PMTs, or are operating in Enforced mode, then we
  //    have reached end of life and can clean up now.
  // 3) If we quarantined a pinned memory object, we can now return its pages to
  //    the PMM.  We don't need to actually keep it around, since we have either
  //    revoked access to that specific region of memory (if we are in enforced
  //    mode), or revoked the device's access to all memory (because we are in
  //    passthru mode).

  // #1 : Log a warning if needed.
  if (quarantined_vmo.vmo() != nullptr) {
    const BtiPageLeakReason reason = was_orphaned ? BtiPageLeakReason::PmtQuarantinedWhenBtiOrphaned
                                                  : BtiPageLeakReason::PmtQuarantined;
    PrintQuarantineWarning(reason, quarantined_page_count, quarantined_pmt_count);
  }

  // #2 : Shutdown if it is time.
  ZX_DEBUG_ASSERT((quarantined_page_count == 0) == (quarantined_pmt_count == 0));
  if (was_orphaned && !has_active_pmts &&
      (!quarantined_pmt_count || smmu_->op_mode() == ArmSmmuMode::kEnforced)) {
    OnEndOfLife();
  }

  // #3 : Release our VMO (if any).
  quarantined_vmo.reset();
}

}  // namespace arm_smmu
