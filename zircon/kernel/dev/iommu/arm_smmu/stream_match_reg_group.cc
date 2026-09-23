// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <dev/arm_smmu/context_bank.h>
#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_registers.h>
#include <dev/arm_smmu/stream_match_reg_group.h>

namespace arm_smmu {

StreamMatchRegGroup::StreamMatchRegGroup(uint32_t smrg_ndx, SmrValue stream_ids)
    : smrg_ndx_(smrg_ndx), stream_ids_{stream_ids} {}

StreamMatchRegGroup::~StreamMatchRegGroup() {
  ZX_DEBUG_ASSERT(gr0_base_.base() == 0);
  ZX_DEBUG_ASSERT(mode_ == S2CR_Type::kInvalid);
  ZX_DEBUG_ASSERT(cb_ndx_ == kInvalidCBNdx);
}

ktl::unique_ptr<StreamMatchRegGroup> StreamMatchRegGroup::Create(Smmu& smmu, uint32_t smrg_ndx,
                                                                 SmrValue stream_ids) {
  fbl::AllocChecker ac;
  ktl::unique_ptr<StreamMatchRegGroup> smrg{new (&ac) StreamMatchRegGroup{smrg_ndx, stream_ids}};
  if (!ac.check()) {
    return nullptr;
  }

  // Cache references to the base addresses for our registers.  While we exist,
  // we are the only thing allowed to write to them.  Specifically, we own the
  // following registers in global register space 0.
  //
  // + SMR(smrg_ndx)
  // + S2CR(smrg_ndx)
  //
  smrg->gr0_base_ = smmu.gr0_base_;

  return smrg;
}

zx::result<ktl::unique_ptr<StreamMatchRegGroup>> StreamMatchRegGroup::CreateAndLockdown(
    Smmu& smmu, uint32_t smrg_ndx, SmrValue stream_ids) {
  if (zx::result<> res = ValidateNdx(smmu, smrg_ndx); res.is_error()) {
    return res.take_error();
  }

  ktl::unique_ptr<StreamMatchRegGroup> smrg = Create(smmu, smrg_ndx, stream_ids);
  if (smrg == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // To lock-down, disable this SMRG at the register level, and we should be
  // finished.
  DisableRegs(smrg->gr0_base_, smrg->smrg_ndx_);
  return zx::ok(ktl::move(smrg));
}

zx::result<ktl::unique_ptr<StreamMatchRegGroup>> StreamMatchRegGroup::CreateAndAdopt(
    Smmu& smmu, uint32_t smrg_ndx, SmrValue stream_ids) {
  if (zx::result<> res = ValidateNdx(smmu, smrg_ndx); res.is_error()) {
    return res.take_error();
  }

  ktl::unique_ptr<StreamMatchRegGroup> smrg = Create(smmu, smrg_ndx, stream_ids);
  if (smrg == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  if (zx::result<> res = smrg->AdoptRegisterState(smmu); res.is_error()) {
    return res.take_error();
  }

  return zx::ok(ktl::move(smrg));
}

zx::result<> StreamMatchRegGroup::ValidateNdx(Smmu& smmu, uint32_t smrg_ndx) {
  if (smrg_ndx >= smmu.num_smrgs()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (!smmu.available_smrgs_.TestBit(smrg_ndx)) {
    return zx::error(ZX_ERR_ALREADY_BOUND);
  }

  return zx::ok();
}

zx::result<> StreamMatchRegGroup::AdoptRegisterState(Smmu& smmu) {
  gr0::S2CR s2cr = gr0::S2CR::Get(smrg_ndx_).ReadFrom(&smmu.gr0_base_);
  mode_ = s2cr.TYPE();

  if (mode_ == S2CR_Type::kInvalid) {
    dprintf(INFO, "%s ERROR: Bad S2CR type (%u) when attempting to adopt SMRG #%u\n", smmu.name(),
            static_cast<uint32_t>(mode_), smrg_ndx_);
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (mode_ == S2CR_Type::kTranslation) {
    gr0::S2CR_Type0 s2cr_t0 = gr0::S2CR_Type0::Get(smrg_ndx_).FromValue(s2cr.reg_value());

    if (s2cr_t0.CBNDX() >= smmu.num_cbs()) {
      dprintf(INFO, "%s ERROR: Invalid context bank index (%u) when attempting to adopt SMRG #%u\n",
              smmu.name(), s2cr_t0.CBNDX(), smrg_ndx_);
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }

    cb_ndx_ = s2cr_t0.CBNDX();
  }

  return zx::ok();
}

void StreamMatchRegGroup::EnableForContextBank(SmmuBti& owner, uint32_t cb_ndx) {
  owner.AssertOwned(*this);
  ZX_DEBUG_ASSERT((mode_ == S2CR_Type::kInvalid) == (cb_ndx_ == kInvalidCBNdx));
  if (mode_ == S2CR_Type::kInvalid) {
    mode_ = S2CR_Type::kTranslation;
    cb_ndx_ = cb_ndx;

    // Note: this code assumes that we are not running with extended (16 bit)
    // stream IDs enabled. This is currently enforced during initialization of the
    // top level SMMU itself.

    // For the S2CR, start with a value of 0.  This will set all of the overrides
    // for all of the hints (transient, instruction fetch, write allocation,
    // etc...) to "default".
    //
    // Then configure the register to indicate translation mode, and direct it
    // to our context bank. Finally, attempt to configure PRIVCFG to
    // unconditionally flag all transactions as "Unprivileged".  Whether or not
    // this will actually change transactions depends on the IDR2.DIPANS bit.
    //
    // Forcing accesses to be unprivileged provides finer grained access control
    // in translation mode when using VMSAv8-64 translation tables.
    // Specifically, it is not possible to deny Read access to a privileged
    // access when there exists a valid translation, but it is possible when
    // accesses are unprivileged.  See section D8.4.1.2.6 Table D8-62 in the ARM
    // ARM.
    //
    gr0::S2CR_Translation::Get(smrg_ndx_)
        .FromValue(0)
        .set_TYPE(S2CR_Type::kTranslation)
        .set_CBNDX(cb_ndx_)
        .set_PRIVCFG(0x2)
        .WriteTo(&gr0_base_);

    gr0::SMR::Get(smrg_ndx_)
        .FromValue(0)
        .set_VALID(1)
        .set_MASK(stream_ids_.mask())
        .set_ID(stream_ids_.value())
        .WriteTo(&gr0_base_);

    arch::DeviceMemoryBarrier();
  }
}

void StreamMatchRegGroup::DisableRegs(hwreg::RegisterMmio gr0_base, uint32_t smrg_ndx) {
  // Note: this code assumes that we are not running with extended (16 bit)
  // stream IDs enabled. This is currently enforced during initialization of the
  // top level SMMU itself.
  gr0::SMR::Get(smrg_ndx).ReadFrom(&gr0_base).set_VALID(0).set_MASK(0).set_ID(0).WriteTo(&gr0_base);
  gr0::S2CR::Get(smrg_ndx).ReadFrom(&gr0_base).set_TYPE(S2CR_Type::kFault).WriteTo(&gr0_base);
  arch::DeviceMemoryBarrier();
}

}  // namespace arm_smmu
