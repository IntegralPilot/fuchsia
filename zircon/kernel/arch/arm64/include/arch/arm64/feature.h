// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_ARM64_FEATURE_H_
#define ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_ARM64_FEATURE_H_

#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/compiler.h>
#include <zircon/features.h>

#include <arch/arm64.h>
#include <kernel/cpu.h>

enum arm64_microarch {
  UNKNOWN,

  ARM_CORTEX_A32,
  ARM_CORTEX_A35,
  ARM_CORTEX_A53,
  ARM_CORTEX_A55,
  ARM_CORTEX_A57,
  ARM_CORTEX_A65,
  ARM_CORTEX_A65AE,
  ARM_CORTEX_A72,
  ARM_CORTEX_A73,
  ARM_CORTEX_A75,
  ARM_CORTEX_A76,
  ARM_CORTEX_A76AE,
  ARM_CORTEX_A77,
  ARM_CORTEX_A78,
  ARM_CORTEX_A78AE,
  ARM_CORTEX_A78C,
  ARM_CORTEX_A510,
  ARM_CORTEX_A520,
  ARM_CORTEX_A710,
  ARM_CORTEX_A715,
  ARM_CORTEX_A720,
  ARM_CORTEX_A725,
  ARM_CORTEX_X1,
  ARM_CORTEX_X1C,
  ARM_CORTEX_X2,
  ARM_CORTEX_X3,
  ARM_CORTEX_X4,
  ARM_CORTEX_X925,
  ARM_NEOVERSE_E1,
  ARM_NEOVERSE_N1,
  ARM_NEOVERSE_N2,
  ARM_NEOVERSE_V1,

  CAVIUM_CN88XX,
  CAVIUM_CN99XX,

  APPLE_UNKNOWN,

  AMPERE_UNKNOWN,
  AMPERE_1,
  AMPERE_1A,

  QEMU_TCG,
};

enum arm64_microarch midr_to_microarch(uint64_t midr);

extern uint32_t arm64_isa_features;
extern bool feat_pmuv3_enabled;

inline bool arm64_feature_test(uint32_t feature) { return arm64_isa_features & feature; }

struct arm64_cache_features {
  // block size of the dc zva instruction, dcache cache line and icache cache line
  uint32_t zva_size;
  uint32_t icache_size;
  uint32_t dcache_size;

  // Broken out versions of CTR_EL0 IDC and DIC
  bool idc;  // Instruction-to-data coherence
  bool dic;  // Data-to-Instruction coherence

  // PIPT or VIPT?
  bool pipt;  // PIPT is the newer, desired version
};

extern arm64_cache_features arm64_cache_features;

// size of the asid
enum class arm64_asid_width {
  UNKNOWN,  // invalid, should be set prior to anything actually using it
  ASID_8,
  ASID_16,
};

struct arm64_mmu_features {
  arm64_asid_width asid_width;

  // supported stage1 page granules
  bool s1_page_4k;
  bool s1_page_16k;
  bool s1_page_64k;

  // privileged access never and related user access override
  bool pan;
  bool uao;

  // accessed and dirty bits
  bool accessed_bit;
  bool dirty_bit;

  // extended CCSIDR register format
  bool ccsidx;

  // virtualization features
  bool vhe;
  bool e2h0;
  arm64_asid_width vmid_width;
};

// the global feature structure for mmu features
extern arm64_mmu_features arm64_mmu_features;

inline enum arm64_asid_width arm64_asid_width() {
  DEBUG_ASSERT(arm64_mmu_features.asid_width != arm64_asid_width::UNKNOWN);
  return arm64_mmu_features.asid_width;
}

inline enum arm64_asid_width arm64_vmid_width() {
  DEBUG_ASSERT(arm64_mmu_features.vmid_width != arm64_asid_width::UNKNOWN);
  return arm64_mmu_features.vmid_width;
}

// call on every cpu to initialize the feature set
void arm64_feature_init();

// dump the feature set
void arm64_feature_debug(bool full);

// Returns true if the current CPU is the first member of a cluster according to
// MPIDR.
bool arm64_feature_current_is_first_in_cluster();

void arm64_print_midr_cpu_name(FILE*);

#endif  // ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_ARM64_FEATURE_H_
