// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/init-fini.h>
#include <lib/ld/abi.h>

#include "test-start.h"

namespace {

using PreinitFunction = elfldltl::InitFiniFunction*;

int call_state = 0;

void Preinit1() {
  if (call_state == 0) {
    call_state = 1;
  }
}

void Preinit2() {
  if (call_state == 1) {
    call_state = 2;
  }
}

[[gnu::aligned(sizeof(elfldltl::InitFiniFunction*)), gnu::used, gnu::retain,
  gnu::section(".preinit_array")]] extern const PreinitFunction kPreinit[] = {
    Preinit1,
    Preinit2,
};

}  // namespace

extern "C" int64_t TestStart() {
  const std::span preinit = ld::abi::_ld_abi.preinit_array;

  if (preinit.empty()) {
    return 0;
  }
  if (preinit.size() != 2) {
    return 1;
  }
  if (preinit[0] != reinterpret_cast<uintptr_t>(Preinit1)) {
    return 2;
  }
  if (preinit[1] != reinterpret_cast<uintptr_t>(Preinit2)) {
    return 3;
  }

  if (call_state != 0) {
    return 4;
  }

  // The bias argument doesn't matter since there is no unrelocated legacy.
  elfldltl::InitFiniInfo<elfldltl::Elf<>>{preinit}.CallInit(0);
  if (call_state != 2) {
    return 5;
  }

  return 17;
}
