// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_SCS_UNWINDER_H_
#define SRC_LIB_UNWINDER_SCS_UNWINDER_H_

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

// Unwind from Shadow Call Stacks, only available on arm64.
//
// It's inherently unreliable to unwind from a shadow call stack, because
//  1) The shadow call stack provides nothing other than return addresses, so it's not possible to
//     unwind the following frames with other unwinders.
//  2) A function can choose not to implement shadow call stack, e.g. a library compiled without
//     SCS, and the unwinder has no way to detect; those frames will be dropped silently.
class ShadowCallStackUnwinder : public UnwinderBase {
 public:
  // We need |ElfModuleCache::IsValidPC|.
  explicit ShadowCallStackUnwinder(const ElfModuleCache& module_cache)
      : UnwinderBase(module_cache) {}

  Error Step(Memory* scs, const Frame& current, Frame& next) override;
  void AsyncStep(AsyncMemory* scs, const Frame& current,
                 fit::callback<void(Error, Registers)> cb) override;
  Frame::Trust trust() const override { return Frame::Trust::kSCS; }

 private:
  Error Step(Memory* scs, uint64_t scs_pointer, Registers& next);
  void AsyncStep(AsyncMemory* scs, uint64_t scs_pointer, Registers::Arch arch,
                 fit::callback<void(Error, Registers)> cb);

  // Used for asynchronous methods to attempt to fetch the entire SCS at once instead of each time
  // |AsyncStep| is called.
  bool should_synchronize_scs_ = true;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_SCS_UNWINDER_H_
