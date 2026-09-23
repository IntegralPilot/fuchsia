// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_ARM_EHABI_UNWINDER_H_
#define SRC_LIB_UNWINDER_ARM_EHABI_UNWINDER_H_

#include "src/lib/unwinder/arm_ehabi_module.h"
#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

// Unwinder implementation that supports unwinding from .ARM extension unwinding tables as specified
// in the ARM EHABI32 specification:
// https://github.com/ARM-software/abi-aa/blob/c51addc3dc03e73a016a1e4edf25440bcac76431/ehabi32/ehabi32.rst
class ArmEhAbiUnwinder : public UnwinderBase {
 public:
  // ElfModuleCache is needed for |IsValidPC| and |GetModuleForPc|.
  explicit ArmEhAbiUnwinder(const ElfModuleCache& module_cache) : UnwinderBase(module_cache) {}

  Error Step(Memory* stack, const Frame& current, Frame& next) override;

  void AsyncStep(AsyncMemory* stack, const Frame& current,
                 fit::callback<void(Error, Registers)> cb) override;

  Frame::Trust trust() const override { return Frame::Trust::kArmEhAbi; }

 private:
  Error Step(Memory* stack, const LoadedElfModule& loaded_elf_module, const Registers& current,
             Registers& next);

  void AsyncStep(AsyncMemory* stack, const LoadedElfModule& elf_module, const Registers& current,
                 fit::callback<void(Error, Registers)> cb);

  // Looksup the EhAbiModule.
  using ArmEhAbiModuleRef = std::reference_wrapper<const ArmEhAbiModule>;
  fit::result<Error, ArmEhAbiModuleRef> GetEhAbiModuleFromModuleInfo(
      const LoadedElfModule& elf_module);

  // Lazily loaded.
  std::map<uint32_t, std::unique_ptr<ArmEhAbiModule>> module_map_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_ARM_EHABI_UNWINDER_H_
