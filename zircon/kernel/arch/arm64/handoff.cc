// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/system.h>
#include <lib/arch/cache.h>
#include <lib/arch/intrin.h>

#include <arch/arm64/mmu.h>
#include <arch/arm64/mp.h>
#include <phys/handoff.h>
#include <vm/handoff-end.h>

uint64_t arm64_tcr_ips;

void ArchPostHandoffBootstrap(const ArchPhysHandoff* arch_handoff) {
  const auto ips = arch::ArmTcrEl1::Read().ips();
  ZX_ASSERT(ips <= arch::ArmPhysicalAddressSize::k48Bits);
  ZX_ASSERT(ips <= arch::ArmIdAa64Mmfr0El1::Read().pa_range());
  arm64_tcr_ips = MMU_TCR_IPS(static_cast<uint64_t>(ips));

  // Clear any phys exception handlers.
  arch::ArmVbarEl1::Write(uintptr_t{0});

  // Disable trampoline page-table in ttbr0
  arch::ArmTcrEl1::Write(Arm64MmuTcrFlags(MMU_TCR_FLAGS_KERNEL));

  // Invalidate the entire TLB
  arch::InvalidateLocalTlbs();
  __dsb(ARM_MB_SY);
  __isb(ARM_MB_SY);

  // save the mpid for cpu 0
  uint64_t mpidr = __arm_rsr64("mpidr_el1");
  arch_register_mpid(0, mpidr & ARM64_MPIDR_MASK);

  // set the per cpu pointer for cpu 0
  arm64_init_percpu_early();
}
