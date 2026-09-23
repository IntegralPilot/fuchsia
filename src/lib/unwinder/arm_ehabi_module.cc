// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/arm_ehabi_module.h"

#include <elf.h>

#include <limits>
#include <memory>

#include <safemath/checked_math.h>

#include "src/lib/unwinder/arm_ehabi_parser.h"
#include "src/lib/unwinder/elf_utils.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/loaded_elf_module.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

// static.
fit::result<Error, std::unique_ptr<ArmEhAbiModule>> ArmEhAbiModule::FromLoadedElfModule(
    const LoadedElfModule& loaded_elf_module) {
  if (loaded_elf_module.load_address() > std::numeric_limits<uint32_t>::max()) {
    return fit::error(Error("Load address too big to be 32 bit module!"));
  }

  if (!loaded_elf_module.binary_memory() && !loaded_elf_module.debug_info_memory()) {
    return fit::error(Error("No valid memory to use!"));
  }

  auto try_load_from_memory =
      [loaded_elf_module](Memory* elf) -> fit::result<Error, std::unique_ptr<ArmEhAbiModule>> {
    auto ehabi_module = std::unique_ptr<ArmEhAbiModule>(new ArmEhAbiModule(
        loaded_elf_module, elf, static_cast<uint32_t>(loaded_elf_module.load_address())));

    if (auto err = ehabi_module->Load(); err.is_error()) {
      return err.take_error();
    }

    return fit::ok(std::move(ehabi_module));
  };

  // Unlike CFI, where .debug_frame will contain higher quality unwind tables in the face of
  // limited/no size restrictions when compared to .eh_frame, we have no preference for either
  // binary or debug_info memory objects, which will contain the same unwinding instructions. So it
  // is just a matter of where it is present.
  fit::result<Error, std::unique_ptr<ArmEhAbiModule>> result =
      fit::error(Error("Failed to load from both binary and debug-info memory!"));
  if (loaded_elf_module.binary_memory()) {
    result = try_load_from_memory(loaded_elf_module.binary_memory());
  }

  // Loading from the live binary memory didn't work or was not available. Try debug_info memory.
  if (result.is_error() && loaded_elf_module.debug_info_memory()) {
    result = try_load_from_memory(loaded_elf_module.binary_memory());
  }

  return result;
}

fit::result<Error> ArmEhAbiModule::Load() {
  Elf32_Ehdr ehdr;
  if (auto err = elf_->Read(elf_ptr_, ehdr); err.has_err()) {
    return fit::error(err);
  }

  if (ehdr.e_ident[EI_MAG0] != ELFMAG0 || ehdr.e_ident[EI_MAG1] != ELFMAG1 ||
      ehdr.e_ident[EI_MAG2] != ELFMAG2 || ehdr.e_ident[EI_MAG3] != ELFMAG3) {
    return fit::error(Error("Invalid ELF header!"));
  }

  if (!elf_utils::VerifyElfIdentification<Elf32_Ehdr>(ehdr, elf_utils::ElfClass::k32Bit)) {
    return fit::error(Error("This doesn't look like an ELF module."));
  }

  auto phdr = loaded_elf_module_.GetSegmentByType(PT_ARM_EXIDX);
  if (phdr.is_error()) {
    return phdr.take_error();
  }

  if (!safemath::CheckAdd(elf_ptr_, phdr->p_vaddr).AssignIfValid(&arm_exidx_start_)) {
    return fit::error(Error("Overflowed while finding .ARM.exidx start."));
  }

  if (!safemath::CheckAdd(arm_exidx_start_, phdr->p_memsz).AssignIfValid(&arm_exidx_end_)) {
    return fit::error(Error("Overflowed while finding .ARM.exidx end."));
  }

  return fit::ok();
}

Error ArmEhAbiModule::Search(uint32_t pc, IdxHeader& entry) const {
  uint32_t low = 0;
  uint32_t high = (arm_exidx_end_ - arm_exidx_start_) / sizeof(IdxHeaderData);

  IdxHeaderData hdr;

  // When set, this will be the address of the most suitable entry we find in the table. If this is
  // std::nullopt by the end of the loop below, there were no suitable matches in this module.
  std::optional<uint32_t> best_entry_addr = std::nullopt;

  // Perform an Upper Bound search to find the largest function address not greater than |pc|. At
  // the end of this loop |addr| will point to the first entry of the index whose function pointer
  // is greater than |pc|. The best match is kept separately so we can better distinguish "not
  // found" errors. Keep in mind the function addresses (the first word of the index entry) must be
  // decoded before we can use them for comparison.
  while (low + 1 < high) {
    uint32_t mid = (low + high) / 2;
    uint32_t addr = arm_exidx_start_ + mid * sizeof(IdxHeaderData);
    uint32_t prel31_encoded_offset;
    if (auto err = elf_->Read(addr, prel31_encoded_offset); err.has_err()) {
      return err;
    }

    int32_t fn_offset = DecodePrel31(prel31_encoded_offset);
    // The function offset described in the Prel31 encoding is relative to the .ARM.exidx section,
    // we have to account for the current offset into the table as well.
    uint32_t decoded_fn_addr = addr + fn_offset;

    if (pc < decoded_fn_addr) {
      high = mid;
    } else {
      low = mid;
      // This is the new best entry for this PC value. Stash the decoded function address since
      // we've already decoded it, and stash away the address of this entry so we can get the next
      // word from the header at the end.
      hdr.fn_addr = decoded_fn_addr;
      best_entry_addr = addr;
    }
  }

  if (!best_entry_addr) {
    return Error("PC not found in this module.");
  }

  uint32_t data_addr = *best_entry_addr + sizeof(hdr.fn_addr);

  // Now we can get the associated unwinding data.
  if (auto err = elf_->Read(data_addr, hdr.data); err.has_err()) {
    return err;
  }

  // The high bit of the data field indicates whether bits 0-30 are an offset to the ARM.extab
  // section (which could either be the "generic model", or the "compact model" with too many
  // entries to inline into the index table) or if they're inlined opcodes (the "compact [inline]
  // model").
  if (hdr.data & 0x80000000) {
    entry.type = IdxHeader::Type::kCompactInline;
  } else {
    entry.type = IdxHeader::Type::kCompact;
    // The decoded relative address is an offset from the current position in the index, which
    // happens to always be in the middle of an index entry since the relative address will always
    // be the second entry.
    //
    // Note that we never actually need to do a section lookup on the .ARM.extab section because
    // this address will be pointing directly to the unwind table that we need for this function.
    // Since we don't know the precise starting address of the section, we cannot find the start of
    // the table based on this offset without consulting the string table or section header string
    // table which are both typically not mapped into a live process.
    hdr.data = DecodePrel31(hdr.data) + data_addr;
  }

  entry.header = hdr;

  return Success();
}

fit::result<Error, ArmEhAbiModule::IdxHeader> ArmEhAbiModule::PrepareToStep(
    const Registers& current) const {
  uint64_t pc;
  if (auto err = current.GetPC(pc); err.has_err()) {
    return fit::error(err);
  }

  IdxHeader entry;
  if (auto err = Search(static_cast<uint32_t>(pc), entry); err.has_err()) {
    return fit::error(err);
  }

  return fit::ok(entry);
}

Error ArmEhAbiModule::Step(Memory* stack, const Registers& current, Registers& next) const {
  IdxHeader entry;
  if (auto result = PrepareToStep(current); result.is_ok()) {
    entry = result.value();
  } else {
    return result.error_value();
  }

  ArmEhAbiParser parser(elf_, entry);

  return parser.Step(stack, current, next);
}

void ArmEhAbiModule::AsyncStep(AsyncMemory* stack, const Registers& current,
                               fit::callback<void(Error, Registers)> cb) const {
  IdxHeader entry;
  if (auto result = PrepareToStep(current); result.is_ok()) {
    entry = result.value();
  } else {
    return cb(result.error_value(), Registers(current.arch()));
  }

  ArmEhAbiParser parser(elf_, entry);

  Registers next(Registers::Arch::kArm32);
  auto err = parser.Step(stack, current, next);

  cb(err, next);
}

}  // namespace unwinder
