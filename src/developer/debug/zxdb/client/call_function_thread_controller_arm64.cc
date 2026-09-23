// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/call_function_thread_controller_arm64.h"

#include <vector>

#include <memory>
#include <utility>

#include "src/developer/debug/shared/register_id.h"
#include "src/developer/debug/shared/register_info.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/common/join_callbacks.h"
#include "src/developer/debug/zxdb/expr/eval_callback.h"

namespace zxdb {

CallFunctionThreadControllerArm64::CallFunctionThreadControllerArm64(
    const AddressRanges& range, const std::vector<ExprValue>& parameters,
    EvalCallback on_function_completed, fit::deferred_callback on_done)
    : CallFunctionThreadController(range, parameters, std::move(on_function_completed),
                                   std::move(on_done)),
      weak_factory_(this) {}

CallFunctionThreadControllerArm64::~CallFunctionThreadControllerArm64() = default;

void CallFunctionThreadControllerArm64::InitWithThread(Thread* thread,
                                                       fit::callback<void(const Err&)> cb) {
  SetThread(thread);
  Log("CallFunctionThreadControllerArm64::InitWithThread");

  CollectAllRegisterCategories(
      thread, [weak_this = weak_factory_.GetWeakPtr(), weak_thread = thread->GetWeakPtr(),
               cb = std::move(cb)](const Err& err) mutable {
        if (!weak_this) {
          return cb(Err("CallFunctionThreadControllerArm64 disappeared."));
        } else if (!weak_thread) {
          return cb(Err("thread disappeared."));
        }

        // This will be the new PC of the stack frame we create.
        uint64_t function_start_addr = weak_this->address_ranges_.begin()->begin();

        uint64_t sp = GetRegisterData(weak_this->general_registers_, debug::RegisterID::kARMv8_sp);

        // Guarantee that SP is aligned.
        sp &= ~0xf;

        // Build and push a new stack frame to this thread's stack. Note at this point
        // this is only local to zxdb and the target has no idea about this frame
        // until we write the stack pointer register below.
        Frame* frame =
            weak_this->PushStackFrame(function_start_addr, sp, weak_this->general_registers_);
        if (!frame) {
          return cb(Err("Failed to create new stack frame."));
        }

        weak_this->Log("Pushed new stack frame pc = 0x%x sp = 0x%x", function_start_addr, sp);

        // Write the stack pointer, link register, and pc in the new frame and send it
        // to the target.
        if (!WriteRegister(weak_this->general_registers_, debug::RegisterID::kARMv8_sp, sp)) {
          return cb(Err("Failed to find SP in the register set"));
        }
        if (!WriteRegister(
                weak_this->general_registers_, debug::RegisterID::kARMv8_lr,
                GetRegisterData(weak_this->general_registers_, debug::RegisterID::kARMv8_pc))) {
          return cb(Err("Failed to find LR in the register set"));
        }
        if (!WriteRegister(weak_this->general_registers_, debug::RegisterID::kARMv8_pc,
                           function_start_addr)) {
          return cb(Err("Failed to find PC in the register set"));
        }

        // Load all the parameters to registers, and then actually do the bulk register write.
        // Currently parameters are only supported when passed via registers.
        weak_this->WriteParametersToRegisters();

        return weak_this->WriteGeneralRegisters(std::move(cb));
      });
}

void CallFunctionThreadControllerArm64::CollectAllRegisterCategories(
    Thread* thread, fit::callback<void(const Err& err)> cb) {
  auto joiner = fxl::MakeRefCounted<JoinCallbacks<RegisterCollection>>();

  // Aarch64 doesn't have floating point registers, they are included in the
  // vector category.
  for (auto category : {debug::RegisterCategory::kGeneral, debug::RegisterCategory::kVector}) {
    thread->GetStack()[0]->GetRegisterCategoryAsync(
        category, true,
        [category, cb = joiner->AddCallback()](
            const Err& err, const std::vector<debug::RegisterValue>& regs) mutable {
          cb(RegisterCollection(err, category, regs));
        });
  }

  joiner->Ready([weak_this = weak_factory_.GetWeakPtr(),
                 cb = std::move(cb)](const std::vector<RegisterCollection>& results) mutable {
    for (auto result : results) {
      if (result.err.has_error()) {
        return cb(result.err);
      }

      // Remove non-writeable registers from the register set.
      std::erase_if(result.registers, [](const debug::RegisterValue& rv) {
        return rv.id == debug::RegisterID::kARMv8_tpidr;
      });

      weak_this->SetRegisterCategory(result.category, result.registers);

      if (cb)
        cb(Err());
    }
  });
}

Frame* CallFunctionThreadControllerArm64::PushStackFrame(
    uint64_t new_pc, uint64_t old_sp, const std::vector<debug::RegisterValue>& regs) {
  // Arm64 doesn't have a red zone requirement, so the sp stays the same until
  // we jump to the callee which will update the stack pointer itself.
  debug_ipc::StackFrame synthesized;
  synthesized.sp = old_sp;   // the new stack pointer.
  synthesized.cfa = old_sp;  // for the frame fingerprint.
  synthesized.ip = new_pc;   // this is the start of the function we're about to call.
  synthesized.regs = regs;

  auto& stack = thread()->GetStack();

  const Frame* original_frame = stack[0];
  debug_ipc::StackFrame original;
  original.sp = original_frame->GetStackPointer();
  original.cfa = original_frame->GetCanonicalFrameAddress();
  original.ip = original_frame->GetAddress();
  original.regs = *original_frame->GetRegisterCategorySync(debug::RegisterCategory::kGeneral);

  stack.SetFrames(debug_ipc::ThreadRecord::StackAmount::kMinimal, {synthesized, original});

  return stack[0];
}

}  // namespace zxdb
