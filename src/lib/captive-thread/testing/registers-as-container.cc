// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/captive-thread/testing/matchers.h>

namespace captive_thread::testing {

void PrintTo(const RegistersContainer& regs, std::ostream* os) {
  std::string_view sep = "{";
  for (const auto& [name, value] : regs) {
    *os << sep << name << "=" << value;
    sep = ", ";
  }
  *os << "}";
}

RegistersContainer RegistersAsContainer(const zx_thread_state_general_regs_t& regs) {
#ifdef __aarch64__
  RegistersContainer pairs;
  pairs.reserve(sizeof(regs) / sizeof(uint64_t));
  for (size_t i = 0; i < 30; ++i) {
    pairs.emplace_back("x" + std::to_string(i), regs.r[i]);
  }
  pairs.append_range(std::initializer_list<RegistersContainer::value_type>{
      {"lr", {regs.lr}},
      {"sp", {regs.sp}},
      {"pc", {regs.pc}},
      {"cpsr", {regs.cpsr}},
      {"tpidr", {regs.tpidr}},
  });
  return pairs;
#elifdef __riscv
  return RegistersContainer({
      {"pc", {regs.pc}}, {"ra", {regs.ra}}, {"sp", {regs.sp}},   {"gp", {regs.gp}},
      {"tp", {regs.tp}}, {"t0", {regs.t0}}, {"t1", {regs.t1}},   {"t2", {regs.t2}},
      {"s0", {regs.s0}}, {"s1", {regs.s1}}, {"a0", {regs.a0}},   {"a1", {regs.a1}},
      {"a2", {regs.a2}}, {"a3", {regs.a3}}, {"a4", {regs.a4}},   {"a5", {regs.a5}},
      {"a6", {regs.a6}}, {"a7", {regs.a7}}, {"s2", {regs.s2}},   {"s3", {regs.s3}},
      {"s4", {regs.s4}}, {"s5", {regs.s5}}, {"s6", {regs.s6}},   {"s7", {regs.s7}},
      {"s8", {regs.s8}}, {"s9", {regs.s9}}, {"s10", {regs.s10}}, {"s11", {regs.s11}},
      {"t3", {regs.t3}}, {"t4", {regs.t4}}, {"t5", {regs.t5}},   {"t6", {regs.t6}},
  });
#elifdef __x86_64__
  return RegistersContainer({
      {"rax", {regs.rax}},         {"rbx", {regs.rbx}},         {"rcx", {regs.rcx}},
      {"rdx", {regs.rdx}},         {"rsi", {regs.rsi}},         {"rdi", {regs.rdi}},
      {"rbp", {regs.rbp}},         {"rsp", {regs.rsp}},         {"r8", {regs.r8}},
      {"r9", {regs.r9}},           {"r10", {regs.r10}},         {"r11", {regs.r11}},
      {"r12", {regs.r12}},         {"r13", {regs.r13}},         {"r14", {regs.r14}},
      {"r15", {regs.r15}},         {"rip", {regs.rip}},         {"rflags", {regs.rflags}},
      {"fs_base", {regs.fs_base}}, {"gs_base", {regs.gs_base}},
  });
#endif
}

}  // namespace captive_thread::testing
