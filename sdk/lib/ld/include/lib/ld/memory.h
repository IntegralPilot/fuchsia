// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_MEMORY_H_
#define LIB_LD_MEMORY_H_

#include <lib/elfldltl/memory.h>

#include "module.h"

namespace ld {

// This provides a Memory API object that represents the Module already loaded.
class ModuleMemory : public elfldltl::DirectMemory {
 public:
  ModuleMemory() = delete;

  ModuleMemory(const ModuleMemory&) = default;

  template <class Module>
  explicit ModuleMemory(const Module& module)
      : elfldltl::DirectMemory{Image(module), module.vaddr_start - module.link_map.addr} {}

  ModuleMemory& operator=(const ModuleMemory&) = default;

 private:
  template <class Module>
  static std::span<std::byte> Image(const Module& module) {
    uintptr_t start = module.vaddr_start;
    assert(module.vaddr_end >= module.vaddr_start);
    size_t size = module.vaddr_end - module.vaddr_start;
    return {reinterpret_cast<std::byte*>(start), size};
  }
};

}  // namespace ld

#endif  // LIB_LD_MEMORY_H_
