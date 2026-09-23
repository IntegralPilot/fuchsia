// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/fp_unwinder.h"

#include <cinttypes>
#include <cstdint>

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/loaded_elf_module.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

namespace {

// The maximum frame size we use when checking whether a frame pointer points to the stack.
// This could be further improved to ask users to provide the stack size.
uint64_t kMaxFrameSize = 8ull * 1024 * 1024;  // 8 MB

fit::result<Error, uint64_t> GetValidatedFP(const Registers& current) {
  uint64_t fp;
  if (auto err = current.GetFP(fp); err.has_err()) {
    return fit::error(err);
  }

  if (current.arch() == Registers::Arch::kRiscv64 && fp >= 16) {
    fp -= 16;
  }

  uint64_t sp;
  if (auto err = current.GetSP(sp); err.has_err()) {
    return fit::error(err);
  }

  if (fp < sp || fp > sp + kMaxFrameSize) {
    return fit::error(Error("current FP %#" PRIx64 " doesn't seem to be on the stack", fp));
  }

  return fit::ok(fp);
}

}  // namespace

Error FramePointerUnwinder::Step(Memory* stack, const Frame& current, Frame& next) {
  uint64_t pc = 0;
  if (auto e = current.regs.GetPC(pc); e.has_err()) {
    return e;
  }

  auto loaded_elf_module = module_cache().GetLoadedElfModuleForPc(pc);
  if (loaded_elf_module.is_error()) {
    return loaded_elf_module.error_value();
  }

  return Step(stack, current.regs, next.regs, loaded_elf_module->get());
}

Error FramePointerUnwinder::Step(Memory* stack, const Registers& current, Registers& next,
                                 const LoadedElfModule& loaded_elf_module) {
  auto fp = GetValidatedFP(current);
  if (fp.is_error()) {
    return fp.error_value();
  }

  uint64_t next_fp;
  uint64_t next_pc;
  if (auto err = ReadNextFpAndSp(stack, *fp, next_fp, next_pc, loaded_elf_module); err.has_err()) {
    return err;
  }

  next.SetSP(*fp);
  next.SetPC(next_pc);
  next.SetFP(next_fp);
  return Success();
}

void FramePointerUnwinder::AsyncStep(AsyncMemory* stack, const Frame& current,
                                     fit::callback<void(Error, Registers)> cb) {
  uint64_t pc = 0;
  if (auto e = current.regs.GetPC(pc); e.has_err()) {
    cb(e, Registers(current.regs.arch()));
    return;
  }

  auto loaded_elf_module = module_cache().GetLoadedElfModuleForPc(pc);
  if (loaded_elf_module.is_error()) {
    cb(loaded_elf_module.error_value(), Registers(current.regs.arch()));
    return;
  }

  AsyncStep(stack, current.regs, loaded_elf_module->get(), std::move(cb));
}

void FramePointerUnwinder::AsyncStep(AsyncMemory* stack, const Registers& current,
                                     const LoadedElfModule& loaded_elf_module,
                                     fit::callback<void(Error, Registers)> cb) {
  auto fp = GetValidatedFP(current);
  if (fp.is_error()) {
    cb(fp.error_value(), Registers(current.arch()));
    return;
  }

  // There's no harm in potentially reading more than we need, since |ReadNextFpAndSp| will account
  // for expected register sizes for this module.
  constexpr uint32_t kDefaultReadSize = 16;
  stack->FetchMemoryRanges({{*fp, kDefaultReadSize}}, [=, cb = std::move(cb)]() mutable {
    uint64_t next_fp;
    uint64_t next_pc;
    if (auto err = ReadNextFpAndSp(stack, *fp, next_fp, next_pc, loaded_elf_module);
        err.has_err()) {
      cb(err, Registers(current.arch()));
      return;
    }

    Registers next(current.arch());
    next.SetSP(*fp);
    next.SetPC(next_pc);
    next.SetFP(next_fp);
    cb(Success(), std::move(next));
  });
}

Error FramePointerUnwinder::ReadNextFpAndSp(Memory* stack, uint64_t& fp, uint64_t& next_fp,
                                            uint64_t& next_pc,
                                            const LoadedElfModule& loaded_elf_module) {
  switch (loaded_elf_module.size()) {
    case Module::AddressSize::k32Bit: {
      // Read 32 bit integers from the stack and upcast them to 64 bit integers for the caller.
      uint32_t next_fp32;
      uint32_t next_pc32;
      if (auto err = stack->ReadAndAdvance(fp, next_fp32); err.has_err()) {
        return err;
      }
      // Don't check the range of next_fp, because it may not be used as the frame pointer.

      if (auto err = stack->ReadAndAdvance(fp, next_pc32); err.has_err()) {
        return err;
      }

      next_fp = next_fp32;
      next_pc = next_pc32;
      break;
    }
    case Module::AddressSize::k64Bit: {
      // Can just read directly into the caller-provided integers.
      if (auto err = stack->ReadAndAdvance(fp, next_fp); err.has_err()) {
        return err;
      }
      // Don't check the range of next_fp, because it may not be used as the frame pointer.

      if (auto err = stack->ReadAndAdvance(fp, next_pc); err.has_err()) {
        return err;
      }

      break;
    }
    default:
      return Error("Unknown pointer size!");
  }

  if (!module_cache().IsValidPC(next_pc)) {
    return Error("next PC %#" PRIx64 " is not pointing to any code", next_pc);
  }

  return Success();
}

}  // namespace unwinder
