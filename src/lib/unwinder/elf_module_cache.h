// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_ELF_MODULE_CACHE_H_
#define SRC_LIB_UNWINDER_ELF_MODULE_CACHE_H_

#include <lib/fit/result.h>

#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <span>

#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/loaded_elf_module.h"

namespace unwinder {

// Provides a high level interface over a collection of ELF modules that are loaded for a particular
// process for checking PC values and returning specific ELF modules corresponding to a particular
// PC value. See LoadedElfModule for more details about what to do with a specific loaded ELF
// module.
class ElfModuleCache {
 public:
  explicit ElfModuleCache(std::span<const Module> modules);

  // Find the ElfModule for the given PC.
  using LoadedElfModuleRef = std::reference_wrapper<const LoadedElfModule>;
  [[nodiscard]] fit::result<Error, LoadedElfModuleRef> GetLoadedElfModuleForPc(uint64_t pc) const;

  // Check whether a given PC is in any known module's valid range.
  bool IsValidPC(uint64_t pc) const;

 private:
  std::map<uint64_t, std::unique_ptr<LoadedElfModule>> module_map_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_ELF_MODULE_CACHE_H_
