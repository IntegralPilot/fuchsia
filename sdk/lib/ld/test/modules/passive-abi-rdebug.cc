// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>

#include <cstdint>

#include "test-start.h"

using Ehdr = elfldltl::Elf<>::Ehdr;

extern "C" int64_t TestStart() {
  if (ld::abi::_r_debug.version != elfldltl::kRDebugVersion) {
    return 1;
  }

  if (ld::abi::_r_debug.state != elfldltl::RDebugState::kConsistent) {
    return 2;
  }

  if (ld::abi::_r_debug.map.get() != &ld::abi::_ld_abi.loaded_modules->link_map) {
    return 3;
  }

  // The dynamic linker is linked at vaddr 0.  Its load bias is always nonzero.
  const uintptr_t ld_base = ld::abi::_r_debug.ldbase;
  if (ld_base == 0) {
    return 4;
  }

  // Since it's linked at vaddr 0, its load bias is really its rutnime base
  // address where its Ehdr lies.
  if (!reinterpret_cast<const Ehdr*>(ld_base)->Loadable()) {
    return 4;
  }

  return 17;
}
