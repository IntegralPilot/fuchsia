// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/elf_utils.h"

namespace unwinder::elf_utils {

Elf64_Ehdr UpcastEhdr(const Elf32_Ehdr& ehdr32) {
  Elf64_Ehdr ehdr;
  memcpy(ehdr.e_ident, ehdr32.e_ident, EI_NIDENT);
  ehdr.e_type = ehdr32.e_type;
  ehdr.e_machine = ehdr32.e_machine;
  ehdr.e_version = ehdr32.e_version;
  ehdr.e_entry = ehdr32.e_entry;
  ehdr.e_phoff = ehdr32.e_phoff;
  ehdr.e_shoff = ehdr32.e_shoff;
  ehdr.e_flags = ehdr32.e_flags;
  ehdr.e_ehsize = ehdr32.e_ehsize;
  ehdr.e_phentsize = ehdr32.e_phentsize;
  ehdr.e_phnum = ehdr32.e_phnum;
  ehdr.e_shentsize = ehdr32.e_shentsize;
  ehdr.e_shnum = ehdr32.e_shnum;
  ehdr.e_shstrndx = ehdr32.e_shstrndx;
  return ehdr;
}

Elf64_Phdr UpcastPhdr(const Elf32_Phdr& phdr32) {
  Elf64_Phdr phdr;
  phdr.p_type = phdr32.p_type;
  phdr.p_offset = phdr32.p_offset;
  phdr.p_vaddr = phdr32.p_vaddr;
  phdr.p_paddr = phdr32.p_paddr;
  phdr.p_filesz = phdr32.p_filesz;
  phdr.p_memsz = phdr32.p_memsz;
  phdr.p_flags = phdr32.p_flags;
  phdr.p_align = phdr32.p_align;
  return phdr;
}

Elf64_Shdr UpcastShdr(const Elf32_Shdr& shdr32) {
  Elf64_Shdr shdr;
  shdr.sh_name = shdr32.sh_name;
  shdr.sh_type = shdr32.sh_type;
  shdr.sh_flags = shdr32.sh_flags;
  shdr.sh_addr = shdr32.sh_addr;
  shdr.sh_offset = shdr32.sh_offset;
  shdr.sh_link = shdr32.sh_link;
  shdr.sh_info = shdr32.sh_info;
  shdr.sh_addralign = shdr32.sh_addralign;
  shdr.sh_entsize = shdr32.sh_entsize;
  return shdr;
}

}  // namespace unwinder::elf_utils
