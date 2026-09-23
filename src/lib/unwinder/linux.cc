// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/linux.h"

#include <errno.h>
#include <sys/uio.h>

namespace unwinder {

Error LinuxMemory::ReadBytes(uint64_t addr, uint64_t size, void* dst) {
  iovec local_iov;
  local_iov.iov_base = dst;
  local_iov.iov_len = size;

  iovec remote_iov;
  remote_iov.iov_base = reinterpret_cast<void*>(addr);
  remote_iov.iov_len = size;

  auto read_result = process_vm_readv(pid_, &local_iov, 1, &remote_iov, 1, 0);
  if (read_result < 0) {
    return Error("process_vm_readv: %d", errno);
  }
  if (static_cast<uint64_t>(read_result) != size) {
    return Error("process_vm_readv short read: expect %zu, got %zd", size, read_result);
  }
  return Success();
}

unwinder::Registers FromLinuxRegisters(const struct user_regs_struct& regs) {
#if defined(__x86_64__)
  unwinder::Registers res(unwinder::Registers::Arch::kX64);
  res.Set(unwinder::RegisterID::kX64_rax, regs.rax);
  res.Set(unwinder::RegisterID::kX64_rbx, regs.rbx);
  res.Set(unwinder::RegisterID::kX64_rcx, regs.rcx);
  res.Set(unwinder::RegisterID::kX64_rdx, regs.rdx);
  res.Set(unwinder::RegisterID::kX64_rsi, regs.rsi);
  res.Set(unwinder::RegisterID::kX64_rdi, regs.rdi);
  res.Set(unwinder::RegisterID::kX64_rbp, regs.rbp);
  res.Set(unwinder::RegisterID::kX64_rsp, regs.rsp);
  res.Set(unwinder::RegisterID::kX64_r8, regs.r8);
  res.Set(unwinder::RegisterID::kX64_r9, regs.r9);
  res.Set(unwinder::RegisterID::kX64_r10, regs.r10);
  res.Set(unwinder::RegisterID::kX64_r11, regs.r11);
  res.Set(unwinder::RegisterID::kX64_r12, regs.r12);
  res.Set(unwinder::RegisterID::kX64_r13, regs.r13);
  res.Set(unwinder::RegisterID::kX64_r14, regs.r14);
  res.Set(unwinder::RegisterID::kX64_r15, regs.r15);
  res.Set(unwinder::RegisterID::kX64_rip, regs.rip);
#elif defined(__aarch64__)
  unwinder::Registers res(unwinder::Registers::Arch::kArm64);
  for (int i = 0; i < static_cast<int>(unwinder::RegisterID::kArm64_last); i++) {
    res.Set(static_cast<unwinder::RegisterID>(i), regs.regs[i]);
  }
#elif defined(__riscv)
  unwinder::Registers res(unwinder::Registers::Arch::kX64);
  res.Set(unwinder::RegisterID::kRiscv64_pc, regs.pc);
  res.Set(unwinder::RegisterID::kRiscv64_ra, regs.ra);
  res.Set(unwinder::RegisterID::kRiscv64_sp, regs.sp);
  res.Set(unwinder::RegisterID::kRiscv64_gp, regs.gp);
  res.Set(unwinder::RegisterID::kRiscv64_tp, regs.tp);
  res.Set(unwinder::RegisterID::kRiscv64_t0, regs.t0);
  res.Set(unwinder::RegisterID::kRiscv64_t1, regs.t1);
  res.Set(unwinder::RegisterID::kRiscv64_t2, regs.t2);
  res.Set(unwinder::RegisterID::kRiscv64_s0, regs.s0);
  res.Set(unwinder::RegisterID::kRiscv64_s1, regs.s1);
  res.Set(unwinder::RegisterID::kRiscv64_a0, regs.a0);
  res.Set(unwinder::RegisterID::kRiscv64_a1, regs.a1);
  res.Set(unwinder::RegisterID::kRiscv64_a2, regs.a2);
  res.Set(unwinder::RegisterID::kRiscv64_a3, regs.a3);
  res.Set(unwinder::RegisterID::kRiscv64_a4, regs.a4);
  res.Set(unwinder::RegisterID::kRiscv64_a5, regs.a5);
  res.Set(unwinder::RegisterID::kRiscv64_a6, regs.a6);
  res.Set(unwinder::RegisterID::kRiscv64_a7, regs.a7);
  res.Set(unwinder::RegisterID::kRiscv64_s2, regs.s2);
  res.Set(unwinder::RegisterID::kRiscv64_s3, regs.s3);
  res.Set(unwinder::RegisterID::kRiscv64_s4, regs.s4);
  res.Set(unwinder::RegisterID::kRiscv64_s5, regs.s5);
  res.Set(unwinder::RegisterID::kRiscv64_s6, regs.s6);
  res.Set(unwinder::RegisterID::kRiscv64_s7, regs.s7);
  res.Set(unwinder::RegisterID::kRiscv64_s8, regs.s8);
  res.Set(unwinder::RegisterID::kRiscv64_s9, regs.s9);
  res.Set(unwinder::RegisterID::kRiscv64_s10, regs.s10);
  res.Set(unwinder::RegisterID::kRiscv64_s11, regs.s11);
  res.Set(unwinder::RegisterID::kRiscv64_t3, regs.t3);
  res.Set(unwinder::RegisterID::kRiscv64_t4, regs.t4);
  res.Set(unwinder::RegisterID::kRiscv64_t5, regs.t5);
  res.Set(unwinder::RegisterID::kRiscv64_t6, regs.t6);
#else
#error What platform?
#endif
  return res;
}

}  // namespace unwinder
