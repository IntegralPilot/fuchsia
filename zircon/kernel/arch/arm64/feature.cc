// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "arch/arm64/feature.h"

#include <bits.h>
#include <ctype.h>
#include <inttypes.h>
#include <lib/arch/arm64/cache.h>
#include <lib/arch/arm64/feature.h>
#include <lib/arch/intrin.h>

#include <arch/arm64.h>
#include <fbl/algorithm.h>
#include <kernel/cpu.h>
#include <ktl/iterator.h>

#include <ktl/enforce.h>

// saved instruction set feature bitmap
uint32_t arm64_isa_features;

// Whether FEAT_PMUv3 is implemented.
bool feat_pmuv3_enabled;

// MMU features
struct arm64_mmu_features arm64_mmu_features;

// Cache size parameters cpus, default to a reasonable minimum
struct arm64_cache_features arm64_cache_features = {
    .zva_size = 32,
    .icache_size = 32,
    .dcache_size = 32,

    .idc = false,
    .dic = false,
    .pipt = false,
};

// Save another global version, used directly in some assembly
extern uint32_t arm64_dcache_size;
uint32_t arm64_dcache_size = 32;

namespace {
struct arm64_cache_desc {
  uint8_t ctype;
  uint32_t num_sets;
  uint32_t associativity;
  uint32_t line_size;
};

struct arm64_cache_info {
  // from CLIDR_EL1
  uint8_t inner_boundary;
  uint8_t lou_u;
  uint8_t loc;
  uint8_t lou_is;
  // from CTR_EL0
  uint8_t imin_line;
  uint8_t dmin_line;
  uint8_t cache_writeback_granule;
  uint8_t l1_instruction_cache_policy;
  bool idc;  // requires icache invalidate to pou for instruction to data coherence
  bool dic;  // requires data clean to pou for data to instruction coherence
  // via iterating each cache level
  arm64_cache_desc level_data_type[7];
  arm64_cache_desc level_inst_type[7];
};

arm64_cache_info cache_info[SMP_MAX_CPUS];

void arm64_get_cache_info(arm64_cache_info* info) {
  *info = {};

  uint64_t clidr = __arm_rsr64("clidr_el1");
  info->inner_boundary = static_cast<uint8_t>(BITS_SHIFT(clidr, 32, 30));
  info->lou_u = static_cast<uint8_t>(BITS_SHIFT(clidr, 29, 27));
  info->loc = static_cast<uint8_t>(BITS_SHIFT(clidr, 26, 24));
  info->lou_is = static_cast<uint8_t>(BITS_SHIFT(clidr, 23, 21));

  uint64_t ctr = __arm_rsr64("ctr_el0");
  info->imin_line = static_cast<uint8_t>(BITS(ctr, 3, 0));
  info->dmin_line = static_cast<uint8_t>(BITS_SHIFT(ctr, 19, 16));
  info->l1_instruction_cache_policy = static_cast<uint8_t>(BITS_SHIFT(ctr, 15, 14));
  info->cache_writeback_granule = static_cast<uint8_t>(BITS_SHIFT(ctr, 27, 24));
  info->idc = BIT(ctr, 28);
  info->dic = BIT(ctr, 29);

  auto process_ccsid = [info](size_t level, uint8_t ctype) {
    const bool instruction = ctype & 0x1;

    __arm_wsr64("csselr_el1", (level << 1) | (instruction ? 1 : 0));  // Select cache level
    __isb(ARM_MB_SY);

    arm64_cache_desc* desc =
        instruction ? &info->level_inst_type[level] : &info->level_data_type[level];
    desc->ctype = ctype;

    const uint64_t ccsid = __arm_rsr64("ccsidr_el1");
    if (arm64_mmu_features.ccsidx) {
      // Parse the newer extended ccsid format
      desc->num_sets = static_cast<uint32_t>(BITS_SHIFT(ccsid, 55, 32) + 1);
      desc->associativity = static_cast<uint32_t>(BITS_SHIFT(ccsid, 23, 3) + 1);
      desc->line_size = 1u << (BITS(ccsid, 2, 0) + 4);
    } else {
      desc->num_sets = static_cast<uint32_t>(BITS_SHIFT(ccsid, 27, 13) + 1);
      desc->associativity = static_cast<uint32_t>(BITS_SHIFT(ccsid, 12, 3) + 1);
      desc->line_size = 1u << (BITS(ccsid, 2, 0) + 4);
    }
  };

  for (size_t i = 0; i < 7; i++) {
    uint8_t ctype = (clidr >> (3 * i)) & 0x07;
    if (ctype == 0) {
      // No more valid ctypes after this
      break;
    }

    if (ctype == 4) {  // Unified
      process_ccsid(i, ctype);
    } else {
      if (ctype & 0x2) {  // Data cache
        process_ccsid(i, ctype & 0x2);
      }
      if (ctype & 0x1) {  // Instruction cache
        process_ccsid(i, ctype & 0x1);
      }
    }
  }
}

void arm64_dump_cache_info(cpu_num_t cpu) {
  arm64_cache_info* info = &(cache_info[cpu]);
  printf("==== ARM64 CACHE INFO CORE %u ====\n", cpu);
  printf("Inner Boundary = L%u\n", info->inner_boundary);
  printf("Level of Unification Uniprocessor = L%u\n", info->lou_u);
  printf("Level of Coherence = L%u\n", info->loc);
  printf("Level of Unification Inner Shareable = L%u\n", info->lou_is);
  printf("Instruction/Data cache minimum line = %u/%u\n", (1U << info->imin_line) * 4,
         (1U << info->dmin_line) * 4);
  printf("Cache Writeback Granule = %u\n", (1U << info->cache_writeback_granule) * 4);
  if (arm64_mmu_features.ccsidx) {
    dprintf(INFO, "Extended CCSIDR format\n");
  }
  const char* icp = "";
  switch (info->l1_instruction_cache_policy) {
    case 0:
      icp = "VPIPT";
      break;
    case 1:
      icp = "AIVIVT";
      break;
    case 2:
      icp = "VIPT";
      break;
    case 3:
      icp = "PIPT";
      break;
  }
  printf("L1 Instruction cache policy = %s, ", icp);
  printf("IDC = %i, DIC = %i\n", info->idc, info->dic);
  for (int i = 0; i < 7; i++) {
    if ((info->level_data_type[i].ctype == 0) && (info->level_inst_type[i].ctype == 0)) {
      break;  // not implemented
    }
    printf("L%d Details:", i + 1);
    if (info->level_data_type[i].ctype == 4) {
      printf("\tUnified Cache, sets=%u, associativity=%u, line size=%u bytes\n",
             info->level_data_type[i].num_sets, info->level_data_type[i].associativity,
             info->level_data_type[i].line_size);
    } else {
      if (info->level_data_type[i].ctype & 0x02) {
        printf("\tData Cache, sets=%u, associativity=%u, line size=%u bytes\n",
               info->level_data_type[i].num_sets, info->level_data_type[i].associativity,
               info->level_data_type[i].line_size);
      }
      if (info->level_inst_type[i].ctype & 0x01) {
        if (info->level_data_type[i].ctype & 0x02) {
          printf("\t");
        }
        printf("\tInstruction Cache, sets=%u, associativity=%u, line size=%u bytes\n",
               info->level_inst_type[i].num_sets, info->level_inst_type[i].associativity,
               info->level_inst_type[i].line_size);
      }
    }
  }
}

}  // namespace

enum arm64_microarch midr_to_microarch(uint64_t midr) {
  uint64_t implementer = BITS_SHIFT(midr, 31, 24);
  uint64_t partnum = BITS_SHIFT(midr, 15, 4);

  if (implementer == 'A') {
    // ARM cores
    switch (partnum) {
      case 0xd01:
        return ARM_CORTEX_A32;
      case 0xd03:
        return ARM_CORTEX_A53;
      case 0xd04:
        return ARM_CORTEX_A35;
      case 0xd05:
        return ARM_CORTEX_A55;
      case 0xd06:
        return ARM_CORTEX_A65;
      case 0xd07:
        return ARM_CORTEX_A57;
      case 0xd08:
        return ARM_CORTEX_A72;
      case 0xd09:
        return ARM_CORTEX_A73;
      case 0xd0a:
        return ARM_CORTEX_A75;
      case 0xd0b:
        return ARM_CORTEX_A76;
      case 0xd0c:
        return ARM_NEOVERSE_N1;
      case 0xd0d:
        return ARM_CORTEX_A77;
      case 0xd0e:
        return ARM_CORTEX_A76AE;
      case 0xd40:
        return ARM_NEOVERSE_V1;
      case 0xd41:
        return ARM_CORTEX_A78;
      case 0xd42:
        return ARM_CORTEX_A78AE;
      case 0xd43:
        return ARM_CORTEX_A65AE;
      case 0xd44:
        return ARM_CORTEX_X1;
      case 0xd46:
        return ARM_CORTEX_A510;
      case 0xd47:
        return ARM_CORTEX_A710;
      case 0xd48:
        return ARM_CORTEX_X2;
      case 0xd49:
        return ARM_NEOVERSE_N2;
      case 0xd4a:
        return ARM_NEOVERSE_E1;
      case 0xd4b:
        return ARM_CORTEX_A78C;
      case 0xd4c:
        return ARM_CORTEX_X1C;
      case 0xd4d:
        return ARM_CORTEX_A715;
      case 0xd4e:
        return ARM_CORTEX_X3;
      case 0xd80:
        return ARM_CORTEX_A520;
      case 0xd81:
        return ARM_CORTEX_A720;
      case 0xd82:
        return ARM_CORTEX_X4;
      case 0xd85:
        return ARM_CORTEX_X925;
      case 0xd87:
        return ARM_CORTEX_A725;
      default:
        return UNKNOWN;
    }
  } else if (implementer == 'C') {
    // Cavium
    switch (partnum) {
      case 0xa1:
        return CAVIUM_CN88XX;
      case 0xaf:
        return CAVIUM_CN99XX;
      default:
        return UNKNOWN;
    }
  } else if (implementer == 0x61) {
    // Apple
    // For the moment, qemu via HVF does not seem to return
    // a meaningful part number.
    return APPLE_UNKNOWN;
  } else if (implementer == 0xc0) {
    // Ampere
    switch (partnum) {
      case 0xac3:
        return AMPERE_1;
      case 0xac4:
        return AMPERE_1A;
      default:
        return AMPERE_UNKNOWN;
    }
  } else if (implementer == 0) {
    // software implementation
    switch (partnum) {
      case 0x51:
        return QEMU_TCG;
      default:
        return UNKNOWN;
    }
  } else {
    return UNKNOWN;
  }
}

namespace {

void midr_to_core_string(uint64_t midr, char* str, size_t len) {
  auto microarch = midr_to_microarch(midr);
  uint64_t implementer = BITS_SHIFT(midr, 31, 24);
  uint64_t variant = BITS_SHIFT(midr, 23, 20);
  [[maybe_unused]] uint64_t architecture = BITS_SHIFT(midr, 19, 16);
  uint64_t partnum = BITS_SHIFT(midr, 15, 4);
  uint64_t revision = BITS_SHIFT(midr, 3, 0);

  const char* partnum_str = "unknown";
  switch (microarch) {
    case ARM_CORTEX_A32:
      partnum_str = "ARM Cortex-A32";
      break;
    case ARM_CORTEX_A35:
      partnum_str = "ARM Cortex-A35";
      break;
    case ARM_CORTEX_A53:
      partnum_str = "ARM Cortex-A53";
      break;
    case ARM_CORTEX_A55:
      partnum_str = "ARM Cortex-A55";
      break;
    case ARM_CORTEX_A57:
      partnum_str = "ARM Cortex-A57";
      break;
    case ARM_CORTEX_A65:
      partnum_str = "ARM Cortex-A65";
      break;
    case ARM_CORTEX_A65AE:
      partnum_str = "ARM Cortex-A65AE";
      break;
    case ARM_CORTEX_A72:
      partnum_str = "ARM Cortex-A72";
      break;
    case ARM_CORTEX_A73:
      partnum_str = "ARM Cortex-A73";
      break;
    case ARM_CORTEX_A75:
      partnum_str = "ARM Cortex-A75";
      break;
    case ARM_CORTEX_A76:
      partnum_str = "ARM Cortex-A76";
      break;
    case ARM_CORTEX_A76AE:
      partnum_str = "ARM Cortex-A76AE";
      break;
    case ARM_CORTEX_A77:
      partnum_str = "ARM Cortex-A77";
      break;
    case ARM_CORTEX_A78:
      partnum_str = "ARM Cortex-A78";
      break;
    case ARM_CORTEX_A78AE:
      partnum_str = "ARM Cortex-A78AE";
      break;
    case ARM_CORTEX_A78C:
      partnum_str = "ARM Cortex-A78C";
      break;
    case ARM_CORTEX_A510:
      partnum_str = "ARM Cortex-A510";
      break;
    case ARM_CORTEX_A520:
      partnum_str = "ARM Cortex-A520";
      break;
    case ARM_CORTEX_A710:
      partnum_str = "ARM Cortex-A710";
      break;
    case ARM_CORTEX_A715:
      partnum_str = "ARM Cortex-A715";
      break;
    case ARM_CORTEX_A720:
      partnum_str = "ARM Cortex-A720";
      break;
    case ARM_CORTEX_A725:
      partnum_str = "ARM Cortex-A725";
      break;
    case ARM_CORTEX_X1:
      partnum_str = "ARM Cortex-X1";
      break;
    case ARM_CORTEX_X1C:
      partnum_str = "ARM Cortex-X1C";
      break;
    case ARM_CORTEX_X2:
      partnum_str = "ARM Cortex-X2";
      break;
    case ARM_CORTEX_X3:
      partnum_str = "ARM Cortex-X3";
      break;
    case ARM_CORTEX_X4:
      partnum_str = "ARM Cortex-X4";
      break;
    case ARM_CORTEX_X925:
      partnum_str = "ARM Cortex-X925";
      break;
    case ARM_NEOVERSE_E1:
      partnum_str = "ARM Neoverse E1";
      break;
    case ARM_NEOVERSE_N1:
      partnum_str = "ARM Neoverse N1";
      break;
    case ARM_NEOVERSE_N2:
      partnum_str = "ARM Neoverse N2";
      break;
    case ARM_NEOVERSE_V1:
      partnum_str = "ARM Neoverse V1";
      break;
    case CAVIUM_CN88XX:
      partnum_str = "Cavium CN88XX";
      break;
    case CAVIUM_CN99XX:
      partnum_str = "Cavium CN99XX";
      break;
    case APPLE_UNKNOWN:
      partnum_str = "Unknown Apple Silicon";
      break;
    case AMPERE_UNKNOWN:
      partnum_str = "Unkown Ampere CPU";
      break;
    case AMPERE_1:
      partnum_str = "Ampere 1";
      break;
    case AMPERE_1A:
      partnum_str = "Ampere 1A";
      break;
    case QEMU_TCG:
      partnum_str = "QEMU TCG";
      break;
    case UNKNOWN: {
      const char i = isprint(static_cast<int>(implementer)) ? static_cast<char>(implementer) : '0';
      snprintf(str, len, "Unknown implementer %c partnum %#lx r%lup%lu", i, partnum, variant,
               revision);
      return;
    }
  }

  snprintf(str, len, "%s r%lup%lu", partnum_str, variant, revision);
}
}  // namespace

bool arm64_feature_current_is_first_in_cluster() {
  const uint64_t mpidr = __arm_rsr64("mpidr_el1");

  // Note: this test is not strictly correct, since the affinity fields really do not encode the
  // cluster layout of the cores anymore. Since about cortex-a75 and somewhere in the mid armv8
  // spec, the layout of these fields has been somewhat retconned to mean whatever the vendor wants,
  // and true cluster layout comes from external sources (such as device tree). However, the MT
  // bit says whether or not AFF0 is purely listing threads within a cpu, and all new cores have
  // it set all the time, even if they're not SMT capable.
  if (mpidr & MPIDR_MT) {
    return ((mpidr & MPIDR_AFF1_MASK) >> MPIDR_AFF1_SHIFT) == 0;
  }
  return ((mpidr & MPIDR_AFF0_MASK) >> MPIDR_AFF0_SHIFT) == 0;
}

// call on every cpu to save features
void arm64_feature_init() {
  // set up some global constants based on the boot cpu
  cpu_num_t cpu = arch_curr_cpu_num();
  if (cpu == 0) {
    // read the block size of DC ZVA
    auto dczid = arch::ArmDataCacheZeroIdEl0::Read();
    arm64_cache_features.zva_size = 0;
    if (!dczid.dzp()) {
      arm64_cache_features.zva_size = static_cast<uint32_t>(dczid.zva_line_size());
    }
    ASSERT(arm64_cache_features.zva_size > 0);

    // Read the dcache and icache line size
    auto ctr = arch::ArmCacheTypeEl0::Read();
    arm64_cache_features.dcache_size = static_cast<uint32_t>(ctr.dcache_line_size());
    arm64_cache_features.icache_size = static_cast<uint32_t>(ctr.icache_line_size());
    arm64_dcache_size = arm64_cache_features.dcache_size;

    // Read instruction and data cache coherence feature bits.
    arm64_cache_features.dic = ctr.dic();
    arm64_cache_features.idc = ctr.idc();

    // Read the icache type. Note: according to the spec only VIPT and PIPT are valid options,
    // so just store whether or not it's PIPT and if a new value appears, fall back
    // to assuming VIPT.
    arm64_cache_features.pipt = ctr.l1_ip() == arch::ArmL1ICachePolicy::PIPT;

    // parse the ISA feature bits
    arm64_isa_features |= ZX_HAS_CPU_FEATURES;

    auto isar0 = arch::ArmIdAa64IsaR0El1::Read();

    // By D13.1.3 "Principles of the ID scheme for fields in ID registers", it
    // is safe to assume that values assigned in the future will describe
    // supersets of the existing options.
    switch (isar0.aes()) {
      default:
      case arch::ArmIdAa64IsaR0El1::Aes::kPmull:
        arm64_isa_features |= ZX_ARM64_FEATURE_ISA_PMULL;
        [[fallthrough]];
      case arch::ArmIdAa64IsaR0El1::Aes::kAes:
        arm64_isa_features |= ZX_ARM64_FEATURE_ISA_AES;
        break;
      case arch::ArmIdAa64IsaR0El1::Aes::kNone:
        break;
    }

    if (isar0.sha1() != arch::ArmIdAa64IsaR0El1::Sha1::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_SHA1;
    }
    switch (isar0.sha2()) {
      default:
      case arch::ArmIdAa64IsaR0El1::Sha2::kSha512:
        arm64_isa_features |= ZX_ARM64_FEATURE_ISA_SHA512;
        [[fallthrough]];
      case arch::ArmIdAa64IsaR0El1::Sha2::kSha256:
        arm64_isa_features |= ZX_ARM64_FEATURE_ISA_SHA256;
        break;
      case arch::ArmIdAa64IsaR0El1::Sha2::kNone:
        break;
    }
    if (isar0.crc32() != arch::ArmIdAa64IsaR0El1::Crc32::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_CRC32;
    }
    if (isar0.atomic() != arch::ArmIdAa64IsaR0El1::Atomic::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_ATOMICS;
    }
    if (isar0.rdm() != arch::ArmIdAa64IsaR0El1::Rdm::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_RDM;
    }
    if (isar0.sha3() != arch::ArmIdAa64IsaR0El1::Sha3::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_SHA3;
    }
    if (isar0.sm3() != arch::ArmIdAa64IsaR0El1::Sm3::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_SM3;
    }
    if (isar0.sm4() != arch::ArmIdAa64IsaR0El1::Sm4::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_SM4;
    }
    if (isar0.dp() != arch::ArmIdAa64IsaR0El1::DotProd::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_DP;
    }
    if (isar0.fhm() != arch::ArmIdAa64IsaR0El1::Fhm::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_FHM;
    }
    if (isar0.ts() != arch::ArmIdAa64IsaR0El1::Ts::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_TS;
    }
    if (isar0.rndr() != arch::ArmIdAa64IsaR0El1::Rndr::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_RNDR;
    }

    auto isar1 = arch::ArmIdAa64IsaR1El1::Read();
    if (isar1.dpb() != arch::ArmIdAa64IsaR1El1::Dpb::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_DPB;
    }
    if (isar1.i8mm() != arch::ArmIdAa64IsaR1El1::I8mm::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_I8MM;
    }

    auto isar2 = arch::ArmIdAa64IsaR2El1::Read();
    if (isar2.mops() != arch::ArmIdAa64IsaR2El1::Mops::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_MOPS;
    }

    auto pfr0 = arch::ArmIdAa64Pfr0El1::Read();
    if (pfr0.el0() == arch::ArmIdAa64Pfr0El1::El::k32) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_ARM32;
    }
    if (pfr0.fp() != arch::ArmIdAa64Pfr0El1::Fp::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_FP;
    }
    if (pfr0.advsimd() != arch::ArmIdAa64Pfr0El1::Fp::kNone) {
      arm64_isa_features |= ZX_ARM64_FEATURE_ISA_ASIMD;
    }
    if (pfr0.sve() != arch::ArmIdAa64Pfr0El1::Sve::kNone) {
      // TODO(https://fxbug.dev/42081850): We do not currently report
      // ZX_ARM64_FEATURE_ISA_SVE even if the processor supports the feature
      // because Zircon does not have all the supporting code yet.
    }

    auto mmfr0 = arch::ArmIdAa64Mmfr0El1::Read();

    // check the size of the asid
    switch (mmfr0.asid_bits()) {
      default:
        printf("ARM: warning, unrecognized ASID width value (%u) in ID_AA64MMFR0_EL1\n",
               static_cast<uint32_t>(mmfr0.asid_bits()));
        // default to 8 bits
        [[fallthrough]];
      case arch::ArmAsidSize::k8bits:
        arm64_mmu_features.asid_width = arm64_asid_width::ASID_8;
        break;
      case arch::ArmAsidSize::k16bits:
        arm64_mmu_features.asid_width = arm64_asid_width::ASID_16;
        break;
    }

    // Read the supported stage 1 page granularities.
    if (mmfr0.tgran4() != 0b1111) {
      arm64_mmu_features.s1_page_4k = true;
    }
    if (mmfr0.tgran16() != 0) {
      arm64_mmu_features.s1_page_16k = true;
    }
    if (mmfr0.tgran64() != 0b1111) {
      arm64_mmu_features.s1_page_64k = true;
    }

    auto mmfr1 = arch::ArmIdAa64Mmfr1El1::Read();

    // See about A and D bits in page tables.
    switch (mmfr1.hafdbs()) {
      default:
      case 2:
        arm64_mmu_features.dirty_bit = true;
        [[fallthrough]];
      case 1:
        arm64_mmu_features.accessed_bit = true;
        break;
      case 0:
        // No A or D feature implemented
        break;
    }

    // Check for PAN features
    if (mmfr1.pan() != 0) {
      arm64_mmu_features.pan = true;

      // >1 values also determine support for various
      // address translation instruction variants with PAN.
    }

    // Check for VHE features.
    if (mmfr1.vh() != 0) {
      arm64_mmu_features.vhe = true;
    }

    // Check the size of the VMID.
    switch (mmfr1.vmid_bits()) {
      default:
        printf("ARM: warning, unrecognized VMID width value (%u) in ID_AA64MMFR1_EL1\n",
               static_cast<uint32_t>(mmfr0.asid_bits()));
        // default to 8 bits
        [[fallthrough]];
      case arch::ArmAsidSize::k8bits:
        arm64_mmu_features.vmid_width = arm64_asid_width::ASID_8;
        break;
      case arch::ArmAsidSize::k16bits:
        arm64_mmu_features.vmid_width = arm64_asid_width::ASID_16;
        break;
    }

    auto mmfr2 = arch::ArmIdAa64Mmfr2El1::Read();

    // Check for User Access Override
    if (mmfr2.uao() != 0) {
      arm64_mmu_features.uao = true;
    }
    if (mmfr2.ccidx() != 0) {
      arm64_mmu_features.ccsidx = true;
    }

    // Check if FEAT_PMUv3 is enabled.
    uint64_t pmu_version = (__arm_rsr64("id_aa64dfr0_el1") >> 8) & 0xf;
    feat_pmuv3_enabled = pmu_version > 0b0000 && pmu_version < 0b1111;

    auto mmfr4 = arch::ArmIdAa64Mmfr4El1::Read();

    // Check for E2H0 features.
    if (mmfr4.e2h0() == 0) {
      arm64_mmu_features.e2h0 = true;
    }
  }

  // read the cache info for each cpu
  arm64_get_cache_info(&(cache_info[cpu]));
}

namespace {

void print_cpu_info() {
  uint64_t midr = __arm_rsr64("midr_el1");
  char cpu_name[128];
  midr_to_core_string(midr, cpu_name, sizeof(cpu_name));

  uint64_t mpidr = __arm_rsr64("mpidr_el1");

  dprintf(INFO, "ARM cpu %u: midr %#lx '%s' mpidr %#" PRIx64 " aff %u:%u:%u:%u\n",
          arch_curr_cpu_num(), midr, cpu_name, mpidr,
          (uint32_t)((mpidr & MPIDR_AFF3_MASK) >> MPIDR_AFF3_SHIFT),
          (uint32_t)((mpidr & MPIDR_AFF2_MASK) >> MPIDR_AFF2_SHIFT),
          (uint32_t)((mpidr & MPIDR_AFF1_MASK) >> MPIDR_AFF1_SHIFT),
          (uint32_t)((mpidr & MPIDR_AFF0_MASK) >> MPIDR_AFF0_SHIFT));
}

void print_isa_features() {
  constexpr struct {
    uint32_t bit;
    const char* name;
  } kFeatures[] = {
      {ZX_ARM64_FEATURE_ISA_FP, "fp"},           {ZX_ARM64_FEATURE_ISA_ASIMD, "asimd"},
      {ZX_ARM64_FEATURE_ISA_AES, "aes"},         {ZX_ARM64_FEATURE_ISA_PMULL, "pmull"},
      {ZX_ARM64_FEATURE_ISA_SHA1, "sha1"},       {ZX_ARM64_FEATURE_ISA_SHA256, "sha256"},
      {ZX_ARM64_FEATURE_ISA_SHA512, "sha512"},   {ZX_ARM64_FEATURE_ISA_CRC32, "crc32"},
      {ZX_ARM64_FEATURE_ISA_ATOMICS, "atomics"}, {ZX_ARM64_FEATURE_ISA_RDM, "rdm"},
      {ZX_ARM64_FEATURE_ISA_SHA3, "sha3"},       {ZX_ARM64_FEATURE_ISA_SM3, "sm3"},
      {ZX_ARM64_FEATURE_ISA_SM4, "sm4"},         {ZX_ARM64_FEATURE_ISA_DP, "dp"},
      {ZX_ARM64_FEATURE_ISA_DPB, "dpb"},         {ZX_ARM64_FEATURE_ISA_FHM, "fhm"},
      {ZX_ARM64_FEATURE_ISA_TS, "ts"},           {ZX_ARM64_FEATURE_ISA_RNDR, "rndr"},
      {ZX_ARM64_FEATURE_ISA_I8MM, "i8mm"},       {ZX_ARM64_FEATURE_ISA_SVE, "sve"},
      {ZX_ARM64_FEATURE_ISA_ARM32, "arm32"},     {ZX_ARM64_FEATURE_ISA_MOPS, "mops"},
  };

  uint line = 0;
  uint col = 0;
  for (const auto& feature : kFeatures) {
    if (arm64_feature_test(feature.bit)) {
      if (col == 0) {
        if (line == 0) {
          col += printf("ARM ISA Features: ");
        } else {
          col += printf("                  ");
        }
      }
      col += printf("%s ", feature.name);
      if (col >= 100) {
        printf("\n");
        col = 0;
        line++;
      }
    }
  }

  if (col > 0) {
    printf("\n");
  }
}

}  // namespace

// dump the feature set
// print additional information if full is passed
void arm64_feature_debug(bool full) {
  print_cpu_info();

  if (full) {
    print_isa_features();
    dprintf(INFO, "ARM ASID width %s\n",
            (arm64_asid_width() == arm64_asid_width::ASID_16) ? "16" : "8");
    dprintf(INFO, "ARM Supported S1 Page sizes (4k/16k/64k): %d/%d/%d\n",
            arm64_mmu_features.s1_page_4k, arm64_mmu_features.s1_page_16k,
            arm64_mmu_features.s1_page_64k);
    dprintf(INFO, "ARM accessed bit %d, dirty bit %d\n", arm64_mmu_features.accessed_bit,
            arm64_mmu_features.dirty_bit);
    dprintf(INFO, "ARM PAN %d, UAO %d\n", arm64_mmu_features.pan, arm64_mmu_features.uao);
    dprintf(INFO, "ARM cache line sizes: icache %u dcache %u zva %u\n",
            arm64_cache_features.icache_size, arm64_cache_features.dcache_size,
            arm64_cache_features.zva_size);
    dprintf(INFO, "ARM cache I/D coherence: I2D %i D2I %i\n", arm64_cache_features.idc,
            arm64_cache_features.dic);
    dprintf(INFO, "ARM icache type: %s\n", arm64_cache_features.pipt ? "PIPT" : "VIPT");
    dprintf(INFO, "ARM VMID width %s\n",
            (arm64_asid_width() == arm64_asid_width::ASID_16) ? "16" : "8");
    dprintf(INFO, "ARM VHE %d, E2H0 %d\n", arm64_mmu_features.vhe, arm64_mmu_features.e2h0);
    if (DPRINTF_ENABLED_FOR_LEVEL(INFO)) {
      arm64_dump_cache_info(arch_curr_cpu_num());
    }
  }
}

void arm64_print_midr_cpu_name(FILE* stream) {
  uint64_t midr = __arm_rsr64("midr_el1");
  char cpu_name[128];
  midr_to_core_string(midr, cpu_name, sizeof(cpu_name));
  stream->Write(cpu_name);
}
