// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/ktrace.h>

#include <hypervisor/ktrace.h>
#include <kernel/thread.h>

#if defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
#endif
static fxt::StringRef<fxt::RefType::kId> vcpu_meta[] = {
    [VCPU_INTERRUPT] = "wait:interrupt"_intern,
    [VCPU_PORT] = "wait:port"_intern,
};
static_assert((sizeof(vcpu_meta) / sizeof(vcpu_meta[0])) == VCPU_META_COUNT,
              "vcpu_meta array must match enum VcpuMeta");

static fxt::StringRef<fxt::RefType::kId> vcpu_exit[] = {
#if ARCH_ARM64
    [VCPU_UNDERFLOW_MAINTENANCE_INTERRUPT] = "exit:underflow_maintenance_interrupt"_intern,
    [VCPU_PHYSICAL_INTERRUPT] = "exit:physical_interrupt"_intern,
    [VCPU_WFI_INSTRUCTION] = "exit:wfi_instruction"_intern,
    [VCPU_WFE_INSTRUCTION] = "exit:wfe_instruction"_intern,
    [VCPU_SMC_INSTRUCTION] = "exit:smc_instruction"_intern,
    [VCPU_SYSTEM_INSTRUCTION] = "exit:system_instruction"_intern,
    [VCPU_INSTRUCTION_ABORT] = "exit:instruction_abort"_intern,
    [VCPU_DATA_ABORT] = "exit:data_abort"_intern,
    [VCPU_SERROR_INTERRUPT] = "exit:serror_interrupt"_intern,
#elif ARCH_X86
    [VCPU_EXCEPTION_OR_NMI] = "exit:exception_or_nmi"_intern,
    [VCPU_EXTERNAL_INTERRUPT] = "exit:external_interrupt"_intern,
    [VCPU_INTERRUPT_WINDOW] = "exit:interrupt_window"_intern,
    [VCPU_CPUID] = "exit:cpuid"_intern,
    [VCPU_HLT] = "exit:hlt"_intern,
    [VCPU_CONTROL_REGISTER_ACCESS] = "exit:control_register_access"_intern,
    [VCPU_IO_INSTRUCTION] = "exit:io_instruction"_intern,
    [VCPU_RDMSR] = "exit:rdmsr"_intern,
    [VCPU_WRMSR] = "exit:wrmsr"_intern,
    [VCPU_VM_ENTRY_FAILURE] = "exit:vm_entry_failure"_intern,
    [VCPU_EPT_VIOLATION] = "exit:ept_violation"_intern,
    [VCPU_XSETBV] = "exit:xsetbv"_intern,
    [VCPU_PAUSE] = "exit:pause"_intern,
    [VCPU_VMCALL] = "exit:vmcall"_intern,
#endif
    [VCPU_NOT_SUPPORTED] = "exit:not_supported"_intern,
    [VCPU_FAILURE] = "exit:failure"_intern,
};
#if defined(__clang__)
#pragma GCC diagnostic pop
#endif
static_assert((sizeof(vcpu_exit) / sizeof(vcpu_exit[0])) == VCPU_EXIT_COUNT,
              "vcpu_exit array must match enum VcpuExit");

void ktrace_vcpu(VcpuBlockOp block_op, VcpuMeta meta) {
  const fxt::StringRef name = meta < VCPU_META_COUNT ? vcpu_meta[meta] : "vcpu meta"_intern;
  if (block_op == VCPU_BLOCK) {
    KTRACE_DURATION_BEGIN_LABEL_REF("kernel:arch", name, ("meta #", meta));
  } else if (block_op == VCPU_UNBLOCK) {
    KTRACE_DURATION_END_LABEL_REF("kernel:arch", name);
  }
}

void ktrace_vcpu_exit(VcpuExit exit, uint64_t exit_address) {
  if (exit < VCPU_EXIT_COUNT) {
    KTRACE_DURATION_END("kernel:arch", "vcpu", ("exit_address", exit_address),
                        ("exit_type", vcpu_exit[exit]));
  } else {
    KTRACE_DURATION_END("kernel:arch", "vcpu", ("exit_address", exit_address), ("exit_type", exit));
  }
}
