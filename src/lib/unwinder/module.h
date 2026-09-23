// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_MODULE_H_
#define SRC_LIB_UNWINDER_MODULE_H_

#include <cstdint>
#include <cstring>

#include "src/lib/unwinder/memory.h"

namespace unwinder {

// An ELF module.
struct Module {
  // AddressMode determines the layout of ELF structures.
  enum class AddressMode {
    kProcess,  // Mapped in a running process. Data will be read from a vaddr.
    kFile,     // Packed as a file. Data will be read from an offset.
  };

  enum class AddressSize {
    k32Bit,
    k64Bit,
    kUnknown,
  };

  // The load address.
  uint64_t load_address;

  // Binary data accessor from either a live process or a local ELF file. Cannot be null.
  Memory* binary_memory;
  // Debug info data accessor for an ELF file containing DWARF debugging info. Optionally null.
  Memory* debug_info_memory;

  // Address mode.
  AddressMode mode;

  AddressSize size;

  Module(uint64_t addr, Memory* binary, AddressMode mod)
      : load_address(addr),
        binary_memory(binary),
        debug_info_memory(nullptr),
        mode(mod),
        size(ProbeElfModuleClass()) {}

  Module(uint64_t addr, Memory* binary, Memory* debug_info, AddressMode mod)
      : load_address(addr),
        binary_memory(binary),
        debug_info_memory(debug_info),
        mode(mod),
        size(ProbeElfModuleClass()) {}

 private:
  // The ELF Identifier Class field tells us if this binary is 32 or 64 bit.
  AddressSize ProbeElfModuleClass() const;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_MODULE_H_
