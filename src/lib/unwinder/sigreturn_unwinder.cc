// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/sigreturn_unwinder.h"

#include <array>
#include <cinttypes>
#include <cstdint>

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/loaded_elf_module.h"

namespace unwinder {

namespace {
// Returns the (possibly slightly adjusted) PC from the given |regs| and an SP-based offset that
// points directly to the start of the registers contained in a sigcontext_t struct for the
// appropriate architecture of |regs|.
fit::result<Error, std::pair<uint64_t, uint64_t>> GetPCAndOffsetFromRegs(const Registers& regs) {
  uint64_t pc = 0;
  if (auto err = regs.GetPC(pc); err.has_err()) {
    return fit::error(err);
  }

  switch (regs.arch()) {
    case Registers::Arch::kX64:
      return fit::error(Error("SigReturnUnwinder not implemented for X64"));
    case Registers::Arch::kArm32: {
      // The sp points to an rt_sigframe:
      //
      // 128 byte siginfo struct
      // ucontext struct:
      //     4 byte long: uc_flags
      //     4 byte pointer: uc_link
      //    12 byte stack_t
      //       sigcontext
      const uint64_t sigcontext_offset = 128 + 4 + 4 + 12;
      // Add another 12 to skip over the trap_no, error_code, and oldmask fields in the sigcontext
      // struct.
      const uint64_t regs_offset = sigcontext_offset + 12;

      uint64_t sp;
      if (Error error = regs.GetSP(sp); error.has_err()) {
        return fit::error(error);
      }

      return fit::ok(std::make_pair(pc, sp + regs_offset));
    }
    case Registers::Arch::kArm64: {
      // The sp points to an rt_sigframe:
      //
      // 128 byte siginfo struct
      // ucontext struct:
      //     8 byte long: uc_flags
      //     8 byte pointer: uc_link
      //    24 byte stack_t
      //   128 byte signal set
      //     8 byte padding to 16 byte align sigcontext
      //       sigcontext
      const uint64_t sigcontext_offset = 128 + 8 + 8 + 24 + 128 + 8;
      const uint64_t regs_offset = sigcontext_offset + 8;

      uint64_t sp;
      if (Error error = regs.GetSP(sp); error.has_err()) {
        return fit::error(error);
      }

      return fit::ok(std::make_pair(pc, sp + regs_offset));
    }
    case Registers::Arch::kRiscv64:
      return fit::error(Error("SigReturnUnwinder not implemented for riscv64"));
  }
}
}  // namespace

Error SigReturnUnwinder::Step(Memory* stack, const Frame& current, Frame& next) {
  uint64_t pc = 0;
  uint64_t sp_offset = 0;
  if (auto result = GetPCAndOffsetFromRegs(current.regs); result.is_ok()) {
    std::tie(pc, sp_offset) = result.value();
  } else {
    return result.error_value();
  }

  return Step(stack, pc, sp_offset, current.regs.arch(), next.regs);
}

Error SigReturnUnwinder::Step(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers::Arch arch,
                              Registers& next) {
  switch (arch) {
    case Registers::Arch::kX64:
      return StepX64(stack, pc, sp_offset, next);
    case Registers::Arch::kArm32:
      return StepArm32(stack, pc, sp_offset, next);
    case Registers::Arch::kArm64:
      return StepArm64(stack, pc, sp_offset, next);
    case Registers::Arch::kRiscv64:
      return StepRiscv64(stack, pc, sp_offset, next);
  }
}

void SigReturnUnwinder::AsyncStep(AsyncMemory* stack, const Frame& current,
                                  fit::callback<void(Error, Registers)> cb) {
  uint64_t pc = 0;
  uint64_t sp_offset = 0;
  if (auto result = GetPCAndOffsetFromRegs(current.regs); result.is_ok()) {
    std::tie(pc, sp_offset) = result.value();
  } else {
    cb(result.error_value(), Registers(current.regs.arch()));
    return;
  }

  AsyncStep(stack, pc, sp_offset, current.regs.arch(), std::move(cb));
}

void SigReturnUnwinder::AsyncStep(AsyncMemory* stack, uint64_t pc, uint64_t sp_offset,
                                  Registers::Arch arch, fit::callback<void(Error, Registers)> cb) {
  uint32_t read_size = 0;
  switch (arch) {
    case Registers::Arch::kArm32:
      // The sigcontext struct has 16, 4 byte registers.
      read_size = 4 * 16;
      break;
    case Registers::Arch::kArm64:
      // The sigcontext struct has 33 (31 x0-x30 GP + SP + PC), 8 byte registers.
      read_size = 8 * 33;
      break;
    case Registers::Arch::kX64:
      [[fallthrough]];
    case Registers::Arch::kRiscv64:
      cb(Error("Not implemented"), Registers(arch));
      return;
  }

  stack->FetchMemoryRanges({{sp_offset, read_size}}, [=, cb = std::move(cb)]() mutable {
    Registers next(arch);
    auto err = Step(stack, pc, sp_offset, arch, next);
    cb(err, std::move(next));
  });
}

// static.
Error SigReturnUnwinder::ProbePCForSigReturn(const ElfModuleCache& module_cache,
                                             const Registers& regs) {
  uint64_t pc = 0;
  if (auto err = regs.GetPC(pc); err.has_err()) {
    return err;
  }

  return ProbePCForSigReturn(module_cache, regs.arch(), pc);
}

// static.
Error SigReturnUnwinder::ProbePCForSigReturn(const ElfModuleCache& module_cache,
                                             Registers::Arch arch, uint64_t pc) {
  auto loaded_elf_module = module_cache.GetLoadedElfModuleForPc(pc);
  if (loaded_elf_module.is_error()) {
    return loaded_elf_module.error_value();
  }

  if (loaded_elf_module->get().binary_memory() == nullptr) {
    return Error("No binary memory found for address: %#" PRIx64, pc);
  }

  Memory* binary_memory = loaded_elf_module->get().binary_memory();
  switch (arch) {
    case Registers::Arch::kArm32:
      return ProbeArm32SigReturn(binary_memory, pc);
    case Registers::Arch::kArm64:
      return ProbeArm64SigReturn(binary_memory, pc);
    default:
      return Error("Not implemented.");
  }
}

Error SigReturnUnwinder::StepX64(Memory* stack, uint64_t pc, uint64_t sp_offset, Registers& next) {
  return Error("not implemented");
}

Error SigReturnUnwinder::StepArm32(Memory* stack, uint64_t pc, uint64_t sp_offset,
                                   Registers& next) {
  // The sigreturn function looks like:
  //
  // 00000114 <__kernel_rt_sigreturn>:
  //      114: e3a070ad      mov     r7, #173
  //      118: ef000000      svc     #0x0
  if (auto err = ProbePCForSigReturn(module_cache(), Registers::Arch::kArm32, pc); err.has_err()) {
    return err;
  }

  // The layout of the sigcontext struct looks like this:
  //
  //  struct sigcontext {
  //    unsigned long trap_no;
  //    unsigned long error_code;
  //    unsigned long oldmask;
  //    unsigned long arm_r0;
  //    unsigned long arm_r1;
  //    unsigned long arm_r2;
  //    unsigned long arm_r3;
  //    unsigned long arm_r4;
  //    unsigned long arm_r5;
  //    unsigned long arm_r6;
  //    unsigned long arm_r7;
  //    unsigned long arm_r8;
  //    unsigned long arm_r9;
  //    unsigned long arm_r10;
  //    unsigned long arm_fp;
  //    unsigned long arm_ip;
  //    unsigned long arm_sp;
  //    unsigned long arm_lr;
  //    unsigned long arm_pc;
  //    unsigned long arm_cpsr;
  //    unsigned long fault_address;
  //  };
  //
  //  We don't care about anything other than the register values.
  for (size_t i = 0; i < static_cast<size_t>(RegisterID::kArm32_last); i++) {
    uint32_t gpr;
    if (Error error = stack->Read(sp_offset + (i * sizeof(gpr)), gpr); error.has_err()) {
      return error;
    }
    next.Set(static_cast<RegisterID>(i), gpr);
  }

  return Success();
}

Error SigReturnUnwinder::StepArm64(Memory* stack, uint64_t pc, uint64_t sp_offset,
                                   Registers& next) {
  // The sigreturn function looks like:
  //
  // 00000000000001d0 <__kernel_rt_sigreturn>:
  //      1d0: d2801168      mov     x8, #0x8b
  //      1d4: d4000001      svc     #0
  if (auto err = ProbePCForSigReturn(module_cache(), Registers::Arch::kArm64, pc); err.has_err()) {
    return err;
  }

  // GPRs, sp, and pc are stored in sigcontext in the same order as aadwarf64
  // names registers.
  for (size_t i = 0; i < static_cast<size_t>(RegisterID::kArm64_last); i++) {
    uint64_t gpr;
    if (Error error = stack->Read(sp_offset + (i * sizeof(gpr)), gpr); error.has_err()) {
      return error;
    }
    next.Set(static_cast<RegisterID>(i), gpr);
  }

  return Success();
}

Error SigReturnUnwinder::StepRiscv64(Memory* stack, uint64_t pc, uint64_t sp_offset,
                                     Registers& next) {
  return Error("not implemented");
}

Error SigReturnUnwinder::ProbeArm32SigReturn(Memory* stack, uint64_t pc) {
  // Check for the sigreturn instruction sequence.
  uint64_t instructions;
  if (Error error = stack->Read(pc, instructions); error.has_err()) {
    return error;
  }
  if (instructions != 0xef000000e3a070adULL) {
    return Error("It doesn't look like a sigreturn function");
  }

  return Success();
}

Error SigReturnUnwinder::ProbeArm64SigReturn(Memory* stack, uint64_t pc) {
  // Check for the sigreturn instruction sequence.
  uint64_t instructions;
  if (Error error = stack->Read(pc, instructions); error.has_err()) {
    return error;
  }
  if (instructions != 0xd4000001d2801168ULL) {
    return Error("It doesn't look like a sigreturn function");
  }

  return Success();
}

}  // namespace unwinder
