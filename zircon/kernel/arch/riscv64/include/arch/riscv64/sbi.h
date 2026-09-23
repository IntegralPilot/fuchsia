// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_SBI_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_SBI_H_

#include <lib/arch/riscv64/sbi-call.h>
#include <lib/arch/riscv64/sbi.h>
#include <stdint.h>

#include <arch/riscv64.h>
#include <dev/power.h>

extern "C" {
// Init functions
void riscv64_sbi_early_init();
void riscv64_sbi_init();

arch::RiscvSbiRet sbi_send_ipi(arch::HartMask mask, arch::HartMaskBase mask_base);
zx_status_t sbi_hart_start(uint64_t hart_id, paddr_t start_addr, uint64_t priv);
zx_status_t sbi_hart_stop();
zx_status_t sbi_get_cpu_state(uint64_t hart_id, power_cpu_state* out_state);
arch::RiscvSbiRet sbi_remote_fencei(cpu_mask_t cpu_mask);
arch::RiscvSbiRet sbi_remote_sfence_vma(cpu_mask_t cpu_mask, uintptr_t start, uintptr_t size);
arch::RiscvSbiRet sbi_remote_sfence_vma_asid(cpu_mask_t cpu_mask, uintptr_t start, uintptr_t size,
                                             uint64_t asid);

// Attempt to shut down the system via the system reset extension.
zx_status_t sbi_shutdown();

// Attempt to reset down the system via the system reset extension.
zx_status_t sbi_reset();
}

// Various wrappers for SBI routines used in the kernel.
// Mostly follows the SBI api, documented at
// https://github.com/riscv-non-isa/riscv-sbi-doc/blob/master/riscv-sbi.adoc
inline arch::RiscvSbiRet sbi_set_timer(uint64_t stime_value) {
  return arch::RiscvSbi::SetTimer(stime_value);
}

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_SBI_H_
