// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-data-holder.h"

#include <gtest/gtest.h>

namespace zxdump::testing {

void TestDataValueType<zx_info_cpu_stats_t>::Check(std::string_view name,
                                                   const zx_info_cpu_stats_t& old_value,
                                                   const zx_info_cpu_stats_t& new_value) {
  EXPECT_EQ(old_value.cpu_number, new_value.cpu_number) << name << " cpu_number";
  EXPECT_EQ(old_value.flags, new_value.flags) << name << " flags";
  EXPECT_LE(old_value.idle_time, new_value.idle_time) << name << " idle_time";
  EXPECT_LE(old_value.reschedules, new_value.reschedules) << name << " reschedules";
  EXPECT_LE(old_value.context_switches, new_value.context_switches) << name << " context_switches";
  EXPECT_LE(old_value.irq_preempts, new_value.irq_preempts) << name << " irq_preempts";
  EXPECT_LE(old_value.preempts, new_value.preempts) << name << " preempts";
  EXPECT_LE(old_value.yields, new_value.yields) << name << " yields";
  EXPECT_LE(old_value.ints, new_value.ints) << name << " ints";
  EXPECT_LE(old_value.timer_ints, new_value.timer_ints) << name << " timer_ints";
  EXPECT_LE(old_value.timers, new_value.timers) << name << " timers";
  EXPECT_LE(old_value.page_faults, new_value.page_faults) << name << " page_faults";
  EXPECT_LE(old_value.exceptions, new_value.exceptions) << name << " exceptions";
  EXPECT_LE(old_value.syscalls, new_value.syscalls) << name << " syscalls";
  EXPECT_LE(old_value.reschedule_ipis, new_value.reschedule_ipis) << name << " reschedule_ipis";
  EXPECT_LE(old_value.generic_ipis, new_value.generic_ipis) << name << " generic_ipis";
}

void TestDataValueType<zx_info_kmem_stats_t>::Check(std::string_view name,
                                                    const zx_info_kmem_stats_t& old_value,
                                                    const zx_info_kmem_stats_t& new_value) {
  // All other fields and go either up or down.
  EXPECT_GT(old_value.total_bytes, 0u);
  EXPECT_EQ(old_value.total_bytes, new_value.total_bytes) << name << " total_bytes";
}

void TestDataValueType<zx_x86_64_info_guest_stats_t>::Check(
    std::string_view name, const zx_x86_64_info_guest_stats_t& old_value,
    const zx_x86_64_info_guest_stats_t& new_value) {
  EXPECT_EQ(old_value.cpu_number, new_value.cpu_number) << name << " cpu_number";
  EXPECT_EQ(old_value.flags, new_value.flags) << name << " flags";
  EXPECT_LE(old_value.vm_entries, new_value.vm_entries) << name << " vm_entries";
  EXPECT_LE(old_value.vm_exits, new_value.vm_exits) << name << " vm_exits";
  EXPECT_LE(old_value.interrupts, new_value.interrupts) << name << " interrupts";
  EXPECT_LE(old_value.interrupt_windows, new_value.interrupt_windows)
      << name << " interrupt_windows";
  EXPECT_LE(old_value.cpuid_instructions, new_value.cpuid_instructions)
      << name << " cpuid_instructions";
  EXPECT_LE(old_value.hlt_instructions, new_value.hlt_instructions) << name << " hlt_instructions";
  EXPECT_LE(old_value.control_register_accesses, new_value.control_register_accesses)
      << name << " control_register_accesses";
  EXPECT_LE(old_value.io_instructions, new_value.io_instructions) << name << " io_instructions";
  EXPECT_LE(old_value.rdmsr_instructions, new_value.rdmsr_instructions)
      << name << " rdmsr_instructions";
  EXPECT_LE(old_value.wrmsr_instructions, new_value.wrmsr_instructions)
      << name << " wrmsr_instructions";
  EXPECT_LE(old_value.ept_violations, new_value.ept_violations) << name << " ept_violations";
  EXPECT_LE(old_value.xsetbv_instructions, new_value.xsetbv_instructions)
      << name << " xsetbv_instructions";
  EXPECT_LE(old_value.pause_instructions, new_value.pause_instructions)
      << name << " pause_instructions";
  EXPECT_LE(old_value.vmcall_instructions, new_value.vmcall_instructions)
      << name << " vmcall_instructions";
}

void TestDataValueType<zx_arm64_info_guest_stats_t>::Check(
    std::string_view name, const zx_arm64_info_guest_stats_t& old_value,
    const zx_arm64_info_guest_stats_t& new_value) {
  EXPECT_EQ(old_value.cpu_number, new_value.cpu_number) << name << " cpu_number";
  EXPECT_EQ(old_value.flags, new_value.flags) << name << " flags";
  EXPECT_LE(old_value.vm_entries, new_value.vm_entries) << name << " vm_entries";
  EXPECT_LE(old_value.vm_exits, new_value.vm_exits) << name << " vm_exits";
  EXPECT_LE(old_value.wfi_wfe_instructions, new_value.wfi_wfe_instructions)
      << name << " wfi_wfe_instructions";
  EXPECT_LE(old_value.instruction_aborts, new_value.instruction_aborts)
      << name << " instruction_aborts";
  EXPECT_LE(old_value.data_aborts, new_value.data_aborts) << name << " data_aborts";
  EXPECT_LE(old_value.system_instructions, new_value.system_instructions)
      << name << " system_instructions";
  EXPECT_LE(old_value.smc_instructions, new_value.smc_instructions) << name << " smc_instructions";
  EXPECT_LE(old_value.interrupts, new_value.interrupts) << name << " interrupts";
}

void TestDataValueType<zx_riscv64_info_guest_stats_t>::Check(
    std::string_view name, const zx_riscv64_info_guest_stats_t& old_value,
    const zx_riscv64_info_guest_stats_t& new_value) {
  EXPECT_EQ(old_value.cpu_number, new_value.cpu_number) << name << " cpu_number";
  EXPECT_EQ(old_value.flags, new_value.flags) << name << " flags";
  EXPECT_LE(old_value.vm_entries, new_value.vm_entries) << name << " vm_entries";
  EXPECT_LE(old_value.vm_exits, new_value.vm_exits) << name << " vm_exits";
  EXPECT_LE(old_value.interrupts, new_value.interrupts) << name << " interrupts";
}

}  // namespace zxdump::testing
