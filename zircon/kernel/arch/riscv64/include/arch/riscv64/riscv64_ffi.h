// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_RISCV64_FFI_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_RISCV64_FFI_H_

#include <stdint.h>
#include <sys/types.h>
#include <zircon/syscalls/debug.h>

extern "C" {

// Implemented in Rust (arch.rs); backs ArchIdlePowerThread::EnterIdleState().
void arch_enter_idle_state();

bool cpp_boot_options_riscv64_enable_asid();

zx_status_t cpp_riscv64_get_general_regs(zx_thread_state_general_regs_t* regs);
zx_status_t cpp_riscv64_set_general_regs(const zx_thread_state_general_regs_t* regs);

uint64_t cpp_riscv64_cpu_mask_to_hart_mask(uint32_t cmask);

void cpp_print_current_thread_backtrace();

}  // extern "C"

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_RISCV64_FFI_H_
