// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/scs_unwinder.h"

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

Error ShadowCallStackUnwinder::Step(Memory* scs, const Frame& current, Frame& next) {
  uint64_t scs_pointer;
  if (auto err = current.regs.GetSCS(scs_pointer); err.has_err()) {
    return err;
  }

  if (!scs_pointer) {
    return Error("shadow call stack is not available");
  }

  return Step(scs, scs_pointer, next.regs);
}

Error ShadowCallStackUnwinder::Step(Memory* scs, uint64_t scs_pointer, Registers& next) {
  // The shadow call stack is pushed/popped via (e.g., on arm64)
  //
  //    str     x30, [x18], #8    ; post-indexed
  //    ...
  //    ldr     x30, [x18, #-8]!  ; pre-indexed
  //
  // So x18 points to the next available slots. The same applies to riscv64.
  uint64_t ra;
  if (auto err = scs->Read(scs_pointer - 8, ra); err.has_err()) {
    return err;
  }

  // A zero ra indicates the beginning of the shadow call stack.
  if (!ra) {
    return Success();
  }
  if (!module_cache().IsValidPC(ra)) {
    return Error("Invalid shadow call stack");
  }

  next.SetPC(ra);
  next.SetSCS(scs_pointer - 8);
  return Success();
}

void ShadowCallStackUnwinder::AsyncStep(AsyncMemory* scs, const Frame& current,
                                        fit::callback<void(Error, Registers)> cb) {
  uint64_t scs_pointer;
  if (auto err = current.regs.GetSCS(scs_pointer); err.has_err()) {
    cb(err, Registers(current.regs.arch()));
    return;
  }

  if (!scs_pointer) {
    cb(Error("shadow call stack is not available"), Registers(current.regs.arch()));
    return;
  }

  AsyncStep(scs, scs_pointer, current.regs.arch(), std::move(cb));
}

void ShadowCallStackUnwinder::AsyncStep(AsyncMemory* scs, uint64_t scs_pointer,
                                        Registers::Arch arch,
                                        fit::callback<void(Error, Registers)> cb) {
  Registers next(arch);

  if (should_synchronize_scs_) {
    should_synchronize_scs_ = false;

    // 4KiB should be more than enough. On 64 bit platforms, this is enough for ~500 SCS entries.
    constexpr uint32_t kDefaultFetchSize = 4 * 1024;
    scs->FetchMemoryRanges({{scs_pointer, kDefaultFetchSize}}, [=, cb = std::move(cb)]() mutable {
      auto err = Step(scs, scs_pointer, next);
      cb(err, std::move(next));
    });
  } else {
    // Memory should already be available, call the synchronous |Step|.
    auto err = Step(scs, scs_pointer, next);
    cb(err, std::move(next));
  }
}

}  // namespace unwinder
