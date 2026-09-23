// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/console.h>

#include <arch/arm64/periphmap.h>
#include <dev/arm_smmu/context_bank.h>
#include <dev/arm_smmu/device_aspace.h>
#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_bti.h>
#include <dev/arm_smmu/smmu_pmt.h>
#include <dev/arm_smmu/smmu_registers.h>
#include <dev/arm_smmu/stream_match_reg_group.h>
#include <dev/arm_smmu/utils.h>
#include <hwreg/mmio.h>
#include <ktl/limits.h>

namespace arm_smmu {

namespace {

constexpr int INDENT_SPACES = 2;
#define SMMU_FMT_INDENT(x) (INDENT_SPACES * (x)), ""

RelaxedAtomic<uint32_t> gDefaultSmmuNdx{0};

int usage(int ret = ZX_ERR_INVALID_ARGS) {
  printf("Usage: smmu <cmd>\n");
  printf("Valid commands are:\n");
  printf("  help     : show this message\n");
  printf("  list     : list all discovered IOMMU instances and their currently managed BTIs.\n");
  printf("  set <id> : set the default SMMU to use with other commands.\n");
  printf("  show <tgt> [-v] [-pmts]\n");
  printf("    Print the current high level status of the selected SMMU.\n");
  printf("    Use <tgt> to select a specific BTI to dump details about, or\n");
  printf("    \"all\" to show the status for the entire smmu\n");
  printf("  dumpregs\n");
  printf("    Print raw register information for the selected SMMU\n");
  printf("  lock <tgt>\n");
  printf("    Attempt to set the operating mode of the target BTI(s) to FAULT.\n");
  printf("  isids <tgt>\n");
  printf("    Attempt to invalidate all Stream IDs of the target BTI(s).\n");
  printf("\n");
  printf("Params:\n");
  printf("  <tgt>   : One of (all | bti <N> | sid <Stream ID>)\n");
  printf("  <--id>  : The ID of the SMMU to operate on, instead of the current default.\n");
  printf("  [-v]    : Optionally enable verbose output, for sub-commands who support it.\n");
  printf("  [-pmts] : Show detailed PMT status when using the show command\n");
  return ret;
}

bool FindOptionalFlag(int argc, const cmd_args* argv, const char* option) {
  for (int i = 0; i < argc; ++i) {
    if (!strcmp(argv[i].str, option)) {
      return true;
    }
  }
  return false;
}

zx::result<ktl::optional<uint32_t>> FindOptionalU32(int argc, const cmd_args* argv,
                                                    const char* option) {
  ktl::optional<uint32_t> ret;
  for (int i = 0; i < argc; ++i) {
    const cmd_args& arg = argv[i];
    if (!strcmp(arg.str, option)) {
      if (ret) {
        printf("Multiple \"%s\" options specified!\n", option);
        return zx::error(ZX_ERR_ALREADY_EXISTS);
      }

      if (++i >= argc) {
        printf("No value specified for option \"%s\"!\n", option);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      const cmd_args& value = argv[i];
      if (value.u > ktl::numeric_limits<uint32_t>::max()) {
        printf("Value out of range for for option \"%s\" (%lu)!\n", option, value.u);
        return zx::error(ZX_ERR_OUT_OF_RANGE);
      }

      ret = static_cast<uint32_t>(value.u);
    }
  }

  return zx::ok(ret);
}

template <typename T>
void DumpReg(const char* tag, const T& r, const hwreg::RegisterMmio& base, int ilvl = 0) {
  using VT = T::ValueType;

  if constexpr (T::PrinterEnabled::value) {
    constexpr const char* raw_field_name = "raw";
    size_t max_name_width = strlen(raw_field_name);

    r.ForEachField([&max_name_width](const char* name, VT, uint32_t, uint32_t) {
      if (name != nullptr) {
        max_name_width = ktl::max(max_name_width, strlen(name));
      }
    });

    char fmt_string[64];
    const char* val_fmt = sizeof(VT) == 8 ? "0x%016lx (%lu)" : "0x%08x (%u)";
    int fmt_res = snprintf(fmt_string, sizeof(fmt_string), "%*s[%%2u:%%2u] : %%%zus : %s\n",
                           SMMU_FMT_INDENT(ilvl), max_name_width, val_fmt);
    if ((fmt_res < 0) || (fmt_res >= static_cast<int>(sizeof(fmt_string)))) {
      printf("Format error when printing %s (res %d)\n", tag, fmt_res);
      return;
    }

    printf("%*s%s : [0x%lx]\n", SMMU_FMT_INDENT(ilvl), tag,
           periph_vaddr_to_paddr(base.base()) + r.reg_addr());
    auto field_printer = [&fmt_string](const char* name, VT val, uint32_t hi, uint32_t lo) {
      constexpr const char* kReservedPrefix = "__res";
      if ((name != nullptr) && (strncmp(name, kReservedPrefix, strlen(kReservedPrefix)))) {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wformat-nonliteral"
        printf(fmt_string, hi, lo, name, val, val);
#pragma GCC diagnostic pop
      }
    };

    field_printer(raw_field_name, r.reg_value(), (sizeof(VT) << 3) - 1, 0);
    r.ForEachField(field_printer);
    printf("\n");
  } else {
    if constexpr (sizeof(VT) == 8) {
      printf("%*s%s : 0x%16lx (%lu)\n", SMMU_FMT_INDENT(ilvl), tag, r.reg_value(), r.reg_value());
    } else {
      printf("%*s%s : 0x%08x (%u)\n", SMMU_FMT_INDENT(ilvl), tag, r.reg_value(), r.reg_value());
    }
  }
}
}  // namespace

zx::result<fbl::RefPtr<SmmuBti>> Smmu::FindCmdTarget(int argc, const cmd_args* argv,
                                                     uint32_t* out_ndx) const {
  if (out_ndx) {
    *out_ndx = 0;
  }

  if (argc < 1) {
    printf("No target specified.  One of (all|bti <N>|sid <Id>) is required.\n");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const char* tgt_type = argv[0].str;
  if (!strcmp(tgt_type, "all")) {
    return zx::ok(nullptr);
  } else {
    const bool is_bti = !strcmp(tgt_type, "bti");
    if (!is_bti && strcmp(tgt_type, "sid")) {
      printf("Invalid target \"%s\"\n", tgt_type);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    if (argc < 2) {
      printf("Insufficient arguments for \"%s\" target\n", tgt_type);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    const uint32_t tgt_val = static_cast<uint32_t>(
        ktl::clamp<uint64_t>(argv[1].u, 0, ktl::numeric_limits<uint32_t>::max()));
    if (!is_bti && (tgt_val & ~valid_stream_id_mask_)) {
      printf("Invalid SID 0x%x\n", tgt_val);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    uint32_t local_ndx{0};
    uint32_t& ndx = out_ndx ? *out_ndx : local_ndx;
    for (auto iter = bti_list_.cbegin(); iter.IsValid(); ++iter, ++ndx) {
      if (is_bti) {
        if (ndx == tgt_val) {
          return zx::ok(iter.CopyPointer());
        }
      } else {
        if (iter->SmrIntersects(SmrValue(tgt_val, valid_stream_id_mask_))) {
          return zx::ok(iter.CopyPointer());
        }
      }
    }

    printf("Failed to find %s target 0x%x in %s\n", tgt_type, tgt_val, name());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
}

int Smmu::CmdShow(int argc, const cmd_args* argv, int cmd_ndx) const {
  Guard<Mutex> guard{&lock_};

  int arg_ndx = cmd_ndx + 1;
  ZX_DEBUG_ASSERT(arg_ndx <= argc);

  zx::result<fbl::RefPtr<SmmuBti>> maybe_tgt = FindCmdTarget(argc - arg_ndx, argv + arg_ndx);
  if (maybe_tgt.is_error()) {
    return maybe_tgt.status_value();
  }

  bool verbose = FindOptionalFlag(argc, argv, "-v");
  bool show_pmts = FindOptionalFlag(argc, argv, "-pmts");
  fbl::RefPtr<SmmuBti> tgt = ktl::move(maybe_tgt.value());
  if (tgt == nullptr) {
    printf("Dumping configuration for %s\n", name());
    printf("Version    : %u.%u\n", idr7_.major(), idr7_.minor());
    printf("User-Bound : %s\n", user_mode_bound_ ? "Yes" : "No");
    printf("OpMode     : %s\n", ArmSmmuModeToString(op_mode_));
    printf("BTI Count  : %zu\n", bti_list_.size_slow());

    size_t i = 0;
    for (const SmmuBti& bti : bti_list_) {
      {
        char bti_name[ZX_MAX_NAME_LEN];
        bti.name_.get(ktl::size(bti_name), bti_name);
        printf("\nBTI #%zu (\"%s\")\n", i++, bti_name);
      }
      bti.CmdShow(1, show_pmts, verbose);
    }
  } else {
    tgt->CmdShow(0, show_pmts, verbose);
  }

  return ZX_OK;
}

#define I(x) (ilvl + (x))
#define INDENT(x) SMMU_FMT_INDENT(I(x))
void SmmuBti::CmdShow(int ilvl, bool show_pmts, bool verbose) const {
  Guard<Mutex> pmt_guard{&pmt_lock_};
  Guard<SpinLock, IrqSave> guard{&lock_};

  printf("%*sMode              : %s%s\n", INDENT(0), BtiModeToString(mode_),
         orphaned_ ? " (orphaned)" : "");
  {
    char bti_name[ZX_MAX_NAME_LEN];
    name_.get(ktl::size(bti_name), bti_name);
    printf("%*sName              : %s\n", INDENT(0), bti_name);
  }
  {
    ktl::array<char, 128> sid_buffer{0};
    printf("%*sStreamIDs         : %s\n", INDENT(0), RenderSidList(sid_buffer));
  }
  printf("%*sActive PMTs       : %zu\n", INDENT(0), active_pmt_list_.size());
  printf("%*sQuarantined PMTs  : %lu\n", INDENT(0), quarantined_pmt_count_);
  printf("%*sQuarantined Pages : %lu\n", INDENT(0), quarantined_page_count_);
  if (aspace_ != nullptr) {
    printf("%*sTLB Pages         : %u\n", INDENT(0), aspace_->page_cache().in_flight_pages());
    printf("%*sCached TLB Pages  : %u\n", INDENT(0), aspace_->page_cache().cache_entries());
  }

  if ((show_pmts) && (mode_ == BtiMode::kTranslation)) {
    size_t ndx{0};
    uint64_t page_total{0};

    for (const SmmuPmt& pmt : active_pmt_list_) {
      pmt.AssertOwnerPmtLockHeld();
      if (pmt.map_location()) {
        const uint64_t base = pmt.map_location()->base;
        const uint64_t size = pmt.map_location()->size;
        const uint64_t page_size = size >> DeviceAspace::kPageShift;

        printf("%*sPMT[%3zu] : [0x%lx, 0x%lx] (%lu Page%s)\n", INDENT(1), ndx, base,
               base + size - 1, page_size, page_size == 1 ? "" : "s");
        page_total += page_size;
      } else {
        // If we have an active PMT in a BTI operating in translation mode, we
        // may not have our map_location any more because someone used the debug
        // console to force-drop our mapping with the "lock" command.
        printf("%*sPMT[%3zu] : ???\n", INDENT(1), ndx);
      }
      ++ndx;
    }
    printf("%*sCounted %lu mapped page%s.\n", INDENT(1), page_total, page_total == 1 ? "" : "s");
  }

  const size_t smrg_count = smrg_list_.size_slow();
  uint32_t smrg_num = 0;
  for (const StreamMatchRegGroup& smrg : smrg_list_) {
    printf("\n%*sSMRG %u/%zu (index %u)\n", INDENT(0), ++smrg_num, smrg_count, smrg.smrg_ndx());
    printf("%*sMode  : %s\n", INDENT(1), ArmS2crTypeToString(smrg.mode()));
    printf("%*sMask  : 0x%04x\n", INDENT(1), smrg.stream_ids().mask());
    printf("%*sID    : 0x%04x\n", INDENT(1), smrg.stream_ids().id());
    printf("%*sCBNdx : 0x%04x\n", INDENT(1), smrg.cb_ndx());

    if (verbose) {
      hwreg::RegisterMmio* gr0_base = const_cast<hwreg::RegisterMmio*>(&smrg.gr0_base_);
      DumpReg("SMR", gr0::SMR::Get(smrg.smrg_ndx()).ReadFrom(gr0_base), *gr0_base, ilvl + 2);

      char s2cr_tag[24]{0};
      snprintf(s2cr_tag, sizeof(s2cr_tag), "S2CR (%s)", ArmS2crTypeToString(smrg.mode()));
      switch (smrg.mode()) {
        case S2CR_Type::kTranslation:
          DumpReg(s2cr_tag, gr0::S2CR_Translation::Get(smrg.smrg_ndx()).ReadFrom(gr0_base),
                  *gr0_base, ilvl + 2);
          break;
        case S2CR_Type::kBypass:
          DumpReg(s2cr_tag, gr0::S2CR_Bypass::Get(smrg.smrg_ndx()).ReadFrom(gr0_base), *gr0_base,
                  ilvl + 2);
          break;
        case S2CR_Type::kFault:
          DumpReg(s2cr_tag, gr0::S2CR_Fault::Get(smrg.smrg_ndx()).ReadFrom(gr0_base), *gr0_base,
                  ilvl + 2);
          break;
        default:
          DumpReg(s2cr_tag, gr0::S2CR::Get(smrg.smrg_ndx()).ReadFrom(gr0_base), *gr0_base,
                  ilvl + 2);
          break;
      }
    }
  }

  if (context_bank_ != nullptr) {
    hwreg::RegisterMmio& gr1_base = context_bank_->gr1_base_;
    hwreg::RegisterMmio& cb_base = context_bank_->cb_base_;
    const uint32_t cb_ndx = context_bank_->cb_ndx();
    const gr1::CBAR cbar = gr1::CBAR::Get(cb_ndx).ReadFrom(&gr1_base);
    const s1cbr::SCTLR sctlr = s1cbr::SCTLR::Get().ReadFrom(&cb_base);
    const ktl::optional<Smmu::IrqDef> irq = smmu_ ? smmu_->get_context_irq(cb_ndx) : ktl::nullopt;

    printf("\n%*sContext Bank (index %u)\n", INDENT(0), cb_ndx);
    printf("%*sMode      : %s\n", INDENT(1), BtiModeToString(context_bank_->mode()));
    printf("%*sIRQ       : %u\n", INDENT(1), irq.value_or(Smmu::IrqDef{}).num);
    printf("%*sAddrMode  : %s\n", INDENT(1), AddrModeToString(context_bank_->addr_mode()));
    printf("%*sCBAR Type : %s\n", INDENT(1), ArmCbarTypeToString(cbar.TYPE()));
    printf("%*sMMU State : %s\n", INDENT(1), sctlr.M() ? "Enabled" : "Disabled");

    for (uint32_t i = 0; i < context_bank_->ttbrs_.size(); ++i) {
      const ContextBank::TTBRInfo& ttbr = context_bank_->ttbrs_[i];
      printf("%*sTTBR[%u]   : %s\n", INDENT(1), i, ttbr.enabled ? "Enabled" : "Disabled");

      if (ttbr.enabled) {
        printf("%*sGranule Size : %u\n", INDENT(2), ttbr.granule_size_bits);
        printf("%*sFirst Valid  : 0x%016lx\n", INDENT(2), ttbr.first_valid_addr);
        printf("%*sLast Valid   : 0x%016lx\n", INDENT(2), ttbr.last_valid_addr);
        printf("%*sPhys Addr    : 0x%016lx\n", INDENT(2), ttbr.ttbr_paddr);
      }

      // TODO(johngro): if we are in translation mode, walk the translation table
      // and print the active mappings.
    }

    if (verbose) {
      switch (cbar.TYPE()) {
        case gr1::CBAR::Type::kS2Translation:
          DumpReg("CBAR (S2Translate)",
                  gr1::CBAR_S2Translation::Get(cb_ndx).FromValue(cbar.reg_value()), gr1_base, I(2));
          break;
        case gr1::CBAR::Type::kS1TS2Bypass:
          DumpReg("CBAR (S1TS2Bypass)",
                  gr1::CBAR_S1TS2Bypass::Get(cb_ndx).FromValue(cbar.reg_value()), gr1_base, I(2));
          break;
        case gr1::CBAR::Type::kS1TS2Fault:
          DumpReg("CBAR (S1TS2Fault)",
                  gr1::CBAR_S1TS2Fault::Get(cb_ndx).FromValue(cbar.reg_value()), gr1_base, I(2));
          break;
        case gr1::CBAR::Type::kS1TS2Translate:
          DumpReg("CBAR (S1TS2Translate)",
                  gr1::CBAR_S1TS2Translate::Get(cb_ndx).FromValue(cbar.reg_value()), gr1_base,
                  I(2));
          break;
      }

      DumpReg("CBA2R", gr1::CBA2R::Get(cb_ndx).ReadFrom(&context_bank_->gr1_base_), gr1_base, I(2));
      DumpReg("SCTLR", sctlr, cb_base, I(2));

      switch (context_bank_->addr_mode()) {
        case AddrMode::k32Bit:
          DumpReg("TTBR0 (32)", s1cbr::TTBR0_32Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          DumpReg("TTBR1 (32)", s1cbr::TTBR1_32Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          DumpReg("TCR (32)", s1cbr::TCR_32Bit::Get().ReadFrom(&context_bank_->cb_base_), cb_base,
                  I(2));
          break;
        case AddrMode::kExt32Bit:
          DumpReg("TTBR0 (E32)", s1cbr::TTBR0_Ext32Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          DumpReg("TTBR1 (E32)", s1cbr::TTBR1_Ext32Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          DumpReg("TCR (E32)", s1cbr::TCR_Ext32Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          break;
        case AddrMode::k64Bit:
          DumpReg("TTBR0 (64)", s1cbr::TTBR0_64Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          DumpReg("TTBR1 (64)", s1cbr::TTBR1_64Bit::Get().ReadFrom(&context_bank_->cb_base_),
                  cb_base, I(2));
          DumpReg("TCR (64)", s1cbr::TCR_64Bit::Get().ReadFrom(&context_bank_->cb_base_), cb_base,
                  I(2));
          break;
        default:
          DumpReg("TCR", s1cbr::TCR::Get().ReadFrom(&context_bank_->cb_base_), cb_base, I(2));
          break;
      }

      DumpReg("TCR2", s1cbr::TCR2::Get().ReadFrom(&context_bank_->cb_base_), cb_base, I(2));
    }
  }
}
#undef INDENT
#undef I

void SmmuBti::CmdLock(uint32_t ndx) {
  char bti_name[ZX_MAX_NAME_LEN];
  name_.get(ktl::size(bti_name), bti_name);

  // We are going to do one of two things, depending on whether we are in
  // Enforced mode, or in Passthru mode.  If we are in enforced mode, we are
  // going to simply go through our list of active PMTs, and remove all of
  // their mappings from the Page Tables.  This does not actually "lock" the
  // BTI, it just makes existing pinned memory in-accessible (assuming that
  // there is any).
  //
  // When we are in passthru mode, we just force the BTI (and its associated
  // context bank) into Fault mode, meaning that the MMU will be enabled, but
  // the TTBRs will be disabled, causing every translation request to fail
  // instead of passing through.
  if (smmu_->op_mode() == ArmSmmuMode::kEnforced) {
    Guard<Mutex> pmt_guard{&pmt_lock_};

    // Go through this BTI's PMTs and release any mappings back to the BTI's
    // address space.  Don't actually release the pinned memory yet, we are
    // only trying to force some faults.
    size_t released{0};
    for (SmmuPmt& pmt : active_pmt_list_) {
      pmt.AssertOwnerPmtLockHeld();

      // In general, we should be able to assert that all active PMTs have a
      // mapping during normal operation.  However, it is possible that someone
      // has run this command twice and the mappings were already released.  To
      // keep our reporting consistent, make sure to only count mappings we
      // actually released.
      if (pmt.map_location()) {
        ReleaseMapping(pmt.TakeMapLocation());
        ++released;
      }
    }
    printf("%s: BTI index #%u (\"%s\") released %zu active mapping%s.\n", smmu_->name(), ndx,
           bti_name, released, (released == 1) ? "" : "s");
  } else {
    Guard<SpinLock, IrqSave> bti_guard{&lock_};

    // If the SMMU is not operating in Enforced mode, it must be in passthru
    // mode (if it was in disabled mode, there would be no constructed SmmuBti
    // objects).
    ZX_DEBUG_ASSERT(smmu_->op_mode() == ArmSmmuMode::kPassthru);

    // Force the BTI into fault mode.  This will lock things down if we
    // are in passthru mode, and prevent new PMTs from being created moving
    // forward.
    const zx::result<> result = SetModeLocked(BtiMode::kFault);
    if (result.is_ok()) {
      // We are forcing our BTI into fault mode from the debug console as opposed
      // to in reaction to an interrupt.  Re-enable the context fault interrupt,
      // so that the console user can see a fault exception during the next access
      // in response to the lockdown command issued from the console.
      if (context_bank_) {
        s1cbr::SCTLR::Get()
            .ReadFrom(&context_bank_->cb_base_)
            .set_CFIE(1)
            .WriteTo(&context_bank_->cb_base_);
      }
      printf("%s: BTI index #%u (\"%s\") set to mode FAULT.\n", smmu_->name(), ndx, bti_name);
    } else {
      printf("%s: Failed to set BTI index #%u (\"%s\") to mode FAULT (err %d).\n", smmu_->name(),
             ndx, bti_name, result.error_value());
    }
  }
}

void Smmu::CmdDumpRegs() {
  Guard<Mutex> guard{&lock_};

  printf("Dumping configuration for %s\n", name());
  printf("Version    : %u.%u\n", idr7_.major(), idr7_.minor());
  printf("User-Bound : %s\n", user_mode_bound_ ? "Yes" : "No");

  const auto idr0 = gr0::IDR0::Get().ReadFrom(&gr0_base_);
  DumpReg("IDR0", idr0, gr0_base_);
  DumpReg("IDR1", gr0::IDR1::Get().ReadFrom(&gr0_base_), gr0_base_);
  DumpReg("IDR2", gr0::IDR2::Get().ReadFrom(&gr0_base_), gr0_base_);
  DumpReg("IDR7", gr0::IDR7::Get().ReadFrom(&gr0_base_), gr0_base_);
  DumpReg("CR0", gr0::CR0::Get().ReadFrom(&gr0_base_), gr0_base_);
  DumpReg("CR2", gr0::CR2::Get().ReadFrom(&gr0_base_), gr0_base_);

  // If this unit supports stream matching, dump the state of the available
  // stream matching register groups.  If a stream matching group is in
  // translation mode, and has a valid context bank ID configured, make a note
  // of it so we can come back later and dump the status of the in-use context
  // banks.
  Bitmask<128> in_use_cbs;
  char tag[64];
  if (idr0.SMS()) {
    for (uint32_t i = 0; i < idr0.NUMSMRG(); ++i) {
      const auto smr = gr0::SMR::Get(i).ReadFrom(&gr0_base_);
      if (smr.VALID()) {
        snprintf(tag, sizeof(tag), "SMR%u", i);
        DumpReg(tag, smr, gr0_base_);

        const auto s2cr = gr0::S2CR::Get(i).ReadFrom(&gr0_base_);
        snprintf(tag, sizeof(tag), "S2CR%u (%s)", i, ArmS2crTypeToString(s2cr.TYPE()));
        switch (s2cr.TYPE()) {
          case S2CR_Type::kTranslation: {
            const gr0::S2CR_Translation s2cr_trans =
                gr0::S2CR_Translation::Get(i).FromValue(s2cr.reg_value());
            DumpReg(tag, s2cr_trans, gr0_base_);

            if (s2cr_trans.CBNDX() >= num_cbs()) {
              printf("Invalid Context Bank ID %u\n", s2cr_trans.CBNDX());
            } else {
              in_use_cbs.SetBit(s2cr_trans.CBNDX());
            }
          } break;
          case S2CR_Type::kBypass:
            DumpReg(tag, gr0::S2CR_Bypass::Get(i).FromValue(s2cr.reg_value()), gr0_base_);
            break;
          case S2CR_Type::kFault:
            DumpReg(tag, gr0::S2CR_Fault::Get(i).FromValue(s2cr.reg_value()), gr0_base_);
            break;
          default:
            DumpReg(tag, s2cr, gr0_base_);
            break;
        }
      }
    }
  }

  // Now dump details about any in-use context banks.
  for (uint32_t i = 0; i < num_cbs(); ++i) {
    if (!in_use_cbs.TestBit(i)) {
      continue;
    }

    printf("Context Bank %u:\n", i);
    gr1::CBAR cbar = gr1::CBAR::Get(i).ReadFrom(&gr1_base_);
    snprintf(tag, sizeof(tag), "CBAR%u (%s)", i, ArmCbarTypeToString(cbar.TYPE()));
    switch (cbar.TYPE()) {
      case CBAR_Type::kS2Translation:
        DumpReg(tag, gr1::CBAR_S2Translation::Get(i).FromValue(cbar.reg_value()), gr1_base_);
        break;
      case CBAR_Type::kS1TS2Bypass:
        DumpReg(tag, gr1::CBAR_S1TS2Bypass::Get(i).FromValue(cbar.reg_value()), gr1_base_);
        break;
      case CBAR_Type::kS1TS2Fault:
        DumpReg(tag, gr1::CBAR_S1TS2Fault::Get(i).FromValue(cbar.reg_value()), gr1_base_);
        break;
      case CBAR_Type::kS1TS2Translate:
        DumpReg(tag, gr1::CBAR_S1TS2Translate::Get(i).FromValue(cbar.reg_value()), gr1_base_);
        break;
    }

    snprintf(tag, sizeof(tag), "CBA2R%u", i);
    const gr1::CBA2R cba2r = gr1::CBA2R::Get(i).ReadFrom(&gr1_base_);
    DumpReg(tag, cba2r, gr1_base_);

    snprintf(tag, sizeof(tag), "CBFRSYNRA%u", i);
    DumpReg(tag, gr1::CBFRSYNRA::Get(i).ReadFrom(&gr1_base_), gr1_base_);

    hwreg::RegisterMmio cb_base = get_cb_base(i);
    if (cbar.TYPE() != CBAR_Type::kS2Translation) {
      snprintf(tag, sizeof(tag), "SCTLR(%u)", i);
      DumpReg(tag, s1cbr::SCTLR::Get().ReadFrom(&cb_base), cb_base);

      s1cbr::TCR tcr = s1cbr::TCR::Get().ReadFrom(&cb_base);
      const AddrMode addr_mode =
          cba2r.VA64() ? AddrMode::k64Bit : (tcr.EAE() ? AddrMode::kExt32Bit : AddrMode::k32Bit);
      switch (addr_mode) {
        case AddrMode::k32Bit:
          snprintf(tag, sizeof(tag), "TCR(%u) (CBA2R.VA64 == 0, EAE == 0)", i);
          DumpReg(tag, s1cbr::TCR_32Bit::Get().FromValue(tcr.reg_value()), cb_base);

          snprintf(tag, sizeof(tag), "TTBR0(%u)", i);
          DumpReg(tag, s1cbr::TTBR0_32Bit::Get().ReadFrom(&cb_base), cb_base);

          snprintf(tag, sizeof(tag), "TTBR1(%u)", i);
          DumpReg(tag, s1cbr::TTBR1_32Bit::Get().ReadFrom(&cb_base), cb_base);
          break;
        case AddrMode::kExt32Bit:
          snprintf(tag, sizeof(tag), "TCR(%u) (CBA2R.VA64 == 0, EAE == 1)", i);
          DumpReg(tag, s1cbr::TCR_Ext32Bit::Get().FromValue(tcr.reg_value()), cb_base);

          snprintf(tag, sizeof(tag), "TTBR0(%u)", i);
          DumpReg(tag, s1cbr::TTBR0_Ext32Bit::Get().ReadFrom(&cb_base), cb_base);

          snprintf(tag, sizeof(tag), "TTBR1(%u)", i);
          DumpReg(tag, s1cbr::TTBR1_Ext32Bit::Get().ReadFrom(&cb_base), cb_base);
          break;
        case AddrMode::k64Bit:
          snprintf(tag, sizeof(tag), "TCR(%u) (CBA2R.VA64 == 1)", i);
          DumpReg(tag, s1cbr::TCR_64Bit::Get().FromValue(tcr.reg_value()), cb_base);

          snprintf(tag, sizeof(tag), "TTBR0(%u)", i);
          DumpReg(tag, s1cbr::TTBR0_64Bit::Get().ReadFrom(&cb_base), cb_base);

          snprintf(tag, sizeof(tag), "TTBR1(%u)", i);
          DumpReg(tag, s1cbr::TTBR1_64Bit::Get().ReadFrom(&cb_base), cb_base);
          break;
        default:
          snprintf(tag, sizeof(tag), "TCR(%u) (Bad AddrMode %u)", i,
                   static_cast<uint32_t>(addr_mode));
          DumpReg(tag, s1cbr::TCR::Get().FromValue(tcr.reg_value()), cb_base);
          break;
      }
    }
  }
}

int Smmu::CmdBtiOp(int argc, const cmd_args* argv, int cmd_ndx, BtiOpFunc op) {
  Guard<Mutex> guard{&lock_};

  ZX_DEBUG_ASSERT(cmd_ndx < argc);
  const int tgt_ndx = cmd_ndx + 1;

  if (tgt_ndx >= argc) {
    printf("Insufficient arguments for \"%s\" command\n", argv[cmd_ndx].str);
    return usage(ZX_ERR_INVALID_ARGS);
  }

  uint32_t found_ndx{0};
  zx::result<fbl::RefPtr<SmmuBti>> maybe_tgt =
      FindCmdTarget(argc - tgt_ndx, argv + tgt_ndx, &found_ndx);
  if (maybe_tgt.is_error()) {
    return maybe_tgt.status_value();
  }

  fbl::RefPtr<SmmuBti> bti_tgt = ktl::move(maybe_tgt.value());
  if (bti_tgt == nullptr) {
    uint32_t ndx = 0;
    for (SmmuBti& bti : bti_list_) {
      op(*this, bti, ndx++);
    }
  } else {
    op(*this, *bti_tgt, found_ndx);
  }

  return ZX_OK;
}

int Smmu::ConsoleCmd(int argc, const cmd_args* argv, uint32_t flags) {
  // We need at least two arguments to have a valid command.
  if (argc < 2) {
    printf("Bad argument count (%d).\n\n", argc);
    return usage(ZX_ERR_INVALID_ARGS);
  }

  enum class Cmd {
    Invalid,
    Help,
    List,
    SetDefault,
    Show,
    DumpRegs,
    Lock,
    InvalidateSids,
  };

  struct CmdMapEntry {
    const char* name;
    Cmd val;
  };

  // clang-format off
  constexpr ktl::array kCmdMap = {
      CmdMapEntry{"help", Cmd::Help},
      CmdMapEntry{"list", Cmd::List},
      CmdMapEntry{"set", Cmd::SetDefault},
      CmdMapEntry{"show", Cmd::Show},
      CmdMapEntry{"dumpregs", Cmd::DumpRegs},
      CmdMapEntry{"lock", Cmd::Lock},
      CmdMapEntry{"isids", Cmd::InvalidateSids},
  };
  // clang-format on

  // Find command in the argument list, and remember its index so that the
  // command handler can parse its positional arguments.  The command is the
  // first entry in the argument list which does not start with a "-".
  uint32_t cmd_ndx{0};
  Cmd cmd = Cmd::Invalid;
  for (int i = 1; i < argc; ++i) {
    const char* cmd_str = argv[i].str;
    if (cmd_str[0] != '-') {
      cmd_ndx = i;
      for (const CmdMapEntry& map_entry : kCmdMap) {
        if (!strcmp(cmd_str, map_entry.name)) {
          cmd = map_entry.val;
        }
      }

      if (cmd == Cmd::Invalid) {
        printf("Invalid command \"%s\"\n", cmd_str);
        return usage();
      }
      break;
    }
  }

  if (!cmd_ndx) {
    printf("Missing command!\n");
    return usage();
  }

  // If the command is anything but "help" or "list", then we are going to need
  // to find an SMMU to operate on.
  fbl::RefPtr<Smmu> chosen_smmu;
  if ((cmd != Cmd::Help) && (cmd != Cmd::List)) {
    if (const zx::result<ktl::optional<uint32_t>> maybe_id = FindOptionalU32(argc, argv, "--id");
        maybe_id.is_ok()) {
      uint32_t tgt_ndx = 0;

      // If we are attempting to set a new default, then use a value explicitly
      // passed via the `--id` option, if provided, or expect a positional
      // argument otherwise.
      if (cmd == Cmd::SetDefault) {
        if (maybe_id.value()) {
          tgt_ndx = maybe_id.value().value();
        } else {
          const uint32_t id_ndx = cmd_ndx + 1;
          if (static_cast<int64_t>(id_ndx) >= argc) {
            printf("No default SMMU ID specified.\n");
            return usage();
          }
          tgt_ndx = static_cast<uint32_t>(argv[id_ndx].u);
        }
      } else {
        tgt_ndx = maybe_id.value().value_or(gDefaultSmmuNdx.load());
      }

      // Lock and try to find the target SMMU.  Error out if we cannot.
      {
        Guard<Mutex> instance_guard{InstanceLock::Get()};
        uint32_t ndx{0};
        for (auto iter = instances_.begin(); iter.IsValid(); ++ndx, ++iter) {
          if (ndx == tgt_ndx) {
            chosen_smmu = iter.CopyPointer();
          }
        }
      }

      if (chosen_smmu == nullptr) {
        printf("Failed to find SMMU #%u\n", tgt_ndx);
        return usage();
      }

      // We found our SMMU.  If we are setting the default, update the global
      // default value and get out.  Otherwise, continue processing with our
      // chosen SMMU.
      if (cmd == Cmd::SetDefault) {
        printf("Default SMMU is now \"%s\".\n", chosen_smmu->name());
        gDefaultSmmuNdx.store(tgt_ndx);
        return ZX_OK;
      }
    } else {
      return usage(maybe_id.status_value());
    }
  }

  // Dispatch the command
  switch (cmd) {
    case Cmd::Help:
      return usage(ZX_OK);

    case Cmd::List: {
      Guard<Mutex> instance_guard{InstanceLock::Get()};
      printf("Discovered %zu SMMU instances\n", instances_.size_slow());
      uint32_t id = 0;
      for (const Smmu& smmu : instances_) {
        Guard<Mutex> guard{&smmu.lock_};
        printf("[%u] - %s (mode %s, user-bound : %s)\n", id++, smmu.name(),
               ArmSmmuModeToString(smmu.op_mode_), smmu.user_mode_bound_ ? "Yes" : "No");

        uint32_t bti_ndx = 0;
        for (const SmmuBti& bti : smmu.bti_list_) {
          ktl::array<char, 128> sid_buffer{0};
          char bti_name[ZX_MAX_NAME_LEN];
          bti.name_.get(ktl::size(bti_name), bti_name);

          Guard<Mutex> bti_pmt_guard{&bti.pmt_lock_};
          Guard<SpinLock, IrqSave> bti_guard{&bti.lock_};
          printf("    BTI[%u] - Mode %s%s StreamIDs [%s] Name \"%s\"\n", bti_ndx++,
                 BtiModeToString(bti.mode()), bti.orphaned_ ? " (orphaned)" : "",
                 bti.RenderSidList(sid_buffer), bti_name);
        }
      }
    } break;

    case Cmd::SetDefault:
      // Set default should have been handled earlier.
      return usage(ZX_ERR_INVALID_ARGS);

    case Cmd::Show:
      ZX_DEBUG_ASSERT(chosen_smmu != nullptr);
      return chosen_smmu->CmdShow(argc, argv, cmd_ndx);

    case Cmd::DumpRegs:
      ZX_DEBUG_ASSERT(chosen_smmu != nullptr);
      chosen_smmu->CmdDumpRegs();
      return ZX_OK;

    case Cmd::Lock: {
      ZX_DEBUG_ASSERT(chosen_smmu != nullptr);
      BtiOpFunc LockBti{[](const Smmu& smmu, SmmuBti& bti, uint32_t ndx) { bti.CmdLock(ndx); }};
      return chosen_smmu->CmdBtiOp(argc, argv, cmd_ndx, LockBti);
    } break;

    case Cmd::InvalidateSids: {
      ZX_DEBUG_ASSERT(chosen_smmu != nullptr);

      BtiOpFunc InvalidateSids{[](const Smmu& smmu, SmmuBti& bti, uint32_t ndx) {
        char bti_name[ZX_MAX_NAME_LEN];
        bti.name_.get(ktl::size(bti_name), bti_name);

        zx::result<> result = bti.InvalidateSids();
        if (result.is_ok()) {
          printf("%s: Stream IDs for BTI index #%u (\"%s\") have been invalidated.\n", smmu.name(),
                 ndx, bti_name);
        } else {
          printf("%s: Failed to invalidate Stream IDs for BTI index #%u (\"%s\") (err %d).\n",
                 smmu.name(), ndx, bti_name, result.error_value());
        }
      }};

      return chosen_smmu->CmdBtiOp(argc, argv, cmd_ndx, InvalidateSids);
    } break;

    default:
      // It should be impossible to have anything but a valid command at this
      // point.
      ASSERT(false);
      break;
  }

  return ZX_OK;
}

}  // namespace arm_smmu

STATIC_COMMAND_START
STATIC_COMMAND("smmu", "SMMUv2 commands", &arm_smmu::Smmu::ConsoleCmd)
STATIC_COMMAND_END(smmu)
