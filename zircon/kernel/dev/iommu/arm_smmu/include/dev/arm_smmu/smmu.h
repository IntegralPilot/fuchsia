// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_H_

#include <lib/console.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/iommu.h>

#include <dev/arm_smmu/bitmask.h>
#include <dev/arm_smmu/smmu_registers.h>
#include <dev/arm_smmu/utils.h>
#include <dev/interrupt.h>
#include <dev/iommu/iommu.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <hwreg/mmio.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <ktl/bit.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/unique_ptr.h>

namespace arm_smmu {

class ContextBank;
class SmmuBti;
class StreamMatchRegGroup;

// The implementation of an Iommu interface for an instance of an ARM SMMU
// (version 2).  Throughout this code, references (in the form of section and
// table numbers) will be made to the public ARM documentation which can be
// found in
//
// "ARM System Memory Management Unit Architecture Specification"
// "SMMU architecture version 2.0"
// "ARM IHI 0062D.c (ID070116)"
//
class Smmu final : public iommu::Iommu, public fbl::DoublyLinkedListable<fbl::RefPtr<Smmu>> {
 public:
  enum class CreateSource { kSyscall, kEarlyInit };

  struct IrqDef {
    constexpr IrqDef() : num(0), flags(0) {}
    constexpr IrqDef(uint32_t num, uint32_t flags) : num(num), flags(flags) {}
    explicit constexpr IrqDef(const zbi_dcfg_arm_smmu_irq_t& irq)
        : num(irq.num), flags(irq.flags) {}

    const uint32_t num;
    const uint32_t flags;

    bool valid() const {
      constexpr uint32_t valid_bits =
          ZBI_ARM_SMMU_IRQ_FLAGS_RISING_EDGE | ZBI_ARM_SMMU_IRQ_FLAGS_FALLING_EDGE |
          ZBI_ARM_SMMU_IRQ_FLAGS_ACTIVE_HIGH | ZBI_ARM_SMMU_IRQ_FLAGS_ACTIVE_LOW;
      return ktl::popcount(flags & valid_bits) == 1;
    }

    interrupt_trigger_mode trigger_mode() const {
      return flags & (ZBI_ARM_SMMU_IRQ_FLAGS_RISING_EDGE | ZBI_ARM_SMMU_IRQ_FLAGS_FALLING_EDGE)
                 ? interrupt_trigger_mode::EDGE
                 : interrupt_trigger_mode::LEVEL;
    }

    interrupt_polarity polarity() const {
      return flags & (ZBI_ARM_SMMU_IRQ_FLAGS_RISING_EDGE | ZBI_ARM_SMMU_IRQ_FLAGS_ACTIVE_HIGH)
                 ? interrupt_polarity::HIGH
                 : interrupt_polarity::LOW;
    }
  };

  ~Smmu() final;

  // No copy, no move
  Smmu(const Smmu&) = delete;
  Smmu(Smmu&&) = delete;
  Smmu& operator=(const Smmu&) = delete;
  Smmu& operator=(Smmu&&) = delete;

  // Create is used during kernel startup in order to create SMMU instances as
  // described to Zircon by the ZBI.
  static zx::result<fbl::RefPtr<iommu::Iommu>> Create(const zbi_dcfg_arm_smmu_driver_t& config);

  // Fetch is used during zx_iommu_create syscalls.  User mode is not allowed to
  // create arbitrary IOMMUs, they may only fetch references to instances which
  // were created during kernel startup.
  static zx::result<fbl::RefPtr<iommu::Iommu>> Fetch(ktl::unique_ptr<const uint8_t[]> desc,
                                                     size_t desc_len);

  //////////////////////////////////////////////////////////////////////////////
  //
  // Implementation of the iommu::Iommu driver interface
  //
  //////////////////////////////////////////////////////////////////////////////
  zx::result<fbl::RefPtr<iommu::Bti>> CreateBti(uint64_t bus_txn_id) final;
  // END iommu::Iommu driver interface implementation

  void ShutdownBti(SmmuBti& bti) TA_EXCL(lock_);
  void AssociateBtiIrq(SmmuBti& bti, uint32_t cb_ndx) TA_REQ(lock_) TA_EXCL(irq_lock_);
  void ShutdownContextBankIrq(uint32_t cb_ndx) TA_REQ(lock_) TA_EXCL(irq_lock_);
  void ReenableContextBankIrq(SmmuBti& bti) TA_EXCL(lock_, irq_lock_);

  ArmSmmuMode op_mode() const { return op_mode_; }
  const auto& get_lock() const TA_RET_CAP(lock_) { return lock_; }
  const auto& get_irq_lock() const TA_RET_CAP(irq_lock_) { return irq_lock_; }
  const char* name() const { return name_; }

  // Console support
  using BtiOpFunc = void (*)(const Smmu& smmu, SmmuBti& bti, uint32_t ndx);
  static int ConsoleCmd(int argc, const cmd_args* argv, uint32_t flags);
  int CmdShow(int argc, const cmd_args* argv, int cmd_ndx) const TA_EXCL(lock_);
  void CmdDumpRegs() TA_EXCL(lock_);
  int CmdBtiOp(int argc, const cmd_args* argv, int cmd_ndx, BtiOpFunc op) TA_EXCL(lock_);
  zx::result<fbl::RefPtr<SmmuBti>> FindCmdTarget(int argc, const cmd_args* argv,
                                                 uint32_t* out_ndx = nullptr) const TA_REQ(lock_);
  ktl::optional<IrqDef> get_context_irq(uint32_t ndx) const TA_EXCL(irq_lock_) {
    Guard<SpinLock, IrqSave> guard{&irq_lock_};
    return (ndx < context_irqs_.size()) ? ktl::optional<IrqDef>(context_irqs_[ndx].irq_def)
                                        : ktl::nullopt;
  }

 private:
  friend class ContextBank;
  friend class SmmuBti;
  friend class StreamMatchRegGroup;

  struct GlobalIrqVector {
    Smmu* smmu{nullptr};
    IrqDef irq_def;
    uint32_t ndx{0};
    bool registered{false};
  };

  struct ContextIrqVector {
    Smmu* smmu{nullptr};
    IrqDef irq_def;
    uint32_t ndx{0};
    bool enabled{false};
    bool registered{false};
    bool in_flight{false};
    fbl::RefPtr<SmmuBti> bti;
  };

  static fbl::RefPtr<Smmu> FindInstance(uint64_t phys_reg_base) TA_REQ(InstanceLock::Get()) {
    for (Smmu& smmu : instances_) {
      if (phys_reg_base == smmu.phys_reg_base_) {
        return fbl::RefPtr<Smmu>{&smmu};
      }
    }
    return nullptr;
  }

  Smmu(uint64_t phys_reg_base);

  zx_status_t Init(const zbi_dcfg_arm_smmu_driver_t& config) TA_EXCL(lock_);

  template <typename VectorType>
  zx_status_t AllocateIrqs(const ktl::span<const zbi_dcfg_arm_smmu_irq_t>& src) TA_REQ(lock_);
  template <typename VectorType>
  zx_status_t RegisterIrqs() TA_REQ(lock_);
  void UnregisterInterrupts() TA_REQ(lock_) TA_EXCL(irq_lock_);

  void Lockdown() TA_REQ(lock_);
  void InvalidateAllTLBEntries() TA_REQ(lock_);

  // Attempt to adopt an existing (valid) stream match register group and its
  // associated context bank (if any).
  //
  // When adopting SMRG state, if the SMRG configuration references a context
  // bank, one of two things may end up happening.
  //
  // 1) If the SMRG state references a context bank which has not yet been
  //    adopted, a new ContextBank object will be created and adopted, and a new
  //    BTI object will be created to hold the newly adopted SMRG and ContextBank
  //    objects.  The existing SMRG and ContextBank register state will be left
  //    as is.  Internal bookkeeping will be updated to match the pre-existing
  //    state.
  // 2) If the SMRG state references a ContextBank which has already been
  //    adopted by a different adopted SMRG, the newly adopted SMRG will be
  //    added to the exiting BTI which was created to contain the pre-existing
  //    SMRG(s)/ContextBank state.
  //
  zx_status_t AdoptSmrg(uint32_t smrg_ndx, SmrValue stream_ids) TA_REQ(lock_);

  void HandleGlobalIrq(uint32_t ndx) TA_EXCL(irq_lock_);
  void HandleContextIrq(uint32_t cb_ndx) TA_EXCL(irq_lock_);
  void PrintCommonContextIrqFaultDetails(uint32_t cb_ndx, const char* bti_name = nullptr)
      TA_EXCL(irq_lock_);

  // See Section 9.6.1 : SMMU_IDR0 and SMMU_IDR1 for more details, in addition
  // to Section 8.1.1.
  //
  // Please note that the IDR1.NUMPAGENDXB comment defining `reg_page_cnt`
  // appears to be mis-formatted.  It says that the number of pages is
  // `2(NUMPAGENDXB + 1)`, but fails to place the parenthetical in a
  // superscript, or use some other common notation used to indicate
  // exponentiation as opposed to multiplication.  In the absence of a
  // superscript font, either `2^(NUMPAGENDXB + 1)` or `2**(NUMPAGENDXB + 1)`
  // could be used as an acceptable substitute.  Section 8.1.1 uses a proper
  // superscript notation, and is more correct/clear
  size_t reg_page_size() const { return idr1_.PAGESIZE() ? (1u << 16) : (1u << 12); }
  size_t reg_page_cnt() const { return size_t{1} << (idr1_.NUMPAGENDXB() + 1); }
  uint32_t num_smrgs() const { return num_smrgs_; }
  uint32_t num_cbs() const { return num_cbs_; }

  hwreg::RegisterMmio get_cb_base(uint32_t ndx) {
    ZX_DEBUG_ASSERT(ndx < num_cbs());
    auto addr = reinterpret_cast<volatile void*>(cb0_base_.base() + (reg_page_size() * ndx));
    return hwreg::RegisterMmio{addr};
  }

  DECLARE_SINGLETON_MUTEX(InstanceLock);
  TA_GUARDED(InstanceLock::Get()) static fbl::DoublyLinkedList<fbl::RefPtr<Smmu>> instances_;

  mutable DECLARE_MUTEX(Smmu) lock_ TA_ACQ_AFTER(InstanceLock::Get());
  const uint64_t phys_reg_base_;
  TA_GUARDED(lock_) bool user_mode_bound_ { false };
  TA_GUARDED(lock_) fbl::DoublyLinkedList<fbl::RefPtr<SmmuBti>> bti_list_;

  // A bitmap of SMRGs and Context Banks which are currently available for
  // allocation.  Any SMRG/CB which is not available either:
  //
  // 1) Does not exist because the HW just does not have that many or, they
  //    were taken by the secure side of things.
  // 2) Is already in use and can be found somewhere in either the smrg_list_ or
  //    the cb_list_.
  //
  TA_GUARDED(lock_) Bitmask<128> available_smrgs_ {};
  TA_GUARDED(lock_) Bitmask<128> available_cbs_ {};

  //////////////////////////////////////////////////////////////////////////////
  //
  // Begin Init members.
  //
  // The following member variables are computed during Init and are effectively
  // constant afterwards.
  //
  //////////////////////////////////////////////////////////////////////////////

  // Our friendly name used in debug printfs.
  char name_[32] = {0};

  // Our operational mode.
  ArmSmmuMode op_mode_{ArmSmmuMode::kDisabled};

  // Cached ID registers.  Only defined after a successful call to Init.
  gr0::IDR0 idr0_{};
  gr0::IDR1 idr1_{};
  gr0::IDR2 idr2_{};
  gr0::IDR7 idr7_{};

  // A mask which can be used to determine the validity of a stream ID encoded
  // as a u32, with the MASK in the upper 16 bits and the ID in the lower 16
  // bits.  Note that this value is *not* constexpr.  Instead, it needs to be
  // computed at initialization time from a combination of the value in
  // IDR0.NUMSIDB as well as whether or not extended Stream ID matching
  // (CR0.EXIDENABLE) is being used.
  uint32_t valid_stream_id_mask_{0};

  // The effective count of the number of context banks available to us.  This
  // value may end up being smaller than IDR1.NUMCB if:
  //
  // 1) The configuration passed to us by the boot loader has restricted us.
  // 2) We are provided fewer context bank interrupts than we have context banks
  //    available to us.
  uint32_t num_cbs_{0};

  // The effective count of the number of stream match register groups banks
  // available to us. Similar to num_cbs_, this number might be smaller than
  // what is reported in IDR0.NUMSMRG if configuration has restricted us.
  uint32_t num_smrgs_{0};

  // Base addresses of the various register banks. See Section 8.1 "About the
  // SMMU address space".
  hwreg::RegisterMmio gr0_base_{0};
  hwreg::RegisterMmio gr1_base_{0};
  hwreg::RegisterMmio cb0_base_{0};

  TA_GUARDED(lock_) ktl::unique_ptr<GlobalIrqVector[]> global_irq_storage_;
  TA_GUARDED(lock_) ktl::unique_ptr<ContextIrqVector[]> context_irq_storage_;

  //////////////////////////////////////////////////////////////////////////////
  //
  // End Init members.
  //
  //////////////////////////////////////////////////////////////////////////////

  mutable DECLARE_SPINLOCK(Smmu) irq_lock_;
  TA_GUARDED(irq_lock_) ktl::span<GlobalIrqVector> global_irqs_;
  TA_GUARDED(irq_lock_) ktl::span<ContextIrqVector> context_irqs_;
};

}  // namespace arm_smmu

using ArmSmmu = arm_smmu::Smmu;

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_H_
