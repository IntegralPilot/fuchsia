// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_SIGRETURN_UNWINDER_H_
#define SRC_LIB_UNWINDER_SIGRETURN_UNWINDER_H_

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

// Unwind when pc is in a Linux sigreturn function.
class SigReturnUnwinder : public UnwinderBase {
 public:
  // We need |ElfModuleCache::GetModuleForPc|.
  explicit SigReturnUnwinder(const ElfModuleCache& module_cache) : UnwinderBase(module_cache) {}

  Error Step(Memory* stack, const Frame& current, Frame& next) override;

  void AsyncStep(AsyncMemory* stack, const Frame& current,
                 fit::callback<void(Error, Registers)> cb) override;

  Frame::Trust trust() const override { return Frame::Trust::kSigReturn; }

  static Error ProbePCForSigReturn(const ElfModuleCache& module_cache, Registers::Arch arch,
                                   uint64_t pc);

  static Error ProbePCForSigReturn(const ElfModuleCache& module_cache, const Registers& regs);

 private:
  Error Step(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers::Arch arch, Registers& next);
  void AsyncStep(AsyncMemory* stack, uint64_t pc, uint64_t sp_offset, Registers::Arch arch,
                 fit::callback<void(Error, Registers)> cb);

  Error StepX64(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers& next);
  Error StepArm32(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers& next);
  Error StepArm64(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers& next);
  Error StepRiscv64(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers& next);

  static Error ProbeArm32SigReturn(Memory* stack, uint64_t pc);
  static Error ProbeArm64SigReturn(Memory* stack, uint64_t pc);
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_SIGRETURN_UNWINDER_H_
