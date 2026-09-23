// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_CONTEXT_BANK_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_CONTEXT_BANK_H_

#include <stdint.h>

#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_bti.h>
#include <fbl/intrusive_double_list.h>
#include <ktl/unique_ptr.h>

namespace arm_smmu {

class SmmuBti;

class ContextBank : public fbl::DoublyLinkedListable<ktl::unique_ptr<ContextBank>> {
 public:
  static zx::result<ktl::unique_ptr<ContextBank>> CreateAndLockdown(Smmu& smmu, uint32_t cb_ndx)
      TA_REQ(smmu.get_lock());

  static zx::result<ktl::unique_ptr<ContextBank>> CreateAndAdopt(Smmu& smmu, uint32_t cb_ndx)
      TA_REQ(smmu.get_lock());

  ContextBank(const ContextBank&) = delete;
  ContextBank operator=(const ContextBank&) = delete;
  ContextBank(ContextBank&&) = delete;
  ContextBank operator=(ContextBank&&) = delete;

  void SetMode(SmmuBti& owner, BtiMode mode) TA_REQ(owner.get_lock());

  static void Disable(Smmu& smmu, uint32_t cb_ndx) TA_REQ(smmu.get_lock()) {
    ZX_DEBUG_ASSERT(cb_ndx < smmu.num_cbs());
    ZX_DEBUG_ASSERT(smmu.available_cbs_.TestBit(cb_ndx));
    DisableRegs(smmu.gr1_base_, smmu.get_cb_base(cb_ndx), cb_ndx);
  }

  uint32_t cb_ndx() const { return cb_ndx_; }
  BtiMode mode() const { return mode_; }
  AddrMode addr_mode() const { return addr_mode_; }

 private:
  friend class ktl::default_delete<ContextBank>;
  friend class SmmuBti;

  // Info fetched from the TCR which tells us a few different things about how a
  // TTBR is configured.
  struct TTBRInfo {
    bool enabled{false};
    uint32_t granule_size_bits{0};
    uint64_t first_valid_addr{0};
    uint64_t last_valid_addr{0};
    uint64_t ttbr_paddr{0};
  };

  // When we configure for translation, we use a 48-bit device address space.
  static constexpr uint32_t kDefaultTCRT0SZ = 16;
  static constexpr uint64_t kDefaultTTBR0AddrSpaceSz = uint64_t{1} << (64 - kDefaultTCRT0SZ);

  // We need to allocate a unique ASID for every context bank in the system that
  // we control.  For now, we just use a context bank's index with some upper
  // bits set in it.
  //
  // TODO(johngro): We need to make sure that when we come out of the
  // bootloaders and claim the hardware, that we do not end up adopting any
  // hardware which is using one of the ASIDs we plan to use.  Currently, we
  // have no reference hardware which enters the kernel with the hardware
  // configured to perform address translation at all, so it isn't an issue yet.
  // Some day, however, we should take the time to provide a 100% guarantee that
  // there will be no collisions.
  static constexpr uint16_t kUnusedASID = 0xFFFF;
  static constexpr uint16_t kUpperASIDBits = 0x5A00;
  static constexpr uint16_t kUpperASIDMask = 0xFF00;

  static ktl::unique_ptr<ContextBank> Create(Smmu& smmu, uint32_t cb_ndx) TA_REQ(smmu.get_lock());
  static zx::result<> ValidateNdx(Smmu& smmu, uint32_t cb_ndx) TA_REQ(smmu.get_lock());
  static void DisableRegs(hwreg::RegisterMmio gr1_base, hwreg::RegisterMmio cb_base,
                          uint32_t cb_ndx);

  static uint16_t asid(uint32_t cb_ndx) {
    ZX_DEBUG_ASSERT(cb_ndx <= 0xFF);
    return static_cast<uint16_t>(cb_ndx | kUpperASIDBits);
  }

  ContextBank(uint32_t cb_ndx);
  ~ContextBank();

  // TLB Maintenance
  static void TLBSyncInvalidateOperation(hwreg::RegisterMmio& cb_base);
  static void TLBInvalidateByAsid(uint16_t asid, hwreg::RegisterMmio& cb_base);
  static void TLBInvalidateRegion(uint64_t base_va, uint64_t size, uint16_t asid,
                                  hwreg::RegisterMmio& cb_base);

  void TLBInvalidateByAsid() { TLBInvalidateByAsid(asid(), cb_base_); }
  void TLBInvalidateRegion(uint64_t base_va, uint64_t size) {
    TLBInvalidateRegion(base_va, size, asid(), cb_base_);
  }

  // aspace size accesses registers and should only be called from a
  // ContextBank's owning BTI, with the BTI's lock held.
  uint64_t aspace_size_for_mode(BtiMode mode);
  uint64_t aspace_size() { return aspace_size_for_mode(mode()); }
  uint16_t asid() const { return asid(cb_ndx()); }
  uint32_t DecodeGranuleSizeBits(uint32_t reg_bits) const;
  void DecodeTtbrRegions(uint32_t t0sz, uint32_t t1sz);
  zx::result<> AdoptRegisterState(Smmu& smmu) TA_REQ(smmu.get_lock());
  void LogFaultInfo() const;

  const uint32_t cb_ndx_;
  BtiMode mode_{BtiMode::kInvalid};
  hwreg::RegisterMmio gr1_base_{0};
  hwreg::RegisterMmio cb_base_{0};

  AddrMode addr_mode_{AddrMode::kInvalid};
  ktl::array<TTBRInfo, 2> ttbrs_;
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_CONTEXT_BANK_H_
