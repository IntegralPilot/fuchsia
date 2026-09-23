// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_ELF_UTILS_H_
#define SRC_LIB_UNWINDER_ELF_UTILS_H_

#include <elf.h>
#include <lib/fit/result.h>

#include "src/lib/unwinder/memory.h"

namespace unwinder::elf_utils {

enum class ElfClass : uint8_t {
  k32Bit = 1,
  k64Bit = 2,
};

Elf64_Ehdr UpcastEhdr(const Elf32_Ehdr& ehdr32);

Elf64_Phdr UpcastPhdr(const Elf32_Phdr& phdr32);

Elf64_Shdr UpcastShdr(const Elf32_Shdr& shdr32);

template <typename Ehdr>
bool VerifyElfIdentification(const Ehdr& ehdr, ElfClass expected_class) {
  return ehdr.e_ident[EI_MAG0] == ELFMAG0 && ehdr.e_ident[EI_MAG1] == ELFMAG1 &&
         ehdr.e_ident[EI_MAG2] == ELFMAG2 && ehdr.e_ident[EI_MAG3] == ELFMAG3 &&
         ehdr.e_ident[EI_CLASS] == static_cast<uint8_t>(expected_class);
}

// Searches for |target_section| in the section headers for the given ELF module. |elf_ptr| should
// point to the beginning of the ELF header when this function is called.
template <typename Ehdr, typename Shdr>
fit::result<Error, Shdr> GetSectionByName(Memory* elf, uint64_t elf_ptr,
                                          std::string_view target_section, const Ehdr& ehdr) {
  uint64_t shstr_hdr_ptr =
      elf_ptr + ehdr.e_shoff + static_cast<uint64_t>(ehdr.e_shentsize) * ehdr.e_shstrndx;
  Shdr shstr_hdr;
  if (auto err = elf->Read(shstr_hdr_ptr, shstr_hdr); err.has_err()) {
    return fit::error(err);
  }

  for (size_t i = 0; i < ehdr.e_shnum; i++) {
    Shdr shdr;
    if (auto err = elf->Read(elf_ptr + ehdr.e_shoff + ehdr.e_shentsize * i, shdr); err.has_err()) {
      return fit::error(err);
    }

    constexpr size_t kMaxSectionNameLength = 256;
    char section_name[kMaxSectionNameLength];
    if (auto err = elf->ReadString(elf_ptr + shstr_hdr.sh_offset + shdr.sh_name, section_name,
                                   kMaxSectionNameLength);
        err.has_err()) {
      return fit::error(err);
    }
    if (strncmp(target_section.data(), section_name, target_section.size()) == 0) {
      return fit::ok(shdr);
    }
  }

  return fit::error(Error("Section %s not found.", target_section.data()));
}

template <typename Ehdr, typename Phdr>
fit::result<Error, Phdr> GetSegmentByType(Memory* elf, uint64_t elf_ptr, uint32_t p_type,
                                          const Ehdr& ehdr) {
  for (size_t i = 0; i < ehdr.e_phnum; i++) {
    Phdr phdr;
    uint64_t addr = elf_ptr + ehdr.e_phoff + i * ehdr.e_phentsize;
    if (auto err = elf->Read(addr, phdr); err.has_err()) {
      return fit::error(err);
    }

    if (phdr.p_type == p_type) {
      return fit::ok(phdr);
    }
  }

  return fit::error(Error("Segment with type %d not found", p_type));
}

}  // namespace unwinder::elf_utils

#endif  // SRC_LIB_UNWINDER_ELF_UTILS_H_
