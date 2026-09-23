// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/elf_module_cache.h"

#include <cinttypes>

#include "src/lib/unwinder/loaded_elf_module.h"

namespace unwinder {

ElfModuleCache::ElfModuleCache(std::span<const Module> modules) {
  for (const auto& module : modules) {
    module_map_.emplace(module.load_address, std::make_unique<LoadedElfModule>(module));
  }
}

fit::result<Error, ElfModuleCache::LoadedElfModuleRef> ElfModuleCache::GetLoadedElfModuleForPc(
    uint64_t pc) const {
  if (module_map_.empty()) {
    return fit::error(Error("No modules."));
  }

  auto it = module_map_.upper_bound(pc);
  if (it == module_map_.begin()) {
    return fit::error(Error("%#" PRIx64 " is not covered by any module", pc));
  }
  it--;

  LoadedElfModule* loaded_elf_module = it->second.get();
  if (auto err = loaded_elf_module->Load(); err.is_error()) {
    return err.take_error();
  }

  if (!loaded_elf_module->IsValidPC(pc)) {
    return fit::error(Error("%#" PRIx64 " is not a valid PC in module %#" PRIx64, pc,
                            loaded_elf_module->load_address()));
  }

  return fit::ok(LoadedElfModuleRef(*loaded_elf_module));
}

bool ElfModuleCache::IsValidPC(uint64_t pc) const { return GetLoadedElfModuleForPc(pc).is_ok(); }

}  // namespace unwinder
