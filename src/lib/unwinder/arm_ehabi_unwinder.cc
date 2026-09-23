// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/arm_ehabi_unwinder.h"

#include <safemath/safe_math.h>

#include "src/lib/unwinder/arm_ehabi_module.h"
#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/loaded_elf_module.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

Error ArmEhAbiUnwinder::Step(Memory* stack, const Frame& current, Frame& next) {
  if (current.regs.arch() != Registers::Arch::kArm64 &&
      current.regs.arch() != Registers::Arch::kArm32) {
    return Error("Not ARM architecture: %s", ArchToString(current.regs.arch()).c_str());
  }

  uint64_t pc = 0;
  if (auto err = current.regs.GetPC(pc); err.has_err()) {
    return err;
  }

  Registers regs = current.regs;

  // pc_is_return_address indicates whether pc in the current registers is a return address from a
  // previous "Step". If it is, we need to subtract 1 to find the call site because "call" could
  // be the last instruction of a nonreturn function and now the PC is pointing outside of the
  // valid code boundary.
  //
  // Subtracting 1 is sufficient here because in |Search| above, we binary search function start
  // addresses to find the unwinding instructions corresponding to this address. So it's still
  // correct even if pc is not pointing to the beginning of an instruction.
  if (current.pc_is_return_address) {
    pc -= 1;
    regs.SetPC(pc);
  }

  auto loaded_elf_module = module_cache().GetLoadedElfModuleForPc(pc);
  if (loaded_elf_module.is_error()) {
    return loaded_elf_module.error_value();
  }

  switch (loaded_elf_module->get().size()) {
    case Module::AddressSize::k32Bit:
      // Make sure we mark the next registers as 32 bit so we're setting the expected PC, LR, and SP
      // registers.
      next.regs = Registers(Registers::Arch::kArm32);
      return Step(stack, loaded_elf_module->get(), current.regs, next.regs);
    case Module::AddressSize::k64Bit:
      return Error("Module for PC is not 32 bit.");
    default:
      return Error("Unknown ELF Class");
  }
}

Error ArmEhAbiUnwinder::Step(Memory* stack, const LoadedElfModule& loaded_elf_module,
                             const Registers& current, Registers& next) {
  auto ehabi_module = GetEhAbiModuleFromModuleInfo(loaded_elf_module);
  if (ehabi_module.is_error()) {
    return ehabi_module.error_value();
  }

  return ehabi_module->get().Step(stack, current, next);
}

void ArmEhAbiUnwinder::AsyncStep(AsyncMemory* stack, const Frame& current,
                                 fit::callback<void(Error, Registers)> cb) {
  if (current.regs.arch() != Registers::Arch::kArm64 &&
      current.regs.arch() != Registers::Arch::kArm32) {
    cb(Error("Not ARM architecture: %s", ArchToString(current.regs.arch()).c_str()),
       Registers(current.regs.arch()));
    return;
  }
  uint64_t pc;
  if (auto err = current.regs.GetPC(pc); err.has_err()) {
    return cb(err, Registers(current.regs.arch()));
  }

  Registers regs = current.regs;

  if (current.pc_is_return_address) {
    pc -= 1;
    regs.SetPC(pc);
  }

  auto loaded_elf_module = module_cache().GetLoadedElfModuleForPc(pc);
  if (loaded_elf_module.is_error()) {
    return cb(loaded_elf_module.error_value(), Registers(current.regs.arch()));
  }

  if (loaded_elf_module->get().size() != Module::AddressSize::k32Bit) {
    cb(Error("Module for PC is not 32 bit."), Registers(current.regs.arch()));
    return;
  }

  AsyncStep(stack, loaded_elf_module->get(), regs, std::move(cb));
}

void ArmEhAbiUnwinder::AsyncStep(AsyncMemory* stack, const LoadedElfModule& elf_module,
                                 const Registers& current,
                                 fit::callback<void(Error, Registers)> cb) {
  auto result = GetEhAbiModuleFromModuleInfo(elf_module);
  if (result.is_error()) {
    return cb(result.error_value(), Registers(current.arch()));
  }

  const ArmEhAbiModule& ehabi_module = result.value();

  uint64_t sp;
  if (auto err = current.GetSP(sp); err.has_err()) {
    cb(err, Registers(current.arch()));
    return;
  }

  constexpr uint32_t kDefaultStackSize = 4096;
  stack->FetchMemoryRanges({{sp, kDefaultStackSize}}, [=, cb = std::move(cb)]() mutable {
    ehabi_module.AsyncStep(stack, current, std::move(cb));
  });
}

fit::result<Error, ArmEhAbiUnwinder::ArmEhAbiModuleRef>
ArmEhAbiUnwinder::GetEhAbiModuleFromModuleInfo(const LoadedElfModule& loaded_elf_module) {
  // The ModuleCache keeps a record of all the modules, so it can properly find the right module
  // for this PC. Since we don't have to keep track of anything other than the 32 bit modules
  // here we can just index on the load address of the already found module. Use checked_cast here
  // since we should never get this far unless we think we have a 32 bit module, which implies that
  // there should also be a 32 bit PC.
  auto found = module_map_.find(safemath::checked_cast<uint32_t>(loaded_elf_module.load_address()));

  if (found == module_map_.end()) {
    auto res = ArmEhAbiModule::FromLoadedElfModule(loaded_elf_module);
    if (res.is_error()) {
      return res.take_error();
    }

    auto inserted = module_map_.emplace(static_cast<uint32_t>(loaded_elf_module.load_address()),
                                        std::move(*res));

    // We have a new, valid ArmEhAbiModule which has been successfully loaded, insert it into the
    // map.
    return fit::ok(ArmEhAbiModuleRef(*inserted.first->second));
  }

  // Already have inserted this one into our cache, return that.
  return fit::ok(ArmEhAbiModuleRef(*found->second));
}

}  // namespace unwinder
