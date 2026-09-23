// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/unwinder_base.h"

#include <utility>

#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/sigreturn_unwinder.h"

namespace unwinder {

namespace {

bool DoneUnwinding(const Registers& regs) {
  // An undefined PC (e.g. on Linux) or 0 PC (e.g. on Fuchsia) marks the end of the unwinding.
  // Don't include this in the output because it's not a real frame and provides no information.
  // A failed unwinding will also end up with an undefined PC.
  if (uint64_t pc; regs.GetPC(pc).has_err() || pc == 0) {
    return true;
  }

  return false;
}

bool PcIsReturnAddress(const Registers& regs) {
  // If |regs| is recovered from a regular function call, rax/lr/ra will be scratched.
  // Otherwise, they will be available.
  RegisterID reg_id;
  switch (regs.arch()) {
    case Registers::Arch::kX64:
      reg_id = RegisterID::kX64_rax;
      break;
    case Registers::Arch::kArm32:
      reg_id = RegisterID::kArm32_lr;
      break;
    case Registers::Arch::kArm64:
      reg_id = RegisterID::kArm64_lr;
      break;
    case Registers::Arch::kRiscv64:
      reg_id = RegisterID::kRiscv64_ra;
      break;
  }
  uint64_t val;

  return regs.Get(reg_id, val).has_err();
}

void FixupFrame(const ElfModuleCache& module_cache, Frame& next) {
  // If the frame was identified with an S augmentation by the CFI unwinder, then we know that
  // this definitely not a return address.
  if (next.is_signal_frame) {
    next.pc_is_return_address = false;
    return;
  }

  // Successfully probing a sigreturn frame means the next frame needs to be unwound by the
  // sigreturn unwinder. Only do this if the CFI unwinder failed to detect the 'S' augmentation.
  if (!next.is_signal_frame) {
    if (auto err = SigReturnUnwinder::ProbePCForSigReturn(module_cache, next.regs); err.ok()) {
      next.pc_is_return_address = false;
      next.is_signal_frame = true;
      return;
    }
  }

  // Otherwise defer to the value of the return address register.
  if (next.trust == Frame::Trust::kCFI) {
    next.pc_is_return_address = PcIsReturnAddress(next.regs);
  } else if (next.trust == Frame::Trust::kSigReturn) {
    next.pc_is_return_address = false;
  } else {
    next.pc_is_return_address = true;
  }
}

}  // namespace

Error TryUnwinder(UnwinderBase* unwinder, Memory* stack, const Frame& current, Frame& next) {
  auto err = unwinder->Step(stack, current, next);

  if (err.has_err()) {
    return err;
  }

  next.trust = unwinder->trust();
  FixupFrame(unwinder->module_cache(), next);

  return Success();
}

void TryAsyncUnwinder(UnwinderBase* unwinder, AsyncMemory* stack, const Frame& current,
                      fit::callback<void(Error, Frame)> cb) {
  unwinder->AsyncStep(stack, current, [=, cb = std::move(cb)](Error err, Registers next) mutable {
    Frame next_frame(std::move(next), false, unwinder->trust());

    if (err.has_err()) {
      next_frame.error = err;
      cb(err, std::move(next_frame));
      return;
    }

    next_frame.trust = unwinder->trust();
    FixupFrame(unwinder->module_cache(), next_frame);
    cb(Success(), std::move(next_frame));
    return;
  });
}

namespace {

// Internal class to handle async unwinding loop.
class AsyncUnwindLoop {
 public:
  AsyncUnwindLoop(AsyncMemory* stack, UnwinderBase::AsyncStepFunc step_func)
      : stack_(stack), step_func_(std::move(step_func)) {}

  void Unwind(const Registers& registers, size_t max_depth,
              fit::callback<void(std::vector<Frame>)> cb) {
    max_depth_ = max_depth;
    on_done_ = std::move(cb);
    result_ = {{registers, false, Frame::Trust::kContext}};

    uint64_t sp;
    if (auto err = registers.GetSP(sp); err.has_err()) {
      on_done_(std::move(result_));
      return;
    }

    constexpr uint32_t kDefaultStackSize = 8192;
    stack_->FetchMemoryRanges({{sp, kDefaultStackSize}}, [this]() { Step(); });
  }

 private:
  void Step() {
    if (result_.size() >= max_depth_) {
      on_done_(std::move(result_));
      return;
    }

    const Frame& current = result_.back();
    step_func_(stack_, current, [this](const Error& err, Frame next) {
      if (err.has_err()) {
        result_.back().fatal_error = true;
        result_.back().error = err;
        on_done_(std::move(result_));
        return;
      }

      if (DoneUnwinding(next.regs)) {
        on_done_(std::move(result_));
        return;
      }

      result_.push_back(std::move(next));
      stack_->delegate()->PostTask([this]() { Step(); });
    });
  }

  AsyncMemory* stack_;
  UnwinderBase::AsyncStepFunc step_func_;
  size_t max_depth_ = 0;
  std::vector<Frame> result_;
  fit::callback<void(std::vector<Frame>)> on_done_;
};

}  // namespace

void UnwinderBase::AsyncUnwind(AsyncMemory* stack, const Registers& registers, size_t max_depth,
                               fit::callback<void(std::vector<Frame>)> on_done) {
  AsyncUnwind(
      stack, registers, max_depth,
      [this](AsyncMemory* stack, const Frame& current, fit::callback<void(Error, Frame)> cb) {
        TryAsyncUnwinder(this, stack, current, std::move(cb));
      },
      std::move(on_done));
}

void UnwinderBase::AsyncUnwind(AsyncMemory* stack, const Registers& registers, size_t max_depth,
                               AsyncStepFunc step_func,
                               fit::callback<void(std::vector<Frame>)> on_done) {
  if (!stack) {
    on_done({});
    return;
  }

  auto loop = std::make_unique<AsyncUnwindLoop>(stack, std::move(step_func));
  loop->Unwind(registers, max_depth,
               [loop = std::move(loop), on_done = std::move(on_done)](
                   std::vector<Frame> frames) mutable { on_done(std::move(frames)); });
}

std::vector<Frame> UnwinderBase::Unwind(Memory* stack, const Registers& registers,
                                        size_t max_depth) {
  UnavailableMemory unavailable_memory;
  if (!stack) {
    stack = &unavailable_memory;
  }

  std::vector<Frame> res = {{registers, false, Frame::Trust::kContext}};

  while (--max_depth) {
    Frame& current = res.back();

    Frame next(Registers(current.regs.arch()), /*pc_is_return_address=*/true, trust());

    if (auto err = TryUnwinder(this, stack, current, next); err.has_err()) {
      current.fatal_error = true;
      current.error = err;
      break;
    }

    if (DoneUnwinding(next.regs)) {
      break;
    }

    res.push_back(std::move(next));
  }

  return res;
}

}  // namespace unwinder
