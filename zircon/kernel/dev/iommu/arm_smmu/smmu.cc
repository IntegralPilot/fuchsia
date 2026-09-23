// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/fit/defer.h>
#include <lib/root_resource_filter.h>
#include <zircon/errors.h>
#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

#include <arch/arm64/periphmap.h>
#include <dev/arm_smmu/context_bank.h>
#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_bti.h>
#include <dev/arm_smmu/smmu_mode.h>
#include <dev/arm_smmu/smmu_registers.h>
#include <dev/arm_smmu/stream_match_reg_group.h>
#include <dev/interrupt.h>
#include <dev/iommu/stub/stub.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <vm/vm_object.h>

#include <ktl/enforce.h>

#define INVALID_PADDR UINT64_MAX

namespace arm_smmu {

namespace {
struct FaultBuffer {
  DECLARE_SPINLOCK(FaultBuffer) lock_;
  TA_GUARDED(lock_) char buffer_[1024] { 0 };
};
FaultBuffer g_fault_buffer;
}  // namespace

fbl::DoublyLinkedList<fbl::RefPtr<Smmu>> Smmu::instances_;

namespace {
ArmSmmuMode GetSmmuMode(ktl::optional<uint64_t> base_addr = ktl::nullopt) {
  const ktl::optional<ArmSmmuMode> maybe_mode =
      ::arm_smmu::GetSmmuMode(BootOptions::Get()->arm_smmu_mode.data(), base_addr);
  return maybe_mode.value_or(ArmSmmuMode::kDisabled);
}
}  // namespace

Smmu::Smmu(uint64_t phys_reg_base) : phys_reg_base_(phys_reg_base) {}
Smmu::~Smmu() {
  // By the time that we destruct, all of our interrupts should be unregistered.
  // Assert this.
  if constexpr (ZX_DEBUG_ASSERT_IMPLEMENTED) {
    for (const GlobalIrqVector& vec : global_irqs_) {
      ZX_DEBUG_ASSERT(!vec.registered);
    }

    for (const ContextIrqVector& vec : context_irqs_) {
      ZX_DEBUG_ASSERT(!vec.registered);
    }
  }
}

zx::result<fbl::RefPtr<iommu::Iommu>> Smmu::Create(const zbi_dcfg_arm_smmu_driver_t& config) {
  // It is an error to attempt to create an SMMU instances with registers which
  // have already been used.
  Guard<Mutex> guard{InstanceLock::Get()};
  if (FindInstance(config.mmio_phys) != nullptr) {
    return zx::error(ZX_ERR_ALREADY_BOUND);
  }

  // Looks like this is a new instance.  Allocate an instance, then attempt to
  // initialize it.  If we succeed, add the instance to the global list.
  fbl::AllocChecker ac;
  fbl::RefPtr<Smmu> instance = fbl::AdoptRef<Smmu>(new (&ac) Smmu(config.mmio_phys));

  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  if (const zx_status_t status = instance->Init(config); status != ZX_OK) {
    return zx::error(status);
  }

  instances_.push_back(instance);
  return zx::ok(ktl::move(instance));
}

zx::result<fbl::RefPtr<iommu::Iommu>> Smmu::Fetch(ktl::unique_ptr<const uint8_t[]> desc,
                                                  size_t desc_len) {
  // If the global SMMU mode is set to Disabled, attempt to create a stub IOMMU
  // for the user.  Any SMMUs we have in our global list are only there so we
  // can dump registers using `k` commands.
  if (GetSmmuMode() == ArmSmmuMode::kDisabled) {
    return StubIommu::Create();
  }

  // Check our parameters, then go looking for an instance which was created
  // during kernel startup which has not already been bound by user-mode.
  if ((desc.get() == nullptr) || (desc_len != sizeof(zx_iommu_desc_arm_smmu_t))) {
    dprintf(INFO, "Bad desc when fetching SMMUv2 (%p, %zu)\n", desc.get(), desc_len);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // TODO(johngro): Currently the user-mode driver framework is creating two
  // IOMMUs for systems with an SMMU.  One of them is a StubIommu instance with
  // an ID equal to the SMMU HW's base address.  The other is a real SMMU
  // instance, but it gets created using an address with the LSB set in it.
  //
  // This is not correct.  They should simply be attempting to create a single
  // SMMU using the base address specified in the device tree.  If the SMMU is
  // disabled in the kernel, we will automatically create a Stub for them.
  // Otherwise, we will give them a handle to the real SMMU hardware, provided
  // that no other handle is currently bound to it.
  zx_iommu_desc_arm_smmu_t& reg_desc =
      *reinterpret_cast<zx_iommu_desc_arm_smmu_t*>(const_cast<unsigned char*>(desc.get()));
  reg_desc.register_base &= ~uint64_t{7};

  Guard<Mutex> guard{InstanceLock::Get()};
  fbl::RefPtr<Smmu> instance = FindInstance(reg_desc.register_base);

  if (instance != nullptr) {
    // At this point, we know that the default ArmSmmuMode is _not_ disabled.
    // But, if we found the instance requested by the user instance whose mode
    // has overridden to disabled, do not return a reference to the disabled
    // instance.  Give them a new stub IOMMU instance instead.
    if (instance->op_mode() == ArmSmmuMode::kDisabled) {
      return StubIommu::Create();
    }

    Guard<Mutex> instance_guard{&instance->lock_};
    if (!instance->user_mode_bound_) {
      instance->user_mode_bound_ = true;
      return zx::ok(ktl::move(instance));
    } else {
      return zx::error(ZX_ERR_ALREADY_BOUND);
    }
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<fbl::RefPtr<iommu::Bti>> Smmu::CreateBti(uint64_t bus_txn_id) {
  // Start by making sure the mask/id being passed to us is supported.
  if (bus_txn_id & ~uint64_t{valid_stream_id_mask_}) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Next, make sure that this mask/id does not collide with any Stream IDs
  // owned by existing BTIs.
  const SmrValue stream_ids{static_cast<uint32_t>(bus_txn_id), valid_stream_id_mask_};
  Guard<Mutex> guard{&lock_};

  for (const SmmuBti& bti : bti_list_) {
    if (bti.SmrIntersects(stream_ids)) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
  }

  // Great!  It looks like this is a new set of stream IDs.  We'll need to
  // allocate an SMRG and a ContextBank in order to proceed.  See if we have any
  // to allocate.
  const ktl::optional<uint32_t> smrg_ndx = available_smrgs_.FindFirstSetBit();
  if (!smrg_ndx.has_value()) {
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  const ktl::optional<uint32_t> cb_ndx = available_cbs_.FindFirstSetBit();
  if (!cb_ndx.has_value()) {
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  // Looks like we have the resources available, construct objects to manage them.
  zx::result<ktl::unique_ptr<StreamMatchRegGroup>> smrg =
      StreamMatchRegGroup::CreateAndLockdown(*this, *smrg_ndx, stream_ids);
  if (smrg.is_error()) {
    return smrg.take_error();
  }

  zx::result<ktl::unique_ptr<ContextBank>> cb = ContextBank::CreateAndLockdown(*this, *cb_ndx);
  if (cb.is_error()) {
    return cb.take_error();
  }

  // Almost there.  Now we just need to construct a BTI, and activate it
  // according to our current SMMU mode of operation (either passthru, or
  // enforced).
  //
  // If our SMMU driver is set to disabled, no one should be attempting to
  // allocate any BTIs here. They should be using StubIommus instead.
  ZX_DEBUG_ASSERT_MSG((op_mode_ == ArmSmmuMode::kPassthru) || (op_mode_ == ArmSmmuMode::kEnforced),
                      "SMMU must be passthru or enforced when creating a BTI (%u)",
                      static_cast<uint32_t>(op_mode_));
  BtiMode bti_mode =
      (op_mode_ == ArmSmmuMode::kPassthru) ? BtiMode::kBypass : BtiMode::kTranslation;
  fbl::RefPtr<SmmuBti> bti = SmmuBti::Create(*this, ktl::move(*smrg), ktl::move(*cb), bti_mode);
  if (bti == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // Victory!  Add ourselves to the list of active BTIs, and return a reference
  // for the BTI dispatcher to hold on to.
  bti_list_.push_back(bti);
  return zx::ok(ktl::move(bti));
}

zx_status_t Smmu::Init(const zbi_dcfg_arm_smmu_driver_t& config) {
  Guard<Mutex> guard{&lock_};

  // First, find our register bank in the peripheral map and stash our virtual
  // address to the registers.  Then initialize our MMIO base for our global
  // registers.
  //
  // TODO(johngro): this code currently assumes that if the first address
  // exists, that all of the SMMU pages exist in the peripheral map.  Some day
  // we should:
  // 1) Extend the peripheral map API to allow us to translate an address while
  //    simultaneously confirming that at least X bytes exist in the map after
  //    the translated address.
  // 2) Start by asking for a single page.  Use the ID registers to determine
  //    the total size of the SMMU register bank.
  // 3) Then compute how many context banks are available to us, and have not
  //    been hidden by the secure side of things.
  // 4) Finally, verify that we can access the subset of register pages that we
  //    are supposed to be able to access.
  const vaddr_t reg_vaddr = periph_paddr_to_vaddr(phys_reg_base_);
  if (!reg_vaddr) {
    return ZX_ERR_NO_RESOURCES;
  }
  gr0_base_ = hwreg::RegisterMmio{reinterpret_cast<volatile void*>(reg_vaddr)};

  // Read and cache our ID registers.  Check the version number to be sure we
  // recognize the hardware.  Stash our initial count of context banks and
  // compute our friendly name as we go.
  idr0_ = gr0::IDR0::Get().ReadFrom(&gr0_base_);
  idr1_ = gr0::IDR1::Get().ReadFrom(&gr0_base_);
  idr2_ = gr0::IDR2::Get().ReadFrom(&gr0_base_);
  idr7_ = gr0::IDR7::Get().ReadFrom(&gr0_base_);
  snprintf(name_, sizeof(name_), "SMMU@0x%08lx", phys_reg_base_);

  if (idr7_.major() != 2) {
    dprintf(INFO, "%s (v%u.%u) is not supported\n", name(), idr7_.major(), idr7_.minor());
    return ZX_ERR_INVALID_ARGS;
  } else {
    dprintf(INFO, "Found %s (v%u.%u)\n", name(), idr7_.major(), idr7_.minor());
  }

  // Figure out how many context banks and SMRs we are allowed to use.  This is
  // based on how many are still available to us after the secure side of things
  // has taken theirs, as well as the artificial limit which might be passed to
  // us by configuration.
  num_cbs_ = idr1_.NUMCB();
  if (config.num_context_banks_override && (config.num_context_banks_override < num_cbs_)) {
    num_cbs_ = config.num_context_banks_override;
  }

  num_smrgs_ = idr0_.NUMSMRG();
  if (config.num_smr_override && (config.num_smr_override < num_smrgs_)) {
    num_smrgs_ = config.num_smr_override;
  }

  // Compute the base address of the other register banks we plan to use.  See
  // Section 8.1 for the details of the register address space.
  //
  const size_t reg_size = reg_page_size() * reg_page_cnt();
  gr1_base_ = hwreg::RegisterMmio{reinterpret_cast<volatile void*>(reg_vaddr + reg_page_size())};
  cb0_base_ = hwreg::RegisterMmio{reinterpret_cast<volatile void*>(reg_vaddr + reg_size)};

  // Figure out what our operating mode should be based on our address and the
  // Zircon boot option.
  op_mode_ = GetSmmuMode(phys_reg_base_);

  // If "Enforced" has been selected, but this SMMU instance does not support
  // translation, we will need to downgrade to Bypass mode.
  if ((op_mode_ == ArmSmmuMode::kEnforced) && !idr0_.S1TS()) {
    dprintf(INFO,
            "WARNING - %s does not support Stage 1 Translation, but fully enforced operation was "
            "requested.  Downgrading to passthru mode.\n",
            name());
    op_mode_ = ArmSmmuMode::kPassthru;
  }

  // This code currently depends on stream-matching support and does not support
  // the older stream indexing mode.
  if (idr0_.SMS() == 0) {
    dprintf(INFO, "ERROR - %s does not support stream-matching (IDR0 0x%08x)\n", name(),
            idr0_.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // This code does not currently support extended stream IDs.  When extended
  // stream IDs are supported and enabled, the match registers' ID and MASK
  // fields move from being 15 bits, to 16 bits, and the location of the "valid"
  // bit moves from the SMR (stream match register) to the S2CB (stream to
  // context bank) register.  Currently, if the feature is enabled and supported
  // when we come out of the bootloader, we _assume_ that it is required to
  // support the stream ID space used by the SOC, so we refuse to load.
  //
  // Support for this feature is not a huge lift, it just requires abstracting
  // the SMR/S2CB interface just a little bit.
  //
  // See section 2.3.1 Transaction streams : StreamID size
  //
  gr0::CR0 cr0 = gr0::CR0::Get().ReadFrom(&gr0_base_);
  if (idr0_.EXIDS() && cr0.EXIDENABLE()) {
    dprintf(INFO, "ERROR - %s does not support extended StreamIDs (IDR0 0x%08x, CR0 0x%08x)\n",
            name(), idr0_.reg_value(), cr0.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // In SMMUv2, IDR0.NUMIRPT == 1 means that we have a dedicated interrupt pin
  // for every context bank.  If the value is > 1, then there are a pool of
  // interrupts which need to be shared across the various context banks, and
  // the CBAR.IRPTNDX field will need to be programmed to map specific context
  // banks to specific interrupts.  Currently, we only support the dedicated
  // interrupt model, so make sure that is how this SMMU is configured.
  //
  // See section 3.6.2 Context Bank Interrupts.
  //
  if (idr0_.NUMIRPT() != 1) {
    dprintf(INFO,
            "ERROR - %s SMMU does not have a dedicated interrupt per context bank. "
            "(IDR0 0x%08x)\n",
            name(), idr0_.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Use the number of stream ID bits as reported in IDR0 to compute a mask
  // which can tell us if an encoded stream ID ((mask << 16 | id) is valid or
  // not.
  valid_stream_id_mask_ = (1u << idr0_.NUMSIDB()) - 1;
  valid_stream_id_mask_ |= (valid_stream_id_mask_ << 16);

  // Similar to extended StreamIDs, we do not currently support the "Extended
  // Stream Matching" extension, which increases the total number of stream
  // match registers from 128 up to as many as 1024.  Adding support would
  // involve changes to the logic of finding and allocating stream match
  // contexts, as well as some additional save/restore code to support SMMU low
  // power states.  See Chapter 14 : Extended Stream Matching Extension.
  gr0::CR2 cr2 = gr0::CR2::Get().ReadFrom(&gr0_base_);
  if (idr0_.EXSMRGS() && cr2.EXSMRGENABLE()) {
    dprintf(INFO, "%s does not support extended stream matching (IDR0 0x%08x, CR2 0x%08x)\n",
            name(), idr0_.reg_value(), cr2.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Allocate storage for our global and context interrupts based on the values
  // passed to us, and make copies of the configuration.
  ZX_DEBUG_ASSERT(config.irq_cnt <= ktl::size(config.irqs));
  ZX_DEBUG_ASSERT(config.global_irq_cnt <= config.irq_cnt);
  const auto all_irqs = ktl::span<const zbi_dcfg_arm_smmu_irq_t>{config.irqs, config.irq_cnt};
  const auto global_irqs = all_irqs.subspan(0, config.global_irq_cnt);
  const auto context_irqs =
      all_irqs.subspan(config.global_irq_cnt, config.irq_cnt - config.global_irq_cnt);

  if (const zx_status_t status = AllocateIrqs<GlobalIrqVector>(global_irqs); status != ZX_OK) {
    return status;
  }

  if (const zx_status_t status = AllocateIrqs<ContextIrqVector>(context_irqs); status != ZX_OK) {
    return status;
  }

  // Set up our "available" bit masks.  At this point, all SMRGs and CBs in the
  // system are considered to be "available", however right after this we might
  // end up reserving some of them because they were flagged as "Early init"
  // contexts.
  available_smrgs_.SetLowestNBits(num_smrgs());
  available_cbs_.SetLowestNBits(num_cbs());

  // If our mode is "disabled", don't go any further.  We don't want to actually
  // claim any hardware, or change any of the existing HW configuration.
  if (op_mode_ == ArmSmmuMode::kDisabled) {
    return ZX_OK;
  }

  // Reserve our registers so that they cannot be accessed by user mode.
  root_resource_filter_add_deny_region(phys_reg_base_, reg_size << 1, ZX_RSRC_KIND_MMIO);

  // If we are not disabled, make sure our configuration registers are set up as
  // we want them to be.
  if (op_mode_ != ArmSmmuMode::kDisabled) {
    cr0.set_VMID16EN(0)     // 8-bit VMIDs
        .set_SMCFCFG(1)     // Raise a fault when there are stream-match conflicts.
        .set_FB(0)          // Disable force broadcast of TLB/cache maintenance ops.
        .set_PTM(1)         // Private TLB maintenance.  Don't listen to system-wide broadcasts.
        .set_VMIDPNE(1)     // Private VMID namespaces
        .set_USFCFG(1)      // Raise a fault on unidentified stream access.
        .set_GSE(0)         // Disable global stalling across contexts.
        .set_STALLD(1)      // Disable per-context stalling.
        .set_GCFGFIE(0)     // Disable global config fault interrupts (enabled below)
        .set_GCFGFRE(1)     // Abort transactions during global config faults (not RAZ/WI).
        .set_EXIDENABLE(0)  // Do not use extended IDs (we checked this above)
        .set_GFIE(0)        // Disable global fault interrupts (enabled below)
        .set_GFRE(1)        // Abort transactions during global faults (not RAZ/WI).
        .set_CLIENTPD(0)    // Accesses are subject to translation, access ctrl, and attr-gen.
        .WriteTo(&gr0_base_);

    cr2.set_EXSMRGENABLE(0)      // Extended SMRGs are disabled.
        .set_COMPINDEXENABLE(0)  // Disabled compressed StreamID matching (eg; use the SMRs)
        .WriteTo(&gr0_base_);
  }

  // Register all of our interrupts.  If anything goes wrong after this, be sure
  // to give them all back.
  auto cleanup_irqs = fit::defer([&]() {
    lock_.lock().AssertHeld();
    UnregisterInterrupts();
  });

  if (const zx_status_t status = RegisterIrqs<GlobalIrqVector>(); status != ZX_OK) {
    return status;
  }
  if (const zx_status_t status = RegisterIrqs<ContextIrqVector>(); status != ZX_OK) {
    return status;
  }

  // Go over the list of "handoff stream IDs" passed to us and adopt all of the
  // associated stream match register groups and context banks so that they are
  // not automatically locked down, or claimed by user-mode.
  const auto handoff_stream_ids =
      ktl::span<const uint32_t>(config.handoff_smrs, config.handoff_smr_cnt);
  for (const uint32_t sid : handoff_stream_ids) {
    if (sid & ~valid_stream_id_mask_) {
      dprintf(INFO, "WARNING: %s is ignoring invalid early init stream ID 0x%08x\n", name(), sid);
      continue;
    }

    SmrValue early_stream_ids(sid, valid_stream_id_mask_);

    for (uint32_t i = 0; i < num_smrgs(); ++i) {
      const gr0::SMR smr = gr0::SMR::Get(i).ReadFrom(&gr0_base_);

      // When attempting to decide whether or not to adopt an SMRG, we compute
      // the intersection of the SMR mask/value with each member of the
      // early-init vector passed to us by config.  Theoretically, this lets a
      // configuration specify multiple SIDs to adopt using only a single entry.
      //
      // That said, there are many possible misconfiguration for which proper
      // behavior is not well defined.  For example, if pre-existing hardware
      // has a set of SID `[0x1, 0x3]`, and the global set of SIDs to adopt
      // includes `[0x1, 0x4, 0x5]`, then we have encountered an SMRG where one
      // of its SIDs is supposed to be adopted, and another is not.
      //
      // For now, we do our best with our intersection test.  There is no
      // particularly clear guidance on how this is formally supposed to work.
      // Furthermore, we assume that if there is something incorrect about this
      // approach, that it will become apparent early in platform bringup, and
      // someone can come back here to tweak the code to handle the newly
      // discovered quirk.
      SmrValue reg_stream_ids(smr.reg_value(), valid_stream_id_mask_);
      if (smr.VALID() && early_stream_ids.Intersects(reg_stream_ids)) {
        const zx_status_t result = AdoptSmrg(i, reg_stream_ids);

        if (result == ZX_OK) {
          dprintf(INFO,
                  "%s : Locked SMRG %u (MASK 0x%04x, ID 0x%04x) which intersects early-init "
                  "stream ID (MASK 0x%04x, ID 0x%04x)\n",
                  name(), i, smr.MASK(), smr.ID(), early_stream_ids.mask(), early_stream_ids.id());
        } else {
          dprintf(INFO, "%s : ERROR - Failed to lock SMRG %u (MASK 0x%04x, ID 0x%04x) (err = %d)\n",
                  name(), i, smr.MASK(), smr.ID(), result);
        }
      }
    }
  }

  // Now, lock-down any contents which were not adopted.
  Lockdown();

  // If the number of context banks we have is larger than the number of
  // context bank interrupts we were given, artificially limit the number of
  // context banks we will make available and use.  We don't want to use any
  // context banks that we don't have interrupts for.
  if (num_cbs() > context_irqs.size()) {
    for (uint32_t i = static_cast<uint32_t>(context_irqs.size()); i < num_cbs(); ++i) {
      available_cbs_.ClrBit(i);
    }
    num_cbs_ = static_cast<uint32_t>(context_irqs.size());
  }

  // Now that everything has been either locked down or adopted, globally
  // invalidate all of the non-secure, non-hypervisor TLB entries, providing us
  // a well define starting state for the TLB cache.
  InvalidateAllTLBEntries();

  // Finally, enable global interrupts.
  Guard<SpinLock, IrqSave> irq_guard{&irq_lock_};

  for (const GlobalIrqVector& vec : global_irqs_) {
    if (vec.registered) {
      if (const zx_status_t status = unmask_interrupt(vec.irq_def.num); status != ZX_OK) {
        dprintf(INFO, "%s : Failed to unmask global IRQ 0x%02x\n", name(), vec.irq_def.num);
        return status;
      }
    }
  }

  cr0.set_GCFGFIE(1).set_GFIE(1).WriteTo(&gr0_base_);

  // Success.  Cancel any cleanup operations and we are done.
  cleanup_irqs.cancel();
  return ZX_OK;
}

template <typename VectorType>
zx_status_t Smmu::AllocateIrqs(const ktl::span<const zbi_dcfg_arm_smmu_irq_t>& src) {
  static_assert(ktl::is_same_v<VectorType, GlobalIrqVector> ||
                ktl::is_same_v<VectorType, ContextIrqVector>);
  constexpr bool kIsGlobal = ktl::is_same_v<VectorType, GlobalIrqVector>;

  if (src.size()) {
    // Allocate our storage.  If we are allocating context bank interrupts, and
    // the number of interrupts we were given exceed the number of context banks
    // reported by the IDRs, limit the number of interrupts we take control of
    // to the number of context banks we see.  We don't want to be claiming
    // interrupts for context banks which have already been claimed by the
    // Secure Monitor at this point.
    fbl::AllocChecker ac;
    ktl::unique_ptr<VectorType[]>& dst_storage = [this]() -> auto& {
      if constexpr (kIsGlobal) {
        return global_irq_storage_;
      } else {
        return context_irq_storage_;
      }
    }();

    uint32_t num_vectors = static_cast<uint32_t>(src.size());
    if constexpr (!kIsGlobal) {
      num_vectors = ktl::min(num_vectors, idr1_.NUMCB());
    }
    dst_storage = ktl::make_unique<VectorType[]>(&ac, num_vectors);

    if (!ac.check()) {
      dprintf(INFO, "%s : Failed to allocate storage for %u %s IRQs\n", name(), num_vectors,
              kIsGlobal ? "Global" : "Context");
      return ZX_ERR_NO_MEMORY;
    }

    // Now, obtain the IRQ lock and, copy our configuration over to our internal bookkeeping.
    {
      Guard<SpinLock, IrqSave> guard{&irq_lock_};

      ktl::span<VectorType>& dst = [this]() -> auto& {
        if constexpr (kIsGlobal) {
          return global_irqs_;
        } else {
          return context_irqs_;
        }
      }();

      dst = ktl::span<VectorType>{dst_storage.get(), num_vectors};
      for (uint32_t i = 0; i < num_vectors; ++i) {
        // The members of the IrqDef contained in our VectorType instance are
        // deliberately const, to make it difficult to accidentally change them
        // once we start operating.  Using a placement new here allows us to
        // cheat just a little bit, and re-initialize the member of the
        // structures we just default initialized even though we (in theory)
        // should not be allowed to do so.
        new (&dst[i].irq_def) IrqDef{src[i]};
        dst[i].ndx = static_cast<uint16_t>(i);
        dst[i].smmu = this;
      }
    }
  }

  return ZX_OK;
}

template <typename VectorType>
zx_status_t Smmu::RegisterIrqs() {
  static_assert(ktl::is_same_v<VectorType, GlobalIrqVector> ||
                ktl::is_same_v<VectorType, ContextIrqVector>);
  constexpr bool kIsGlobal = ktl::is_same_v<VectorType, GlobalIrqVector>;

  Guard<SpinLock, IrqSave> guard{&irq_lock_};

  ktl::span<VectorType>& irqs = [this]() -> auto& {
    if constexpr (kIsGlobal) {
      return global_irqs_;
    } else {
      return context_irqs_;
    }
  }();

  for (VectorType& v : irqs) {
    // Flags for interrupts as described in a device tree should have only one
    // mode specified (why they did this as a bitmask instead of an enumeration, I
    // have no idea).  Verify this before proceeding.
    if (!v.irq_def.valid()) {
      dprintf(INFO, "%s : Invalid IRQ flags (0x%02x) for %s IRQ num 0x%02x\n", name(),
              v.irq_def.flags, kIsGlobal ? "Global" : "Context", v.irq_def.num);
      return ZX_ERR_INVALID_ARGS;
    }

    zx_status_t status =
        configure_interrupt(v.irq_def.num, v.irq_def.trigger_mode(), v.irq_def.polarity());
    if (status != ZX_OK) {
      dprintf(INFO, "%s : Failed to configure %s IRQ num 0x%02x (flags 0x%02x, err %d).\n", name(),
              kIsGlobal ? "Global" : "Context", v.irq_def.num, v.irq_def.flags, status);
      return status;
    }

    // Make absolutely sure that our interrupt is masked.
    //
    // TODO(johngro): Confirm whether or not we actually need to do this.  Seems
    // like IRQs with no registered handler should always be masked already.
    status = mask_interrupt(v.irq_def.num);
    if (status != ZX_OK) {
      dprintf(INFO, "%s : Failed to mask %s IRQ num 0x%02x (flags 0x%02x, err %d).\n", name(),
              kIsGlobal ? "Global" : "Context", v.irq_def.num, v.irq_def.flags, status);
      return status;
    }

    // Drop the `irq_lock` before registering our interrupt handlers.
    //
    // The interrupt drivers have a central lock (call it `A`) which is held
    // during calls to register/unregister, and also during dispatch before
    // calling the registered callback.  During our registered callback, we
    // obtain `irq_lock` (after `A`) as part of dispatching.
    //
    // We are currently holding `irq_lock`, and are going to attempt to acquire
    // `A` as part of registration.  If we fail to drop `irq_lock` here before
    // registering, we set up a theoretical A/B, B/A style deadlock.
    //
    // In practice, this cannot actually happen since we masked the interrupt
    // before this, so it cannot fire, and it cannot attempt to acquire
    // `irq_lock`.  That said, runtime tools like `lockdep` could see the
    // _potential_ for deadlock later on once we are operational if we fail to
    // rigorously follow the lock ordering rules at all times.
    //
    guard.CallUnlocked([&, num = v.irq_def.num, v_ptr = &v]() {
      if constexpr (kIsGlobal) {
        status = register_int_handler(
            num, interrupt_handler_t{v_ptr, [](void* cookie) {
                                       auto* vec = static_cast<GlobalIrqVector*>(cookie);
                                       vec->smmu->HandleGlobalIrq(vec->ndx);
                                     }});
      } else {
        status = register_int_handler(
            num, interrupt_handler_t{v_ptr, [](void* cookie) {
                                       auto* vec = static_cast<ContextIrqVector*>(cookie);
                                       vec->smmu->HandleContextIrq(vec->ndx);
                                     }});
      }
    });

    if (status != ZX_OK) {
      dprintf(INFO, "%s : Failed to register %s IRQ num 0x%02x (flags 0x%02x, err %d).\n", name(),
              kIsGlobal ? "Global" : "Context", v.irq_def.num, v.irq_def.flags, status);
      return status;
    }

    v.registered = true;
  }

  return ZX_OK;
}

void Smmu::UnregisterInterrupts() {
  auto Deactivate = [this]<typename VectorType>(ktl::span<VectorType>& irqs,
                                                Guard<SpinLock, IrqSave>& guard) TA_REQ(irq_lock_) {
    static_assert(ktl::is_same_v<VectorType, GlobalIrqVector> ||
                  ktl::is_same_v<VectorType, ContextIrqVector>);
    constexpr bool kIsGlobal = ktl::is_same_v<VectorType, GlobalIrqVector>;

    for (VectorType& v : irqs) {
      if (v.registered) {
        // Unregister our interrupt, but do it with the irq_lock_ dropped.  Why
        // do we do this and why is this safe?
        //
        // When interrupts are dispatched, there is currently a global lock (the
        // `pdev_lock`) which is held for the duration of the dispatch of any
        // non-permanent interrupt handler.  This lock is also held during
        // registration/unregistration of an interrupt.
        //
        // Holding the lock in these two places provides a guarantee that after
        // calling `unregister_int_handler(num)`, we can be certain that we are
        // not racing with any interrupts in flight, and it is now safe to
        // destroy any resources that an IRQ handler might be targeting.
        //
        // But, it also implies that we have lock ordering rules to follow.  We
        // hold the irq_lock_ both while we dispatch interrupts, as well as when
        // we modify our internal interrupt state (registering/un-registering
        // interrupts).  The IRQ handler is going to obtain the pdev_lock, and
        // then attempt to hold our irq_lock_.  If we hold the irq_lock_ while
        // walking our table, and then call `unregister` with the lock held, it
        // will attempt to hold the pdev_lock and we will have set up the
        // potential for deadlock.
        //
        // So, we drop the lock while we unregister.  Why is this ok?  There are
        // only two types of places that we hold the irq_lock in this code.
        // During dispatch, where we hold only the irq_lock_, and when
        // creating/destroying associations between context interrupts and
        // BtiContexts, or when re-enabling a context bank interrupt after a
        // fault.  In each of these situations, we also hold the top level
        // `lock_` mutex.
        //
        // The IRQ handler is not going to mutate our interrupt tables, so it is
        // fine if there is an IRQ in flight for us to drop the lock here and
        // un-register the interrupt.  Doing so synchronizes  with the interrupt
        // dispatch, meaning it is safe to re-lock and modify the table after we
        // have un-registered.  For all Bti related operations, holding the
        // main `lock_` during UnregisterInterrupts is what protects us.
        //
        // TODO(johngro): consider modeling this more formally using a token and
        // a two-lock AssertHeld pattern.  Gaining exclusive access to the token
        // requires holding both `lock_` and `irq_lock_`, while gaining shared
        // access requires holding only one of the two.
        zx_status_t status;
        guard.CallUnlocked(
            [num = v.irq_def.num, &status]() { status = unregister_int_handler(num); });

        ASSERT_MSG(status == ZX_OK,
                   "%s : Failed to unregister %s IRQ num 0x%02x (flags 0x%02x, err %d) during "
                   "shutdown.\n",
                   name(), kIsGlobal ? "Global" : "Context", v.irq_def.num, v.irq_def.flags,
                   status);

        v.registered = false;
      }
    }
  };

  Guard<SpinLock, IrqSave> guard{&irq_lock_};
  Deactivate(global_irqs_, guard);
  Deactivate(context_irqs_, guard);
}

void Smmu::HandleGlobalIrq(uint32_t ndx) {
  {
    // Mask the top level interrupt to prevent an unknown device from spamming
    // the global IRQ handler with interrupts.
    //
    // TODO(johngro): Figure out a better approach to this.
    //
    // 1) If there is a device we have never created a BTI for which is getting
    //    blocked by us, we'd like to know about it, but we need a way to stop
    //    the device from spamming the global IRQ handler with faults.  Look
    //    into either force-stalling the device, or perhaps disabling global
    //    _fault_ interrupts (but not global config error interrupts) and
    //    re-enabling them later on using a timer or a DPC.
    // 2) If we receive a global config interrupt, consider panic'ing.  There is
    //    no good reason we should ever have a bad configuration, and we should
    //    consider this to be a bug.
    Guard<SpinLock, IrqSave> guard{&irq_lock_};
    ZX_DEBUG_ASSERT(ndx < global_irqs_.size());
    mask_interrupt(global_irqs_[ndx].irq_def.num);
  }

  // Start by reading the global fault status register to see what caused the
  // interrupt.  A value of zero means that we have processed everything and
  // are finished.
  gr0::GFSR gfsr = gr0::GFSR::Get().ReadFrom(&gr0_base_);
  if (gfsr.reg_value() == 0) {
    return;
  }

  // If the MULTI bit is set, it means that there was at least one more fault
  // which occurred before we were able to service the first fault.  When this
  // happens, the MULTI bit is set, but no other details of the fault are
  // recorded in order to preserve the values in the fault registers which
  // describe the first fault.
  if (gfsr.MULTI()) {
    dprintf(INFO,
            "%s : FAULT - At least one global fault occurred while an existing fault was pending.  "
            "Details of subsequent faults have been lost.\n",
            name());
  }

  constexpr ktl::array kFaultNames = {
      "Invalid context",                   // bit 0, ICF
      "Unidentified stream",               // bit 1, USF
      "Stream match conflict",             // bit 2, SMCF
      "Unimplemented context bank",        // bit 3, UCBF
      "Unimplemented context interrupt",   // bit 4, UCIF
      "Configuration access",              // bit 5, CAF
      "External",                          // bit 6, EF
      "Permission",                        // bit 7, PF
      "Unsupported upstream transaction",  // bit 8, UUT
  };

  const gr0::GFAR gfar = gr0::GFAR::Get().ReadFrom(&gr0_base_);
  const gr0::GFSYNR0 gfsynr0 = gr0::GFSYNR0::Get().ReadFrom(&gr0_base_);
  const gr0::GFSYNR1 gfsynr1 = gr0::GFSYNR1::Get().ReadFrom(&gr0_base_);

  const uint32_t fault_ndx = ktl::countr_zero(gfsr.reg_value());
  const char* const fault_name =
      fault_ndx < kFaultNames.size() ? kFaultNames[fault_ndx] : "Unknown";
  dprintf(INFO, "%s: A global \"%s\" fault has occurred. (GFSR = 0x%08x)\n", name(), fault_name,
          gfsr.reg_value());
  // clang-format off
  dprintf(INFO, "%s: Fault Address : 0x%016lx\n", name(), gfar.FADDR());
  dprintf(INFO, "%s: Stream ID     : 0x%04x\n", name(), gfsynr1.StreamID());
  dprintf(INFO, "%s: %s%s %s %s %sfault (GFSYNR0 = 0x%08x)\n", name(),
          gfsynr0.Nested() ? "Nested, " : "",
          gfsynr0.PNU() ? "Privileged" : "Non-Privileged",
          gfsynr0.IND() ? "instruction" : "data",
          gfsynr0.WNR() ? "write" : "read",
          gfsynr0.ATS() ? "address translation " : "",
          gfsynr0.reg_value());
  // clang-format on

  // Zero all of the fault-detail registers, then ack the fault(s) in the status
  // register (GFSR) by writing back the values we read from it at the start of
  // processing.  See section 9.6.15 which says "A value of 1 written to any
  // non-reserved bit clears that bit".
  gr0::GFAR::Get().FromValue(0).WriteTo(&gr0_base_);
  gr0::GFSYNR0::Get().FromValue(0).WriteTo(&gr0_base_);
  gr0::GFSYNR0::Get().FromValue(0).WriteTo(&gr0_base_);
  gfsr.WriteTo(&gr0_base_);
}

void Smmu::HandleContextIrq(uint32_t cb_ndx) {
  fbl::RefPtr<SmmuBti> target;
  uint32_t irq_num;

  ZX_DEBUG_ASSERT(cb_ndx < num_cbs());

  {
    Guard<SpinLock, IrqSave> guard{&irq_lock_};
    ZX_DEBUG_ASSERT(cb_ndx < context_irqs_.size());
    ContextIrqVector& vec = context_irqs_[cb_ndx];

    // Attempt to take a reference to our target BTI before we drop the IRQ lock
    // and dispatch the interrupt.
    irq_num = vec.irq_def.num;
    target = vec.bti;
    mask_interrupt(irq_num);
    if (target) {
      vec.in_flight = true;
      vec.enabled = false;
    }
  }

  // No matter what, remember to clear our "in-flight" flag when we are done
  // handling this IRQ.
  auto clear_in_flight = fit::defer([this, cb_ndx]() {
    Guard<SpinLock, IrqSave> guard{&irq_lock_};
    ContextIrqVector& vec = context_irqs_[cb_ndx];
    ZX_DEBUG_ASSERT(vec.in_flight == true);
    vec.in_flight = false;
  });

  if (target == nullptr) {
    // This is very strange, but it is technically possible to receive a CB
    // interrupt with no target if we happen to be shutting down our target
    // right as one last fault interrupt arrives.  In a situation like this, we
    // (the SMMU instance) _technically_ own these context bank fault syndrome
    // registers, so it should be OK to access the registers without holding a
    // BTI's lock.
    dprintf(INFO,
            "%s : WARNING - received context IRQ (0x%02x) for context bank %u, "
            "but no BTI is registered.\n",
            name(), irq_num, cb_ndx);
    PrintCommonContextIrqFaultDetails(cb_ndx);
  } else {
    // Enter our target's lock, print our fault details, then inform our BTI of
    // the fault so it can go into lockdown mode and (someday) raise a signal on
    // the kernel object indicating that there was a fault.
    //
    // The only way to restore functionality at this point is for the driver to
    // take control of its hardware, and release the quarantine state on the BTI.
    Guard<SpinLock, IrqSave> guard{&target->lock_};

    char bti_name[ZX_MAX_NAME_LEN];
    target->name_.get(ktl::size(bti_name), bti_name);
    PrintCommonContextIrqFaultDetails(cb_ndx, bti_name);
    target->HandleFaultLocked();
  }
}

void Smmu::PrintCommonContextIrqFaultDetails(uint32_t cb_ndx, const char* bti_name) {
  // Unconditionally disable the context bank interrupt at the context bank
  // level.
  hwreg::RegisterMmio cb_base = get_cb_base(cb_ndx);
  s1cbr::SCTLR::Get().ReadFrom(&cb_base).set_CFIE(0).WriteTo(&cb_base);

  // Next attempt to log the relevant details of the fault, even if there is no
  // user-mode BTI associated with this context bank.
  s1cbr::FSR fsr = s1cbr::FSR::Get().ReadFrom(&cb_base);
  const s1cbr::FAR far = s1cbr::FAR::Get().ReadFrom(&cb_base);
  const s1cbr::FSYNR0 fsynr0 = s1cbr::FSYNR0::Get().ReadFrom(&cb_base);
  const gr1::CBFRSYNRA cbfrsynra = gr1::CBFRSYNRA::Get(cb_ndx).ReadFrom(&gr1_base_);

  constexpr ktl::array kFaultNames = {
      "Unknown (reserved)",                // bit 0, Reserved
      "Translation",                       // bit 1, TF
      "Access flag",                       // bit 2, AFF
      "Permission",                        // bit 3, PF,
      "External",                          // bit 4, EF
      "TLB match conflict",                // bit 5, TLBMCF
      "TLB lock",                          // bit 6, TLBLKF
      "Address size",                      // bit 7, ASF
      "Unsupported upstream transaction",  // bit 8, UUT
  };
  const uint32_t fault_ndx = ktl::countr_zero(fsr.reg_value());
  const char* const fault_name =
      fault_ndx < kFaultNames.size() ? kFaultNames[fault_ndx] : "Unknown";

  {
    // Render our string into the global fault buffer while holding the global
    // fault buffer lock.  Sharing a global fault buffer allows us to render the
    // string without needing a large stack allocation at hard IRQ time.
    //
    // It is assumed that systems will not be taking large numbers of concurrent
    // SMMU faults, meaning that this lock should never be anything resembling a
    // significant choke point.
    Guard<SpinLock, IrqSave> guard{&g_fault_buffer.lock_};
    StringFile f{g_fault_buffer.buffer_};

    // clang-format off
    fprintf(&f, "%s \"%s\" fault has occurred in context bank %u%s. (FSR = 0x%08x)\n",
            fsr.MULTI() ? "More than one" : "A",
            fault_name,
            cb_ndx,
            fsr.SS() ? ", and the context is now stalled" : "",
            fsr.reg_value());
    // clang-format on
    fprintf(&f, "SMMU              : %s\n", name());
    fprintf(&f, "BTI Name          : %s\n", bti_name ? bti_name : "<unknown>");
    fprintf(&f, "Stream ID         : 0x%x\n", cbfrsynra.StreamID());
    fprintf(&f, "Fault Address     : 0x%016lx\n", far.FADDR());
    fprintf(&f, "Stalled           : %s\n", fsr.SS() ? "yes" : "no");
    fprintf(&f, "Permission Fault  : %s\n", fsr.PF() ? "yes" : "no");
    fprintf(&f, "Access Fault      : %s\n", fsr.AFF() ? "yes" : "no");
    fprintf(&f, "Translation Fault : %s\n", fsr.TF() ? "yes" : "no");
    constexpr uint32_t kOtherFaultMask = 0x1F0;
    if (fsr.reg_value() & kOtherFaultMask) {
      fprintf(&f, "Other faults present (FSR = 0x%08x)\n", fsr.reg_value());
    }

    if (fsr.PF() || fsr.AFF() || fsr.TF()) {
      // clang-format off
      fprintf(&f, "%s%s %s %s fault%s at page table level %u (FSYNR0 = 0x%08x)\n",
              fsynr0.AFR() ? "Asynchronous, " : "",
              fsynr0.PNU() ? "Privileged" : "Non-Privileged",
              fsynr0.IND() ? "instruction" : "data",
              fsynr0.WNR() ? "write" : "read",
              fsynr0.PTWF() ? ", during a page-table walk," : "",
              fsynr0.PLVL(),
              fsynr0.reg_value());
      // clang-format on
    }

    DRIVER_OOPS("%s", ktl::move(f).take().data());
  }

  // Clear any of the fault bookkeeping so we are ready to receive a new context
  // fault when we are able to.
  //
  // Then ack the fault(s) in the status register (FSR) by writing back the
  // values we read from it at the start of processing.  See section 16.5.9
  // which says "A value of 1 written to any non-reserved bit clears that bit".
  gr1::CBFRSYNRA::Get(cb_ndx).FromValue(0).WriteTo(&gr1_base_);
  s1cbr::FAR::Get().FromValue(0).WriteTo(&cb_base);
  s1cbr::FSYNR0::Get().FromValue(0).WriteTo(&cb_base);
  fsr.WriteTo(&cb_base);
}

void Smmu::Lockdown() {
  // Go over all of the stream match register groups and make sure that they are
  // all disabled.  If we find any enabled SMRs, print some details about the
  // fact that they were enabled as we disabled them.
  for (uint32_t i = 0; i < num_smrgs(); ++i) {
    // Skip any SMRGs which were adopted during early-init.
    if (!available_smrgs_.TestBit(i)) {
      dprintf(INFO, "%s : Lockdown skipping adopted SMRG %u\n", name(), i);
      continue;
    }

    gr0::SMR smr = gr0::SMR::Get(i).ReadFrom(&gr0_base_);
    gr0::S2CR s2cr = gr0::S2CR::Get(i).ReadFrom(&gr0_base_);

    if (smr.VALID()) {
      // Figure out what mode this group is operating in so we can report it.
      const char* tag1 = ArmS2crTypeToString(s2cr.TYPE());
      const char* tag2 = "";

      // If we are in translation mode, examine the configured context bank to
      // figure out how it was configured instead of simply saying "Translation".
      if (s2cr.TYPE() == S2CR_Type::kTranslation) {
        const uint32_t cb_ndx = gr0::S2CR_Translation::Get(i).FromValue(s2cr.reg_value()).CBNDX();

        if (cb_ndx < num_cbs()) {
          tag1 = ArmCbarTypeToString(gr1::CBAR::Get(cb_ndx).ReadFrom(&gr1_base_).TYPE());
          hwreg::RegisterMmio cb_base = get_cb_base(cb_ndx);

          // We know that this is the stage 1 version of SCTLR because we know
          // that this context bank was not adopted, and because we never
          // currently configure a context bank for stage 2 translation.
          tag2 = s1cbr::SCTLR::Get().ReadFrom(&cb_base).M() ? " - S1 Translate Enabled"
                                                            : " - S1 Translate Disabled";
        } else {
          tag1 = "Translate (invalid context bank)";
        }
      }

      dprintf(INFO, "%s : Lockdown\n", name());
      dprintf(INFO, "SMRG : %u\n", i);
      dprintf(INFO, "SMR  : ID 0x%04x MASK 0x%04x\n", smr.ID(), smr.MASK());
      dprintf(INFO, "Mode : %s%s\n", tag1, tag2);
    }

    StreamMatchRegGroup::Disable(*this, i);
  }

  // Go over all of our context banks which have not been claimed (adopted) and
  // disable them so that any transaction which might happen to reach it will be
  // denied.
  for (uint32_t i = 0; i < num_cbs(); ++i) {
    // Skip any Context Banks which were adopted during early-init.
    if (!available_cbs_.TestBit(i)) {
      dprintf(INFO, "%s : Lockdown skipping locked CB %u\n", name(), i);
      continue;
    }

    ContextBank::Disable(*this, i);
  }
}

void Smmu::InvalidateAllTLBEntries() {
  // Invalidate all TLB entries tagged as Non-secure, Non-hyp.
  gr0::TLBIALLNSNH::Get().FromValue(0).WriteTo(&gr0_base_);
  gr0::TLBGSYNC::Get().FromValue(0).WriteTo(&gr0_base_);
  while (gr0::TLBGSTATUS::Get().ReadFrom(&gr0_base_).GSACTIVE() != 0) {
    arch::Yield();
  }

  // See Section 5.4.2 "TLB maintenance operation processing"'s SYNC assembly
  // example.  We need a `dsb sy` instruction following this operation.
  arch::DeviceMemoryBarrier();
}

void Smmu::ShutdownBti(SmmuBti& bti) {
  // Shutting a BTI down is a process which takes a bit of care.  The sequence
  // is generally as follows:
  //
  // First, lock the SMMU for the duration of the process.
  //
  // Next, we need to deal with any context bank IRQ we may have registered, and
  // be certain that there cannot be any context bank IRQ in flight before we go
  // any further.
  //
  // So, lock the target BTI and peek at it's current context bank, if any.  If
  // it has a context bank, then make a note of the CB's index.  Once a BTI has
  // a context bank assigned, the only way for the context bank (and its
  // associated IRQ) to be returned to the SMMU pool is to be shutdown via the
  // SMMU itself.  We know that a concurrent shutdown operation cannot take
  // place right now, because we are holding the main SMMU lock, so we can be
  // sure that the CB effectively belongs to us and cannot be reused until we
  // are done here.
  //
  // So, if we have a context bank index assigned to us, we can unregister the
  // IRQ associated with that index (obtaining the IRQ lock in the process) and
  // synchronize with any in-flight IRQ operations without needing to worry
  // about holding the BTI lock as we do.
  //
  // Once we are certain that there are no interrupts in flight targeting our
  // hardware, and that there will not be until we are finished, we are free to
  // call into the BTI instance itself in order to shut down the rest of the
  // hardware.
  //
  Guard<Mutex> guard{&lock_};
  ktl::optional<uint32_t> maybe_cb_ndx = [&bti]() -> ktl::optional<uint32_t> {
    Guard<SpinLock, IrqSave> guard{&bti.get_lock()};
    return bti.context_bank_ ? ktl::optional<uint32_t>{bti.context_bank_->cb_ndx()} : ktl::nullopt;
  }();

  if (maybe_cb_ndx) {
    // Note: we know that at there are at least additional references to this
    // BTI which currently exist, independent of the reference which might be
    // held by the IRQ handler.  They are the reference held by our main BTI
    // list, as well as the reference which this method's caller must be
    // holding.
    ShutdownContextBankIrq(*maybe_cb_ndx);
  }

  // Go ahead and shut down the BTI hardware now, locking down all transactions
  // and returning all resources to the SMMU's pool in the process.  Once we are
  // done with that, we can remove ourselves from the global BTI list and we
  // should be finished.
  ZX_DEBUG_ASSERT(bti.InContainer());
  bti.Shutdown(*this);
  bti_list_.erase(bti);
}

void Smmu::AssociateBtiIrq(SmmuBti& bti, uint32_t cb_ndx) {
  Guard<SpinLock, IrqSave> guard{&irq_lock_};

  if (cb_ndx >= context_irqs_.size()) {
    dprintf(INFO,
            "%s : WARNING - Failed to associate context IRQ with BTI, invalid index (%u >= %zu)\n",
            name(), cb_ndx, context_irqs_.size());
    return;
  }

  ContextIrqVector& vec = context_irqs_[cb_ndx];
  ZX_DEBUG_ASSERT(vec.bti == nullptr);
  ZX_DEBUG_ASSERT(vec.registered);

  vec.bti = fbl::RefPtr<SmmuBti>{&bti};
  vec.enabled = true;
  unmask_interrupt(vec.irq_def.num);
}

void Smmu::ShutdownContextBankIrq(uint32_t cb_ndx) {
  fbl::RefPtr<SmmuBti> bti_ref;
  bool in_flight{false};
  {
    Guard<SpinLock, IrqSave> guard{&irq_lock_};

    if (cb_ndx >= context_irqs_.size()) {
      dprintf(INFO,
              "%s : WARNING - Failed to re-enable context IRQ with BTI, invalid index "
              "(%u >= %zu)\n",
              name(), cb_ndx, context_irqs_.size());
      return;
    }

    ContextIrqVector& vec = context_irqs_[cb_ndx];
    ZX_DEBUG_ASSERT(vec.registered);

    in_flight = vec.in_flight;
    bti_ref = ktl::move(vec.bti);  // Move the reference outside of the spinlock before dropping it.
    mask_interrupt(vec.irq_def.num);
  }

  // The last thing we need to do before it is safe to finish this routine is to
  // synchronize with any in-flight IRQs.  Currently we do this with a rather
  // crude polling of an in-flight flag which is protected by the `irq_lock_`.
  // It is *very* unlikely that a fault interrupt would ever collide with a
  // shutdown operation, and if it does, the IRQ is never going to take very
  // long to execute meaning that we are not going to be waiting for very long.
  while (in_flight) {
    Thread::Current::SleepRelative(ZX_USEC(100));
    {
      Guard<SpinLock, IrqSave> guard{&irq_lock_};
      const ContextIrqVector& vec = context_irqs_[cb_ndx];
      in_flight = vec.in_flight;
    }
  }
}

void Smmu::ReenableContextBankIrq(SmmuBti& bti) {
  Guard<Mutex> guard{&lock_};
  Guard<SpinLock, IrqSave> bti_guard{&bti.get_lock()};
  Guard<SpinLock, NoIrqSave> irq_guard{&irq_lock_};

  const uint32_t cb_ndx = bti.cb_ndx_locked();
  if (cb_ndx >= context_irqs_.size()) {
    dprintf(INFO,
            "%s : WARNING - Failed to disable context IRQ with BTI, invalid index (%u >= %zu)\n",
            name(), cb_ndx, context_irqs_.size());
    return;
  }

  // We have nothing to do if this vector is not registered, is currently
  // already enabled, or if the IRQ is currently in-flight.
  ContextIrqVector& vec = context_irqs_[cb_ndx];
  if (!vec.registered || vec.enabled || vec.in_flight) {
    return;
  }

  // Only re-enable the interrupt if the BTI is in an operational mode, either
  // bypass or translation.
  if ((bti.mode() != BtiMode::kBypass) && (bti.mode() != BtiMode::kTranslation)) {
    return;
  }

  // Go ahead and actually re-enable the interrupt.
  hwreg::RegisterMmio cb_base = get_cb_base(cb_ndx);
  s1cbr::SCTLR::Get().ReadFrom(&cb_base).set_CFIE(1).WriteTo(&cb_base);
  unmask_interrupt(vec.irq_def.num);
  vec.enabled = true;
}

zx_status_t Smmu::AdoptSmrg(uint32_t smrg_ndx, SmrValue reg_stream_ids) {
  // The index we are being given must be valid, the group must be already
  // enabled, and not currently in use according to our bookkeeping.  At this
  // point in initialization, we should also be able to ASSERT all of these
  // things.
  ZX_DEBUG_ASSERT(smrg_ndx < num_smrgs());
  ZX_DEBUG_ASSERT(available_smrgs_.TestBit(smrg_ndx));
  ZX_DEBUG_ASSERT(gr0::SMR::Get(smrg_ndx).ReadFrom(&gr0_base_).VALID());

  // Go over our existing BTIs.  None of the stream IDs we are attempting to
  // claim should currently be in use by any of our active BTIs.  If they are,
  // we have a problem as it implies that there are at least two SMRGs
  // configured to match one or more of the same stream id, which is undefined
  // behavior for SMMUv2 hardware.
  //
  // Return an error; we will disable and lock down this specific SMRG (at
  // smrg_ndx) after we have finished adoption of the rest of the early-init
  // SMRGs.
  for (const SmmuBti& bti : bti_list_) {
    if (bti.SmrIntersects(reg_stream_ids)) {
      return ZX_ERR_ALREADY_BOUND;
    }
  }

  // Create a new SMRG, and adopt its initial state from existing register state.
  ktl::unique_ptr<StreamMatchRegGroup> smrg;
  if (zx::result<ktl::unique_ptr<StreamMatchRegGroup>> res =
          StreamMatchRegGroup::CreateAndAdopt(*this, smrg_ndx, reg_stream_ids);
      res.is_ok()) {
    smrg = ktl::move(*res);
  } else {
    return res.error_value();
  }

  // If this SMRG is in translation mode, go looking for an existing BtiContext
  // which is already using the same context bank as this SMRG.  If we find one,
  // just add this SMRG to it. Otherwise, we'll need to create a new BtiContext.
  if (smrg->mode() == S2CR_Type::kTranslation) {
    for (SmmuBti& bti : bti_list_) {
      if (bti.cb_ndx() == smrg->cb_ndx()) {
        bti.AddSmrg(*this, ktl::move(smrg));
        // smrg is now nullptr, stop looking for BTIs to add it to.
        break;
      }
    }
  }

  // If our `smrg` pointer is still valid, we didn't find any BTI to add this
  // SMRG to.  We'll need to create a new one, and perhaps adopt a ContextBank
  // in the process.
  if (smrg != nullptr) {
    ktl::unique_ptr<ContextBank> context_bank;
    if (smrg->mode() == S2CR_Type::kTranslation) {
      if (zx::result<ktl::unique_ptr<ContextBank>> res =
              ContextBank::CreateAndAdopt(*this, smrg->cb_ndx());
          res.is_ok()) {
        context_bank = ktl::move(*res);
      } else {
        return res.error_value();
      }
    }

    fbl::RefPtr<SmmuBti> bti =
        SmmuBti::Create(*this, ktl::move(smrg), ktl::move(context_bank), BtiMode::kAdopted);
    if (bti == nullptr) {
      return ZX_ERR_NO_MEMORY;
    }

    // Success, add the BTI to the global list.  There is no need to mark either
    // the SMRG or the context bank as unavailable, SmmuBti::Create has
    // already done that for us.
    bti_list_.push_back(ktl::move(bti));
  }

  return ZX_OK;
}

}  // namespace arm_smmu
