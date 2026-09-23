// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_CFI_UNWINDER_H_
#define SRC_LIB_UNWINDER_CFI_UNWINDER_H_

#include <lib/fit/function.h>

#include <cstdint>
#include <functional>
#include <map>
#include <memory>

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

class CfiModule;

class CfiUnwinder : public UnwinderBase {
 public:
  explicit CfiUnwinder(const ElfModuleCache& elf_module_cache);

  ~CfiUnwinder();

  Error Step(Memory* stack, const Frame& current, Frame& next) override;

  void AsyncStep(AsyncMemory* stack, const Frame& current,
                 fit::callback<void(Error, Registers)> cb) override;

  Frame::Trust trust() const override { return Frame::Trust::kCFI; }

 private:
  // If the returned value is fit::ok, then the contained boolean indicates whether the next frame
  // is a signal frame or not. Otherwise the encased Error type will have more information.
  fit::result<Error, bool> Step(Memory* stack, const Registers& current, Registers& next,
                                bool is_return_address);

  void AsyncStep(AsyncMemory* stack, const Registers& current, bool is_return_address,
                 fit::callback<void(Error, Registers)> cb);

  fit::result<Error, Registers> ConvertTo32BitIfNeeded(uint64_t pc, const Registers& current);

  using CfiModuleRef = std::reference_wrapper<const CfiModule>;
  fit::result<Error, CfiModuleRef> GetCfiModuleInfoForPc(uint64_t pc);

  // Mapping from module load addresses to a pair of (module description, lazily-initialized CFI
  // modules for the binary and optional debugging info).
  std::map<uint64_t, std::unique_ptr<CfiModule>> module_map_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_CFI_UNWINDER_H_
