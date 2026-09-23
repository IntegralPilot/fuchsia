// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_TESTING_MOCK_UNWINDER_H_
#define SRC_LIB_UNWINDER_TESTING_MOCK_UNWINDER_H_

#include <vector>

#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

// A fake implementation of UnwinderBase.
// It is called "MockUnwinder" to align with zxdb naming conventions.
//
// You can specify exactly what frames the unwinder will produce by
// seeding it with a predefined stack of frames.
//
// Example usage:
//   ElfModuleCache module_cache({});
//   MockUnwinder mock_unwinder(module_cache);
//
//   Frame next_expected(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
//   next_expected.regs.SetPC(0x1234);
//   mock_unwinder.SetFrames({std::move(next_expected)});
class MockUnwinder : public UnwinderBase {
 public:
  explicit MockUnwinder(const ElfModuleCache& module_cache) : UnwinderBase(module_cache) {}

  Error Step(Memory* stack, const Frame& current, Frame& next) override {
    if (step_error_.has_err()) {
      return step_error_;
    }
    if (current_frame_idx_ >= frames_.size()) {
      next.regs.SetPC(0);  // Mark end of stack
      return Success();
    }
    next = frames_[current_frame_idx_++];
    return Success();
  }

  void AsyncStep(AsyncMemory* stack, const Frame& current,
                 fit::callback<void(Error, Registers)> cb) override {
    if (step_error_.has_err()) {
      cb(step_error_, Registers(current.regs.arch()));
      return;
    }
    if (current_frame_idx_ >= frames_.size()) {
      Registers next_regs(current.regs.arch());
      next_regs.SetPC(0);  // Mark end of stack
      cb(Success(), std::move(next_regs));
      return;
    }
    Registers next_regs = frames_[current_frame_idx_++].regs;
    cb(Success(), std::move(next_regs));
  }

  Frame::Trust trust() const override { return trust_; }

  void SetFrames(std::vector<Frame> frames) {
    frames_ = std::move(frames);
    current_frame_idx_ = 0;
  }

  void SetStepError(Error err) { step_error_ = err; }

  void SetTrust(Frame::Trust trust) { trust_ = trust; }

 private:
  std::vector<Frame> frames_;
  size_t current_frame_idx_ = 0;
  Error step_error_ = Success();
  Frame::Trust trust_ = Frame::Trust::kCFI;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_TESTING_MOCK_UNWINDER_H_
