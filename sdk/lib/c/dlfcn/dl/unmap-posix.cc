// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mman.h>

#include "runtime-module.h"

namespace dl {

RuntimeModule::~RuntimeModule() {
  delete[] name_.c_str();
  if (!no_delete_ && vaddr_size() > 0) {
    munmap(reinterpret_cast<void*>(static_cast<uintptr_t>(abi_module_.vaddr_start)), vaddr_size());
  }
}

}  // namespace dl
