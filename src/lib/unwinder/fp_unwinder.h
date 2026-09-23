// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_FP_UNWINDER_H_
#define SRC_LIB_UNWINDER_FP_UNWINDER_H_

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

// Unwind from the frame pointer. There's no reliable way to detect whether
// a function has frame pointer enabled, so we try our best.
class FramePointerUnwinder : public UnwinderBase {
 public:
  explicit FramePointerUnwinder(const ElfModuleCache& module_cache) : UnwinderBase(module_cache) {}

  Error Step(Memory* stack, const Frame& current, Frame& next) override;
  void AsyncStep(AsyncMemory* stack, const Frame& current,
                 fit::callback<void(Error, Registers)> cb) override;
  Frame::Trust trust() const override { return Frame::Trust::kFP; }

 private:
  Error Step(Memory* stack, const Registers& current, Registers& next,
             const LoadedElfModule& loaded_elf_module);
  void AsyncStep(AsyncMemory* stack, const Registers& current,
                 const LoadedElfModule& loaded_elf_module,
                 fit::callback<void(Error, Registers)> cb);
  Error ReadNextFpAndSp(Memory* stack, uint64_t& fp, uint64_t& next_fp, uint64_t& next_pc,
                        const LoadedElfModule& loaded_elf_module);
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_FP_UNWINDER_H_
