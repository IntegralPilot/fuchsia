// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/loaded_elf_module.h"

#include <algorithm>
#include <cstring>
#include <iterator>
#include <type_traits>

#include <safemath/safe_math.h>

#include "src/lib/unwinder/elf_utils.h"
#include "src/lib/unwinder/module.h"

namespace unwinder {

namespace {

template <typename Ehdr>
fit::result<Error, Elf64_Ehdr> ReadEhdr(Memory* memory, uint64_t load_address) {
  Ehdr ehdr;
  if (auto err = memory->Read(load_address, ehdr); err.has_err()) {
    return fit::error(err);
  }

  Elf64_Ehdr ehdr64;
  if constexpr (std::is_same_v<Ehdr, Elf32_Ehdr>) {
    ehdr64 = elf_utils::UpcastEhdr(ehdr);
  } else {
    ehdr64 = ehdr;
  }

  return fit::ok(ehdr64);
}

fit::result<Error, Elf64_Shdr> TryFindSectionByName(Memory* elf_memory, const Module* elf_module,
                                                    const Elf64_Ehdr& ehdr,
                                                    std::string_view target_section) {
  if (!elf_memory) {
    return fit::error(Error("No memory."));
  }

  switch (elf_module->size) {
    case Module::AddressSize::k32Bit: {
      auto res = elf_utils::GetSectionByName<Elf64_Ehdr, Elf32_Shdr>(
          elf_memory, elf_module->load_address, target_section, ehdr);
      if (res.is_error()) {
        return res.take_error();
      }

      return fit::ok(elf_utils::UpcastShdr(*res));
    }
    case Module::AddressSize::k64Bit: {
      auto res = elf_utils::GetSectionByName<Elf64_Ehdr, Elf64_Shdr>(
          elf_memory, elf_module->load_address, target_section, ehdr);
      if (res.is_error()) {
        return res;
      }

      return fit::ok(*res);
    }
    case Module::AddressSize::kUnknown: {
      return fit::error(Error("Unknown ELF class."));
    }
  }
}

}  // namespace

fit::result<Error> LoadedElfModule::Load() {
  if (!module_.binary_memory) {
    return fit::error(Error("no binary memory"));
  }

  if (auto err = LoadElfHeader(); err.is_error()) {
    return err;
  }

  if (auto err = LoadPhdrs(*ehdr_); err.is_error()) {
    return err;
  }

  return fit::ok();
}

fit::result<Error> LoadedElfModule::LoadElfHeader() {
  if (ehdr_.has_value()) {
    return fit::ok();
  }

  // Callers are responsible for ensuring that |module_.binary_memory| is valid before calling this
  // method.
  switch (module_.size) {
    case Module::AddressSize::k32Bit: {
      auto ehdr = ReadEhdr<Elf32_Ehdr>(module_.binary_memory, module_.load_address);
      if (ehdr.is_error()) {
        return ehdr.take_error();
      }

      ehdr_ = *ehdr;
      break;
    }
    case Module::AddressSize::k64Bit: {
      auto ehdr = ReadEhdr<Elf64_Ehdr>(module_.binary_memory, module_.load_address);
      if (ehdr.is_error()) {
        return ehdr.take_error();
      }

      ehdr_ = *ehdr;
      break;
    }
    case Module::AddressSize::kUnknown: {
      return fit::error(Error("Unknown ELF class."));
    }
  }

  if (!elf_utils::VerifyElfIdentification(*ehdr_, module_.size == Module::AddressSize::k32Bit
                                                      ? elf_utils::ElfClass::k32Bit
                                                      : elf_utils::ElfClass::k64Bit)) {
    return fit::error(Error("Invalid ELF header"));
  }

  return fit::ok();
}

fit::result<Error, Elf64_Shdr> LoadedElfModule::GetSectionByName(
    std::string_view target_section) const {
  if (!ehdr_) {
    return fit::error(Error("ELF Header not loaded!"));
  }

  if (auto res = TryFindSectionByName(module_.binary_memory, &module_, *ehdr_, target_section);
      res.is_ok()) {
    return res.take_value();
  }

  return TryFindSectionByName(module_.debug_info_memory, &module_, *ehdr_, target_section);
}

fit::result<Error, Elf64_Phdr> LoadedElfModule::GetSegmentByType(uint32_t p_type) const {
  if (phdrs_.empty()) {
    return fit::error(Error("No phdrs loaded yet."));
  }

  const auto& found =
      std::ranges::find_if(phdrs_, [=](const Elf64_Phdr& phdr) { return p_type == phdr.p_type; });

  if (found != phdrs_.end()) {
    return fit::ok(*found);
  }

  return fit::error(Error("Segment with type %d not found", p_type));
}

fit::result<Error> LoadedElfModule::LoadPhdrs(const Elf64_Ehdr& ehdr) {
  // Already loaded.
  if (!phdrs_.empty()) {
    return fit::ok();
  }

  if (ehdr.e_phnum == 0) {
    // No program headers to load.
    return fit::ok();
  }

  if (ehdr.e_ident[EI_CLASS] == ELFCLASS64) {
    // Use resize here since we won't be inserting anything a.la. emplace_back, which means we need
    // the size of the vector to be initialized already so the loop below doesn't think that the
    // vector is empty.
    phdrs_.resize(ehdr.e_phnum);
    if (auto err = module_.binary_memory->ReadBytes(module_.load_address + ehdr.e_phoff,
                                                    ehdr.e_phnum * ehdr.e_phentsize, phdrs_.data());
        err.has_err()) {
      return fit::error(err);
    }
  } else {
    // Meanwhile here we have to to a translation anyway to upcast the Elf32_Phdrs to Elf64_Phdrs so
    // we can just reserve upfront and then the insertions below will increase the vector's size
    // without reallocating.
    phdrs_.reserve(ehdr.e_phnum);
    std::vector<Elf32_Phdr> phdr32_buf(ehdr.e_phnum);
    if (auto err =
            module_.binary_memory->ReadBytes(module_.load_address + ehdr.e_phoff,
                                             ehdr.e_phnum * ehdr.e_phentsize, phdr32_buf.data());
        err.has_err()) {
      return fit::error(err);
    }

    std::ranges::transform(
        phdr32_buf, std::inserter(phdrs_, phdrs_.begin()),
        [](const Elf32_Phdr& phdr32) -> Elf64_Phdr { return elf_utils::UpcastPhdr(phdr32); });
  }

  pc_begin_ = std::numeric_limits<uint64_t>::max();
  pc_end_ = std::numeric_limits<uint64_t>::min();

  for (const auto& phdr : phdrs_) {
    if (phdr.p_type == PT_LOAD) {
      // Note that we cannot limit the inspection of PT_LOAD segments to those that are marked
      // executable because this does not necessarily match the actual mapping of executable VMOs.
      // If we want to narrow the range of PCs further we should consult the |zx_info_maps_t| for
      // this process. Overflow and arithmetic errors are ignored here, we just need the minimum
      // over the entire set of PT_LOAD segments.
      std::ignore =
          safemath::CheckMin(safemath::CheckAdd(module_.load_address, phdr.p_vaddr), pc_begin_)
              .AssignIfValid(&pc_begin_);

      std::ignore =
          safemath::CheckMax(safemath::CheckAdd(module_.load_address, phdr.p_vaddr, phdr.p_memsz),
                             pc_end_)
              .AssignIfValid(&pc_end_);
    }
  }

  return fit::ok();
}

}  // namespace unwinder
