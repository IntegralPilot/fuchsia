// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/unwind.h"

#include <cstdint>
#include <cstdio>
#include <memory>
#include <utility>
#include <vector>

#include "src/lib/unwinder/arm_ehabi_unwinder.h"
#include "src/lib/unwinder/cfi_unwinder.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/fp_unwinder.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/module.h"
#include "src/lib/unwinder/plt_unwinder.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/scs_unwinder.h"
#include "src/lib/unwinder/sigreturn_unwinder.h"
#include "src/lib/unwinder/unwinder_base.h"

namespace unwinder {

std::string Frame::Describe() const {
  std::string res = "registers={" + regs.Describe() + "}  trust=";
  switch (trust) {
    case Trust::kScan:
      res += "Scan";
      break;
    case Trust::kSigReturn:
      res += "SigReturn";
      break;
    case Trust::kSCS:
      res += "SCS";
      break;
    case Trust::kFP:
      res += "FP";
      break;
    case Trust::kPLT:
      res += "PLT";
      break;
    case Trust::kArmEhAbi:
      res += "ArmEhAbi";
      break;
    case Trust::kCFI:
      res += "CFI";
      break;
    case Trust::kContext:
      res += "Context";
      break;
  }
  if (pc_is_return_address) {
    res += "  pc_is_return_address";
  }
  if (error.has_err()) {
    res += "  error=\"" + error.msg() + "\"";
  }
  return res;
}

Unwinder::Unwinder(std::span<const Module> modules)
    : module_cache_(modules), cfi_unwinder_(module_cache_) {}

std::vector<Frame> Unwinder::Unwind(Memory* stack, const Registers& registers, size_t max_depth) {
  UnavailableMemory unavailable_memory;
  if (!stack) {
    stack = &unavailable_memory;
  }

  std::vector<Frame> res = {{registers, false, Frame::Trust::kContext}};

  while (--max_depth) {
    Frame& current = res.back();

    Frame next(Registers(current.regs.arch()), /*placeholders*/ true, Frame::Trust::kCFI);

    Step(stack, current, next);

    // An undefined PC (e.g. on Linux) or 0 PC (e.g. on Fuchsia) marks the end of the unwinding.
    // Don't include this in the output because it's not a real frame and provides no information.
    // A failed unwinding will also end up with an undefined PC.
    if (uint64_t pc; next.regs.GetPC(pc).has_err() || pc == 0) {
      break;
    }

    res.push_back(std::move(next));
  }

  return res;
}

void Unwinder::Step(Memory* stack, Frame& current, Frame& next) {
  ArmEhAbiUnwinder arm_ehabi_unwinder(module_cache_);
  FramePointerUnwinder fp_unwinder(module_cache_);
  PltUnwinder plt_unwinder(module_cache_);
  ShadowCallStackUnwinder scs_unwinder(module_cache_);
  SigReturnUnwinder sigreturn_unwinder(module_cache_);

  bool success = false;
  std::string err_msg;

  // Try sigreturn first, since it will be explicitly requested via |current.is_signal_frame|. This
  // means the CFI unwinder got an S augmentation or we already successfully probed sigreturn
  // instructions for |current.pc|.
  if (current.is_signal_frame) {
    if (auto err = TryUnwinder(&sigreturn_unwinder, stack, current, next); err.ok()) {
      success = true;
    } else {
      err_msg += "SIGRETURN: " + err.msg();
    }
  }

  // For non-signal frames, try CFI first because it's the most accurate one.
  if (auto err = TryUnwinder(&cfi_unwinder_, stack, current, next); err.ok()) {
    success = true;
  } else {
    err_msg += "; CFI: " + err.msg();
  }

  // Try ArmEhAbi before the others because it will play well with CFI. Note that this is only
  // possible today by running a 32 bit ARM binary in Starnix - which will be running as a typical
  // 64 bit Fuchsia program. The unwinder implementation will only participate in unwinding if it
  // can successfully probe that the current PC is within a 32 bit ELF module. It's also possible
  // for some binaries to have both CFI and EHABI for a particular address. We want to make sure
  // that the CFI gets to go first, since that will recover the most information, if and only if the
  // CFI was not able to recover PC, we should also consult the EHABI instructions for this address
  // as well.
  uint64_t maybe_pc = 0;
  if (!success || next.regs.GetPC(maybe_pc).has_err() || maybe_pc == 0) {
    if (auto err = TryUnwinder(&arm_ehabi_unwinder, stack, current, next); err.ok()) {
      success = true;
    } else {
      err_msg += "; ARMEHABI: " + err.msg();
    }
  }

  if (!success && !current.pc_is_return_address) {
    // PLT unwinder only works for the first frame.
    if (auto err = TryUnwinder(&plt_unwinder, stack, current, next); err.ok()) {
      success = true;
    } else {
      err_msg += "; PLT: " + err.msg();
    }
  }

  // Try frame pointers before SCS because it plays well with the CFI.
  if (!success) {
    if (auto err = TryUnwinder(&fp_unwinder, stack, current, next); err.ok()) {
      success = true;
    } else {
      err_msg += "; FP: " + err.msg();
    }
  }

  // Try shadow call stacks last because it can only recover PC.
  if (!success) {
    if (auto err = TryUnwinder(&scs_unwinder, stack, current, next); err.ok()) {
      success = true;
    } else {
      err_msg += "; SCS: " + err.msg();
    }
  }

  current.fatal_error = !success;
  if (!err_msg.empty()) {
    current.error = Error(err_msg);
  }
}

AsyncUnwinder::AsyncUnwinder(AsyncMemory::Delegate* delegate, std::span<const Module> modules)
    : stack_(std::make_unique<AsyncMemory>(delegate)),
      module_cache_(modules),
      cfi_unwinder_(module_cache_) {
  // The order here is important! This will be the order that the unwinders are attempted and should
  // not be changed without careful thought.
  //
  // In general, the order goes like this:
  //   1. Always try CFI first (which will check both .debug_frame and .eh_frame sections). CFI is
  //      the most reliable unwinding method and will recover all possible register state as
  //      produced by the unwinding metadata. The CFI unwinder object is owned directly by this
  //      class rather than being instantiated as part of the vector for easier access.
  //   2. Next try the SigReturnUnwinder, which will restore all registers found in the sigcontext
  //      struct if one is found. This works well in this case since CFI can then take over the
  //      following frame again.
  //   3. Arm EH ABI comes next, since it is also metadata based and can restore as many registers
  //      as the metadata specifies. One downside of this method (which makes it inferior to CFI) is
  //      that it only captures the register state at the beginning of a function call, if SP is
  //      modified after that point then the unwinding information is no longer valid.
  //   4. After that we try frame pointers, which also work well with all of the above since both SP
  //      and PC are recovered for each frame.
  //   5. Finally try to use the shadow call stack. If it works, then any remaining frames must also
  //      be unwound from the shadow call stack as well since the SP is lost.
  unwinders_.emplace_back(std::make_unique<SigReturnUnwinder>(module_cache_));
  unwinders_.emplace_back(std::make_unique<ArmEhAbiUnwinder>(module_cache_));
  unwinders_.emplace_back(std::make_unique<PltUnwinder>(module_cache_));
  unwinders_.emplace_back(std::make_unique<FramePointerUnwinder>(module_cache_));
  unwinders_.emplace_back(std::make_unique<ShadowCallStackUnwinder>(module_cache_));
}

void AsyncUnwinder::Unwind(const Registers& registers, size_t max_depth,
                           fit::callback<void(std::vector<Frame>)> on_done) {
  if (!stack_->delegate()) {
    // Memory delegate must be provided.
    on_done({});
    return;
  }

  UnwinderBase::AsyncUnwind(
      stack_.get(), registers, max_depth,
      [this](AsyncMemory* stack, const Frame& current, fit::callback<void(Error, Frame)> cb) {
        // First try CFI.
        TryAsyncUnwinder(&cfi_unwinder_, stack, current,
                         [this, current, cb = std::move(cb)](Error err, Frame next) mutable {
                           if (err.ok()) {
                             cb(Success(), std::move(next));
                             return;
                           }
                           // If CFI failed, try other unwinders in order.
                           TryNextUnwinder(current, 0, std::move(cb));
                         });
      },
      std::move(on_done));
}

void AsyncUnwinder::TryNextUnwinder(const Frame& current, size_t index,
                                    fit::callback<void(Error, Frame)> cb) {
  if (index >= unwinders_.size()) {
    cb(Error("All unwinders failed"),
       Frame(Registers(current.regs.arch()), true, Frame::Trust::kScan));
    return;
  }

  UnwinderBase* unwinder = unwinders_[index].get();
  // Safe to capture |this| because the caller owns it and it's stable.
  stack_->delegate()->PostTask([this, current, index, cb = std::move(cb), unwinder]() mutable {
    TryAsyncUnwinder(unwinder, stack_.get(), current,
                     [this, current, index, cb = std::move(cb)](Error err, Frame next) mutable {
                       if (err.ok()) {
                         cb(Success(), std::move(next));
                         return;
                       }
                       TryNextUnwinder(current, index + 1, std::move(cb));
                     });
  });
}

std::vector<Frame> Unwind(Memory* memory, const std::vector<uint64_t>& modules,
                          const Registers& registers, size_t max_depth) {
  std::vector<Module> converted;
  converted.reserve(modules.size());
  for (const auto& addr : modules) {
    converted.emplace_back(addr, memory, Module::AddressMode::kProcess);
  }

  return Unwinder(converted).Unwind(memory, registers, max_depth);
}

}  // namespace unwinder
