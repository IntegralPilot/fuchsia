// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_LOADED_ELF_MODULE_H_
#define SRC_LIB_UNWINDER_LOADED_ELF_MODULE_H_

#include <elf.h>
#include <lib/fit/result.h>

#include <cstdint>
#include <vector>

#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/module.h"

namespace unwinder {

class Memory;

// Provides a high level interface over a |Module|. Eventually these classes will be merged.
//
// A LoadedElfModule can represent either a literally loaded ELF program in a program's address
// space, or an ELF file (or files) on disk, depending on the corresponding Module's |mode|. In
// either case, there is always a live process associated with this object that has a load address.
//
// This class assumes that is has synchronous access to both |binary_memory| and |debug_info_memory|
// members of the corresponding Module object.
class LoadedElfModule {
 public:
  explicit LoadedElfModule(const Module& module) : module_(module) {}

  // Load ELF headers and program headers.
  [[nodiscard]] fit::result<Error> Load();

  // Check whether a given PC is in the valid range of this module.
  bool IsValidPC(uint64_t pc) const { return pc >= pc_begin_ && pc < pc_end_; }

  const Module& module() const { return module_; }
  uint64_t load_address() const { return module_.load_address; }
  Memory* binary_memory() const { return module_.binary_memory; }
  Memory* debug_info_memory() const { return module_.debug_info_memory; }
  Module::AddressMode mode() const { return module_.mode; }
  Module::AddressSize size() const { return module_.size; }

  const std::vector<Elf64_Phdr>& phdrs() const { return phdrs_; }

  // Helpers to get sections or segments.
  [[nodiscard]] fit::result<Error, Elf64_Shdr> GetSectionByName(
      std::string_view target_section) const;

  // Always returns an Elf64_Phdr, even if this module is 32 bits.
  [[nodiscard]] fit::result<Error, Elf64_Phdr> GetSegmentByType(uint32_t p_type) const;

 private:
  // Loads the ELF header to |ehdr_| (upcasting if necessary). |module_.binary_memory| is assumed to
  // be valid when this function is called.
  fit::result<Error> LoadElfHeader();

  // Loads all Program Headers to |phdrs_| (upcasting them if necessary). |module_.binary_memory| is
  // assumed to be valid when this function is called.
  fit::result<Error> LoadPhdrs(const Elf64_Ehdr& ehdr);

  const Module module_;

  // Marks the executable section.
  uint64_t pc_begin_ = 0;  // inclusive
  uint64_t pc_end_ = 0;    // exclusive

  std::optional<Elf64_Ehdr> ehdr_;
  std::vector<Elf64_Phdr> phdrs_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_LOADED_ELF_MODULE_H_
