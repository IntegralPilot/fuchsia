// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_UNWINDER_BASE_H_
#define SRC_LIB_UNWINDER_UNWINDER_BASE_H_

#include <vector>

#include "sdk/lib/fit/include/lib/fit/function.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/frame.h"

namespace unwinder {

class AsyncMemory;
class ElfModuleCache;
class Memory;

// Base class for all unwinders.
class UnwinderBase {
 public:
  explicit UnwinderBase(const ElfModuleCache& module_cache) : module_cache_(module_cache) {}
  virtual ~UnwinderBase() = default;

  // Unwind one frame, populating |next| with the new register values. |next| is invalid if an error
  // is returned.
  virtual Error Step(Memory* stack, const Frame& current, Frame& next) = 0;

  // Unwind one frame, possibly asynchronously. The callback is issued with the resulting registers
  // for the new frame on success. If the error is populated, the registers are not valid.
  virtual void AsyncStep(AsyncMemory* stack, const Frame& current,
                         fit::callback<void(Error, Registers)> cb) = 0;

  // Unwind the entire stack using only this unwinder.
  std::vector<Frame> Unwind(Memory* stack, const Registers& registers, size_t max_depth = 50);

  // Unwind the entire stack asynchronously using only this unwinder.
  void AsyncUnwind(AsyncMemory* stack, const Registers& registers, size_t max_depth,
                   fit::callback<void(std::vector<Frame>)> on_done);

  using AsyncStepFunc = fit::function<void(AsyncMemory* stack, const Frame& current,
                                           fit::callback<void(Error, Frame)> cb)>;

  // Unwind the entire stack asynchronously using a custom step function.
  static void AsyncUnwind(AsyncMemory* stack, const Registers& registers, size_t max_depth,
                          AsyncStepFunc step_func, fit::callback<void(std::vector<Frame>)> on_done);

  // The trust that should be associated with this unwinder.
  virtual Frame::Trust trust() const = 0;

  const ElfModuleCache& module_cache() const { return module_cache_; }

 private:
  const ElfModuleCache& module_cache_;
};

// Shared logic to try a specific unwinder and perform fixups.
Error TryUnwinder(UnwinderBase* unwinder, Memory* stack, const Frame& current, Frame& next);

// Shared logic to try a specific unwinder asynchronously and perform fixups.
void TryAsyncUnwinder(UnwinderBase* unwinder, AsyncMemory* stack, const Frame& current,
                      fit::callback<void(Error, Frame)> cb);

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_UNWINDER_BASE_H_
