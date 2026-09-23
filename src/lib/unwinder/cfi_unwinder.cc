// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/cfi_unwinder.h"

#include <cinttypes>
#include <cstdint>
#include <limits>
#include <memory>
#include <utility>

#include "src/lib/unwinder/cfi_module.h"
#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/loaded_elf_module.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

namespace {
// Validates our assumptions about what registers we should have recovered in a "transition" 32 bit
// frame that was recovered from a 64 bit frame. For now, this should only ever happen via Starnix's
// custom CFI directives that get us the first restricted mode frame, which is what should be
// contained in |regs|. In this state, we should always successfully recover all of SP, LR, and PC,
// and they should all be pointing into the 32 bit restricted mode address space.
Error Validate32BitRegisters(const Registers& regs) {
  if (regs.arch() != Registers::Arch::kArm32) {
    return Error("New registers aren't kArm32?");
  }

  uint64_t val;
  if (auto err = regs.GetSP(val); err.has_err()) {
    return Error("32 bit registers don't have SP (r13) set: %s\n32 Bit Registers: %s",
                 err.msg().c_str(), regs.Describe().c_str());
  } else if (val > std::numeric_limits<uint32_t>::max()) {
    return Error("32 bit SP (r13) %" PRIx64 " > uint32::MAX", val);
  }

  if (auto err = regs.GetReturnAddress(val); err.has_err()) {
    return Error("32 bit registers don't have LR (r14) set: %s\n32 Bit Registers: %s",
                 err.msg().c_str(), regs.Describe().c_str());
  } else if (val > std::numeric_limits<uint32_t>::max()) {
    return Error("32 bit LR (r14) %" PRIx64 " > uint32::MAX", val);
  }

  if (auto err = regs.GetPC(val); err.has_err()) {
    return Error("32 bit registers don't have PC (r15) set: %s\n32 Bit Registers: %s",
                 err.msg().c_str(), regs.Describe().c_str());
  } else if (val > std::numeric_limits<uint32_t>::max()) {
    return Error("32 bit PC (r15) %" PRIx64 " > uint32::MAX", val);
  }

  return Success();
}

// Attempts to convert |current| to an equivalent Register object with the arm32 ABI and the given
// (possibly adjusted) |pc|. All other registers in the returned value will be converted from
// |current|.
//
// In particular, this means:
//   * x0-x12 are restored to r0-r12 in the returned object verbatim.
//   * x29 (aarch64 FP) is NOT restored.
//   * x30 (aarch64 LR) is restored to r14 (arm32 LR).
//   * x31 (aarch64 SP) is restored to r13 (arm32 SP).
//
// See Registers::To32Bit for more details.
// Returns an error if the |next| registers do not have PC and LR populated with 32 bit values.
fit::result<Error, Registers> TryConvertRegistersTo32Bit(uint64_t pc, const Registers& current) {
  if (current.arch() != Registers::Arch::kArm64) {
    return fit::error(Error("Current registers are not kArm64."));
  }

  uint64_t ra = 0;

  // As of today the only way we should ever successfully transition to a 32 bit frame is when we
  // have just recovered all of the registers specified by Starnix's CFI directives to reconstruct
  // the restricted mode stack. That means that we should _always_ have both PC and LR available
  // from the CFI (which should be finished processing by the time this is called). Therefore if we
  // cannot fetch either of them from |next| we bail out.
  if (current.GetReturnAddress(ra).has_err()) {
    return fit::error(Error("Next registers do not have LR set: %s", current.Describe().c_str()));
  }

  if (!(pc < std::numeric_limits<uint32_t>::max()) ||
      !(ra < std::numeric_limits<uint32_t>::max())) {
    return fit::error(Error("PC [%" PRIx64 "] or LR [%" PRIx64
                            "] contains address greater than 32 bit address space.",
                            pc, ra));
  }

  auto result = current.To32Bit();
  if (!result.is_ok()) {
    return result.take_error();
  }

  // Set the PC ourselves because it has already been adjusted by the time this function has been
  // called.
  result->SetPC(pc);
  return result.take_value();
}

}  // namespace

CfiUnwinder::CfiUnwinder(const ElfModuleCache& elf_module_cache) : UnwinderBase(elf_module_cache) {}

CfiUnwinder::~CfiUnwinder() = default;

Error CfiUnwinder::Step(Memory* stack, const Frame& current, Frame& next) {
  if (auto result = Step(stack, current.regs, next.regs, current.pc_is_return_address);
      result.is_error()) {
    return result.error_value();
  } else {
    next.is_signal_frame = result.value();
  }

  return Success();
}

fit::result<Error, bool> CfiUnwinder::Step(Memory* stack, const Registers& current, Registers& next,
                                           bool is_return_address) {
  uint64_t pc;
  if (auto err = current.GetPC(pc); err.has_err()) {
    return fit::error(err);
  }

  Registers regs = current;

  // is_return_address indicates whether pc in the current registers is a return address from a
  // previous "Step". If it is, we need to subtract 1 to find the call site because "call" could
  // be the last instruction of a nonreturn function and now the PC is pointing outside of the
  // valid code boundary.
  //
  // Subtracting 1 is sufficient here because in CfiParser::ParseInstructions, we scan CFI until
  // pc > pc_limit. So it's still correct even if pc_limit is not pointing to the beginning of an
  // instruction.
  if (is_return_address) {
    pc -= 1;
    regs.SetPC(pc);
  }

  if (auto result = ConvertTo32BitIfNeeded(pc, current); result.is_ok()) {
    // Validations succeeded, |next| is a 32 bit frame recovered from Starnix restricted mode.
    // It's entirely possible at this point for the 32 bit binary to have CFI instructions for
    // this 32 bit PC value, so we continue on.
    regs = result.value();
    next = Registers(regs.arch());
  }

  auto cfi_module = GetCfiModuleInfoForPc(pc);
  if (cfi_module.is_error()) {
    return cfi_module.take_error();
  }

  auto result = cfi_module->get().Step(stack, regs, next);
  if (result.is_error()) {
    return result;
  }

  return fit::ok(result.value());
}

void CfiUnwinder::AsyncStep(AsyncMemory* stack, const Frame& current,
                            fit::callback<void(Error, Registers)> cb) {
  AsyncStep(stack, current.regs, current.pc_is_return_address, std::move(cb));
}

void CfiUnwinder::AsyncStep(AsyncMemory* stack, const Registers& current, bool is_return_address,
                            fit::callback<void(Error, Registers)> cb) {
  uint64_t pc;
  if (auto err = current.GetPC(pc); err.has_err()) {
    return cb(err, Registers(current.arch()));
  }

  Registers regs = current;

  // is_return_address indicates whether pc in the current registers is a return address from a
  // previous "Step". If it is, we need to subtract 1 to find the call site because "call" could
  // be the last instruction of a nonreturn function and now the PC is pointing outside of the
  // valid code boundary.
  //
  // Subtracting 1 is sufficient here because in CfiParser::ParseInstructions, we scan CFI until
  // pc > pc_limit. So it's still correct even if pc_limit is not pointing to the beginning of an
  // instruction.
  if (is_return_address) {
    pc -= 1;
    regs.SetPC(pc);
  }

  if (auto result = ConvertTo32BitIfNeeded(pc, current); result.is_ok()) {
    // Validations succeeded, |next| is a 32 bit frame recovered from Starnix restricted mode.
    // It's entirely possible at this point for the 32 bit binary to have CFI instructions for
    // this 32 bit PC value, so we continue on.
    regs = result.value();
  }

  auto cfi_module = GetCfiModuleInfoForPc(pc);
  if (cfi_module.is_error()) {
    return cb(cfi_module.error_value(), Registers(current.arch()));
  }

  return cfi_module->get().AsyncStep(stack, regs,
                                     [cb = std::move(cb)](Error err, Registers next_regs) mutable {
                                       cb(err, std::move(next_regs));
                                     });
}

fit::result<Error, Registers> CfiUnwinder::ConvertTo32BitIfNeeded(uint64_t pc,
                                                                  const Registers& current) {
  // We might have a PC in a 32 bit module. If we do, we'll need to convert the registers to the 32
  // bit arch so all the named registers correspond to their 32 bit register numbers instead of 64
  // bit. Checking that PC < UINT32_MAX is just a heuristic and doesn't actually indicate
  // anything about address size for the module.
  if (current.arch() != Registers::Arch::kArm32 && pc < std::numeric_limits<uint32_t>::max()) {
    auto next_module_res = module_cache().GetLoadedElfModuleForPc(pc);
    if (next_module_res.is_error()) {
      return next_module_res.take_error();
    };

    const LoadedElfModule& next_module = next_module_res->get();
    if (next_module.size() == Module::AddressSize::k32Bit) {
      // In the error case, the message is probably only useful for developing and debugging the
      // unwinder itself and will happen frequently enough that we shouldn't log it, but for
      // debugging purposes can be displayed if needed. The validation step in the success case is a
      // fatal error since we have strict expectations of what registers are restored by Starnix,
      // which is currently the only way we should ever transition to code in a 32 bit address
      // space.
      if (auto maybe_32bit = TryConvertRegistersTo32Bit(pc, current); maybe_32bit.is_ok()) {
        // Both PC and LR in |current| appear to be 32 bit addresses, now validate that the
        // converted 32 bit registers actually have everything that we expect to get from Starnix's
        // CFI: PC, LR, and SP should all be populated and have 32 bit addresses, if this fails at
        // this point, it's an error.
        if (auto err = Validate32BitRegisters(*maybe_32bit); err.has_err()) {
          return fit::error(err);
        }

        return maybe_32bit;
      } else {
        return fit::error(Error("failed to convert registers to 32 bit: %s\n",
                                maybe_32bit.error_value().msg().c_str()));
      }
    } else {
      return fit::error(Error("next module at 0x%lx is not 32 bit!\n", next_module.load_address()));
    }
  }

  return fit::error(Error("Invalid input: current.arch = %d, pc = 0x%lx\n", current.arch(), pc));
}

fit::result<Error, CfiUnwinder::CfiModuleRef> CfiUnwinder::GetCfiModuleInfoForPc(uint64_t pc) {
  auto loaded_elf_module_res = module_cache().GetLoadedElfModuleForPc(pc);
  if (loaded_elf_module_res.is_error()) {
    return loaded_elf_module_res.take_error();
  }

  const LoadedElfModule& loaded_elf_module = loaded_elf_module_res->get();
  auto found = module_map_.find(loaded_elf_module.load_address());

  if (found == module_map_.end()) {
    // Not in our cache yet, create a new one from the ELF module.
    auto cfi_module = CfiModule::FromLoadedElfModule(loaded_elf_module);
    if (cfi_module.is_error()) {
      return cfi_module.take_error();
    }

    auto inserted = module_map_.emplace(loaded_elf_module.load_address(), std::move(*cfi_module));
    return fit::ok(CfiModuleRef(*inserted.first->second));
  }

  // Already loaded.
  return fit::ok(CfiUnwinder::CfiModuleRef(*found->second));
}

}  // namespace unwinder
