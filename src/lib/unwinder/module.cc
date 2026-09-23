// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/module.h"

#include <elf.h>

namespace unwinder {

Module::AddressSize Module::ProbeElfModuleClass() const {
  uint8_t elf_ident[EI_NIDENT];

  if (!binary_memory) {
    return Module::AddressSize::kUnknown;
  }

  binary_memory->ReadBytes(load_address, EI_NIDENT, elf_ident);

  switch (elf_ident[EI_CLASS]) {
    case ELFCLASS32:
      return Module::AddressSize::k32Bit;
    case ELFCLASS64:
      return Module::AddressSize::k64Bit;
    default:
      return Module::AddressSize::kUnknown;
  }
}

}  // namespace unwinder
