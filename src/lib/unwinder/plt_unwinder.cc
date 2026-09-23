// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/plt_unwinder.h"

#include <cstdint>

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/loaded_elf_module.h"

namespace unwinder {

Error PltUnwinder::Step(Memory* stack, const Frame& current, Frame& next) {
  return Step(stack, current.regs, next.regs);
}

Error PltUnwinder::Step(Memory* stack, const Registers& current, Registers& next) {
  switch (current.arch()) {
    case Registers::Arch::kX64:
      return StepX64(stack, current, next);
    case Registers::Arch::kArm32:
      return StepArm32(stack, current, next);
    case Registers::Arch::kArm64:
      return StepArm64(stack, current, next);
    case Registers::Arch::kRiscv64:
      return StepRiscv64(stack, current, next);
  }
}

void PltUnwinder::AsyncStep(AsyncMemory* stack, const Frame& current,
                            fit::callback<void(Error, Registers)> cb) {
  AsyncStep(stack, current.regs, std::move(cb));
}

void PltUnwinder::AsyncStep(AsyncMemory* stack, const Registers& current,
                            fit::callback<void(Error, Registers)> cb) {
  Registers next(current.arch());

  switch (current.arch()) {
    case Registers::Arch::kX64: {
      // X64 PLT entries need to read from the stack.
      uint64_t sp = 0;
      if (auto err = current.GetSP(sp); err.has_err()) {
        cb(err, std::move(next));
        return;
      }

      // We always are going to read 2 machine sized integers from the stack when unwinding from an
      // x64 PLT call.
      constexpr uint32_t kStackFetchSize = 16;
      stack->FetchMemoryRanges({{sp, kStackFetchSize}},
                               [=, next = std::move(next), cb = std::move(cb)]() mutable {
                                 auto err = StepX64(stack, current, next);
                                 cb(err, std::move(next));
                               });
      break;
    }
    case Registers::Arch::kArm32: {
      // Arm32 only requires register and module memory access which can be provided synchronously.
      auto err = StepArm32(stack, current, next);
      cb(err, std::move(next));
      break;
    }
    case Registers::Arch::kArm64: {
      // Arm64 only requires register and module memory access which can be provided synchronously.
      auto err = StepArm64(stack, current, next);
      cb(err, std::move(next));
      break;
    }
    case Registers::Arch::kRiscv64: {
      auto err = StepRiscv64(stack, current, next);
      cb(err, std::move(next));
      break;
    }
  }
}

Error PltUnwinder::StepX64(Memory* stack, const Registers& current, Registers& next) {
  // The PLT stub looks like
  //
  // 0000000001477870 <printf@plt>:
  //  1477870: ff 25 42 26 1b 00             jmpq    *1779266(%rip)
  //  1477876: 68 06 00 00 00                pushq   $6
  //  147787b: e9 80 ff ff ff                jmp     0x1477800 <.plt>

  uint64_t sp;
  if (auto err = current.GetSP(sp); err.has_err()) {
    return err;
  }
  uint64_t sp_val[2];
  if (auto err = stack->Read(sp, sp_val); err.has_err()) {
    return err;
  }
  uint64_t ra;
  if (module_cache().IsValidPC(sp_val[0])) {
    ra = sp_val[0];
    sp += 8;
  } else if (module_cache().IsValidPC(sp_val[1])) {
    ra = sp_val[1];
    sp += 16;
  } else {
    return Error("It doesn't look like a PLT trampoline");
  }
  // Simulate a return.
  next = current;
  next.SetPC(ra);
  next.SetSP(sp);
  return Success();
}

Error PltUnwinder::StepArm32(Memory* stack, const Registers& current, Registers& next) {
  // The PLT stub looks like
  //
  // 000ce930 <printf@plt>:
  //   ce930: e28fc600      add     r12, pc, #0, #12
  //   ce934: e28cca24      add     r12, r12, #36, #20
  //   ce938: e5bcf44c      ldr     pc, [r12, #0x44c]!
  //   ce93c: d4 d4 d4 d4   .word   0xd4d4d4d4

  uint64_t lr;
  if (auto err = current.GetReturnAddress(lr); err.has_err()) {
    return err;
  }
  uint64_t pc;
  if (auto err = current.GetPC(pc); err.has_err()) {
    return err;
  }

  // Check whether the machine instruction is a PLT entry to avoid false positives. The compiler
  // inserts an invalid instruction encoded as d4d4d4d4 represented in little endian that we use as
  // a signature.
  auto loaded_elf_module = module_cache().GetLoadedElfModuleForPc(lr);
  if (loaded_elf_module.is_error()) {
    return loaded_elf_module.error_value();
  }
  uint32_t instruction;
  if (auto err = loaded_elf_module->get().binary_memory()->Read((pc & ~0xf) | 0xc, instruction);
      err.has_err()) {
    return err;
  }
  if (instruction != 0xd4d4d4d4) {
    return Error("It doesn't look like a PLT trampoline");
  }

  next = current;
  next.SetPC(lr);
  next.Unset(RegisterID::kArm32_lr);
  return Success();
}

Error PltUnwinder::StepArm64(Memory* stack, const Registers& current, Registers& next) {
  // The PLT stub looks like
  //
  // 00000000002d4580 <__stack_chk_fail@plt>:
  //   2d4580: 90000070      adrp    x16, 0x2e0000
  //   2d4584: f9456a11      ldr     x17, [x16, #2768]
  //   2d4588: 912b4210      add     x16, x16, #2768
  //   2d458c: d61f0220      br      x17

  uint64_t lr;
  if (auto err = current.GetReturnAddress(lr); err.has_err()) {
    return err;
  }
  uint64_t pc;
  if (auto err = current.GetPC(pc); err.has_err()) {
    return err;
  }

  // Check whether the machine instruction is a PLT entry to avoid false positives.
  // We use "br x17" as a signature, which is d61f0220 represented in little endian.
  auto loaded_elf_module = module_cache().GetLoadedElfModuleForPc(lr);
  if (loaded_elf_module.is_error()) {
    return loaded_elf_module.error_value();
  }

  uint32_t br_instruction;
  if (auto err = loaded_elf_module->get().binary_memory()->Read((pc & ~0xf) | 0xc, br_instruction);
      err.has_err()) {
    return err;
  }
  if (br_instruction != 0xd61f0220) {
    return Error("It doesn't look like a PLT trampoline");
  }

  next = current;
  next.SetPC(lr);
  next.Unset(RegisterID::kArm64_lr);
  next.Unset(RegisterID::kArm64_x16);
  next.Unset(RegisterID::kArm64_x17);
  return Success();
}

Error PltUnwinder::StepRiscv64(Memory* stack, const Registers& current, Registers& next) {
  return Error("not implemented");
}

}  // namespace unwinder
