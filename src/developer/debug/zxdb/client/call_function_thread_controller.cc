// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/call_function_thread_controller.h"

#include <map>
#include <utility>
#include <vector>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/register_info.h"
#include "src/developer/debug/shared/register_value.h"
#include "src/developer/debug/zxdb/client/call_function_thread_controller_arm64.h"
#include "src/developer/debug/zxdb/client/call_function_thread_controller_x64.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/step_thread_controller.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/client/thread_controller.h"
#include "src/developer/debug/zxdb/expr/return_value.h"
#include "src/developer/debug/zxdb/symbols/base_type.h"
#include "src/developer/debug/zxdb/symbols/function.h"

namespace zxdb {

CallFunctionThreadController::CallFunctionThreadController(const AddressRanges& ranges,
                                                           const std::vector<ExprValue>& parameters,
                                                           EvalCallback on_function_completed,
                                                           fit::deferred_callback on_done)
    : ThreadController(std::move(on_done)),
      address_ranges_(ranges),
      parameters_(parameters),
      on_function_completed_(std::move(on_function_completed)),
      weak_factory_(this) {}

CallFunctionThreadController::~CallFunctionThreadController() {
  if (on_function_completed_) {
    on_function_completed_(
        Err("CallFunctionThreadController destroyed before function call was completed."));
  }
}

// static.
bool CallFunctionThreadController::WriteRegister(std::vector<debug::RegisterValue>& regs,
                                                 debug::RegisterID id, uint64_t value) {
  auto reg_it = std::find_if(regs.begin(), regs.end(),
                             [id](const debug::RegisterValue& rv) { return rv.id == id; });
  if (reg_it == regs.end())
    return false;

  *reg_it = debug::RegisterValue(id, value);
  return true;
}

// static.
uint64_t CallFunctionThreadController::GetRegisterData(
    const std::vector<debug::RegisterValue>& regs, debug::RegisterID id) {
  auto reg_it = std::find_if(regs.begin(), regs.end(),
                             [id](const debug::RegisterValue& rv) { return rv.id == id; });
  if (reg_it == regs.end())
    return 0;

  return reg_it->GetValue();
}

Err CallFunctionThreadController::WriteParametersToRegisters() {
  auto arg_regs_opt = thread()->session()->arch_info().abi()->GetFunctionParameterRegisters();

  if (!arg_regs_opt) {
    return Err("Unknown ");
  }

  auto argument_registers = *arg_regs_opt;
  if (parameters_.size() > argument_registers.size()) {
    return Err("Only %zu arguments are supported functions on %s, but was given %zu",
               argument_registers.size(), thread()->session()->arch_info().triple_name().c_str(),
               parameters_.size());
  }

  for (size_t i = 0; i < parameters_.size(); i++) {
    uint64_t promoted = 0;

    if (parameters_[i].data().size() > sizeof(promoted)) {
      return Err("Parameter %zu cannot fit in target register. Size is: %zu", i,
                 parameters_[i].data().size());
    }

    if (auto err = parameters_[i].PromoteTo64(&promoted); err.has_error()) {
      return err;
    }

    if (!WriteRegister(general_registers_, argument_registers[i], promoted)) {
      return Err("Failed writing register %s", debug::RegisterIDToString(argument_registers[i]));
    }
  }

  return Err();
}

void CallFunctionThreadController::WriteGeneralRegisters(fit::callback<void(const Err&)> cb) {
  debug_ipc::WriteRegistersRequest request;
  request.id = {thread()->GetProcess()->GetKoid(), thread()->GetKoid()};
  request.registers = general_registers_;

  thread()->GetProcess()->session()->remote_api()->WriteRegisters(
      request,
      [weak_this = weak_factory_.GetWeakPtr(), weak_thread = thread()->GetWeakPtr(),
       cb = std::move(cb)](const Err& err, const debug_ipc::WriteRegistersReply& reply) mutable {
        if (err.has_error()) {
          return cb(err);
        } else if (reply.status.has_error()) {
          return cb(Err(reply.status.message()));
        } else if (!weak_thread) {
          return cb(Err("Thread disappeared."));
        } else if (!weak_this) {
          return cb(Err("CallFunctionThreadController disappeared."));
        }

        weak_this->Log("Spawning finish thread controller to complete synthetic frame.");

        auto& stack = weak_thread->GetStack();
        weak_this->finish_controller_ = std::make_unique<FinishThreadController>(
            stack, *stack.IndexForFrame(stack[0]), [weak_this](const FunctionReturnInfo& info) {
              if (weak_this)
                weak_this->return_info_ = info;
            });

        weak_this->finish_controller_->InitWithThread(weak_thread.get(), std::move(cb));
      });
}

void CallFunctionThreadController::ResolveReturnValue(const FunctionReturnInfo& return_info,
                                                      EvalCallback cb) {
  // If FunctionReturnCallback was never called or gave us bad info then bail early since we can
  // never get the return value.
  if (!return_info.thread || !return_info.symbol) {
    return cb(Err("Frame exited with invalid return info"));
  }

  auto eval_context = return_info.thread->GetStack()[0]->GetEvalContext();
  const Function* fn = return_info.symbol.Get()->As<Function>();

  if (!fn) {
    return cb(Err("Function symbol disappeared"));
  }

  GetReturnValue(eval_context, fn,
                 [weak_this = weak_factory_.GetWeakPtr(),
                  cb = std::move(cb)](ErrOrValue err_or_value) mutable {
                   if (!weak_this) {
                     return;
                   }

                   ExprValue value;
                   if (err_or_value.ok()) {
                     if (!err_or_value.value_or_empty().type()) {
                       // Void return.
                       auto voidtype =
                           fxl::MakeRefCounted<BaseType>(BaseType::kBaseTypeNone, 0, "");
                       value = ExprValue(std::move(voidtype), {});
                     } else {
                       value = err_or_value.take_value();
                     }
                   } else {
                     return cb(err_or_value.err());
                   }

                   return cb(std::move(value));
                 });
}

void CallFunctionThreadController::CleanupFunction(fit::callback<void(const Err&)> cb) {
  Log("CallFunctionThreadController::CleanupFunction");

  debug_ipc::WriteRegistersRequest request;
  request.id = {.process = thread()->GetProcess()->GetKoid(), .thread = thread()->GetKoid()};

  for (auto [cat, regs] : saved_register_state_) {
    request.registers.insert(request.registers.end(), regs.begin(), regs.end());
  }

  thread()->session()->remote_api()->WriteRegisters(
      request, [weak_this = weak_factory_.GetWeakPtr(), cb = std::move(cb)](
                   const Err& err, const debug_ipc::WriteRegistersReply& reply) mutable {
        if (!weak_this) {
          return cb(Err("CallFunctionThreadController disappeared."));
        } else if (err.has_error()) {
          weak_this->saved_register_state_.clear();
          return cb(Err("Got error while restoring registers: " + err.msg()));
        } else if (reply.status.has_error()) {
          weak_this->saved_register_state_.clear();
          return cb(Err("Got WriteRegistersReply status error: " + reply.status.message()));
        }

        // Force sync the registers to the top most frame, which should be the
        // same as what we just restored. This ensures that if the user types
        // "regs" after calling the function they should be identical to what
        // they were before. Once this completes, we're done.
        weak_this->thread()->GetStack()[0]->GetRegisterCategoryAsync(
            debug::RegisterCategory::kGeneral, true,
            [weak_this, cb = std::move(cb)](const Err& err,
                                            const std::vector<debug::RegisterValue>& regs) mutable {
              if (cb)
                return cb(err);
            });
      });
}

ThreadController::ContinueOp CallFunctionThreadController::GetContinueOp() {
  return finish_controller_->GetContinueOp();
}

ThreadController::StopOp CallFunctionThreadController::OnThreadStop(
    debug_ipc::ExceptionType stop_type,
    const std::vector<fxl::WeakPtr<Breakpoint>>& hit_breakpoints) {
  Log("CallFunctionThreadController::OnThreadStop");

  if (finish_controller_) {
    auto op = finish_controller_->OnThreadStop(stop_type, hit_breakpoints);

    if (op == kStopDone) {
      // Okay the finish controller is done, now we need to cleanup the
      // register state which will happen asynchronously. We should have already
      // captured the return info from the FunctionReturnCallback provided to
      // this controller, so now all we need to do is cleanup.
      finish_controller_.reset();

      // When the finish controller is finished, the synthetic frame that was
      // created by the ABI instance of this class has been completed.
      // First collect the return value, then cleanup the register state and
      // report that we're finished.
      ResolveReturnValue(return_info_, [stop_type, weak_this = weak_factory_.GetWeakPtr()](
                                           ErrOrValue err_or_value) mutable {
        if (weak_this) {
          weak_this->CleanupFunction([err_or_value, stop_type, weak_this](const Err& err) mutable {
            if (!weak_this)
              return;

            // An error restoring the thread state is more important to
            // surface to the user than anything to do with the return
            // value since this would leave the debugger in a weird state.
            if (err.has_error())
              err_or_value = err;

            weak_this->on_function_completed_(std::move(err_or_value));

            weak_this->thread()->ResumeFromAsyncThreadController(stop_type);
          });
        }
      });

      return kFuture;
    }

    // The finish controller is not finished yet forward that back up to the
    // thread.
    return op;
  }

  // We are only considered done once we've reported back the return value and
  // have completed restoring the thread state.
  return on_function_completed_ ? kFuture : kStopDone;
}

}  // namespace zxdb
