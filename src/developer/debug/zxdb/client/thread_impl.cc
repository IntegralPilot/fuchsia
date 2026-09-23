// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/thread_impl.h"

#include <inttypes.h>
#include <lib/syslog/cpp/macros.h>

#include <iostream>
#include <limits>

#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/ipc/unwinder_support.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/register_info.h"
#include "src/developer/debug/shared/zx_status.h"
#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_observer.h"
#include "src/developer/debug/zxdb/client/elf_memory_region.h"
#include "src/developer/debug/zxdb/client/frame_impl.h"
#include "src/developer/debug/zxdb/client/process_impl.h"
#include "src/developer/debug/zxdb/client/remote_api.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"
#include "src/developer/debug/zxdb/client/target_impl.h"
#include "src/developer/debug/zxdb/client/thread_controller.h"
#include "src/developer/debug/zxdb/common/join_callbacks.h"
#include "src/developer/debug/zxdb/expr/cast.h"
#include "src/developer/debug/zxdb/expr/expr.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/lib/unwinder/unwind.h"

namespace zxdb {

namespace {

// Maximum number of times we'll allow thread controller to return "kFuture" before we declare
// they're in an infinite loop. Normally there will only be one "future" return before calling
// ResumeFromAsyncThreadController() so if we get many it probably indicates a thread controller is
// continuing to return the "future" from the same state and it's stuck (this is an easy mistake).
constexpr int kMaxNestedFutureCompletion = 16;

}  // namespace

ThreadImpl::ThreadImpl(ProcessImpl* process, const debug_ipc::ThreadRecord& record)
    : Thread(process->session()),
      process_(process),
      koid_(record.id.thread),
      stack_(this),
      async_task_tree_(this),
      weak_factory_(this) {
  SetMetadata(record);
  settings_.set_fallback(&process_->target()->settings());

  // Should be at the bottom of this function. This will enable notifications now that
  // initialization is complete.
  allow_notifications_ = true;
}

ThreadImpl::~ThreadImpl() = default;

Process* ThreadImpl::GetProcess() const { return process_; }

uint64_t ThreadImpl::GetKoid() const { return koid_; }

const std::string& ThreadImpl::GetName() const { return name_; }

std::optional<debug_ipc::ThreadRecord::State> ThreadImpl::GetState() const { return state_; }
debug_ipc::ThreadRecord::BlockedReason ThreadImpl::GetBlockedReason() const {
  return blocked_reason_;
}
std::optional<StopInfo> ThreadImpl::CurrentStopInfo() const { return current_stop_info_; }

void ThreadImpl::Pause(fit::callback<void()> on_paused) {
  // The frames may have been requested when the thread was running which will have marked them
  // "empty but complete." When a pause happens the frames will become available so we want
  // subsequent requests to request them.
  ClearState();

  debug_ipc::PauseRequest request;
  request.ids.push_back({.process = process_->GetKoid(), .thread = koid_});

  session()->remote_api()->Pause(
      request, [weak_thread = weak_factory_.GetWeakPtr(), on_paused = std::move(on_paused)](
                   const Err& err, debug_ipc::PauseReply reply) mutable {
        if (!err.has_error() && weak_thread) {
          // Save the new metadata.
          if (reply.threads.size() == 1 && reply.threads[0].id.thread == weak_thread->koid_) {
            weak_thread->SetMetadata(reply.threads[0]);
          } else {
            // If the client thread still exists, the agent's record of that thread should have
            // existed at the time the message was sent so there should be no reason the update
            // doesn't match.
            FX_NOTREACHED();
          }
        }
        on_paused();
      });
}

void ThreadImpl::Continue(bool forward_exception) {
  debug_ipc::ResumeRequest request;
  request.ids.push_back({.process = process_->GetKoid(), .thread = koid_});

  if (controllers_.empty()) {
    if (!forward_exception && current_stop_info_) {
      if (current_stop_info_->exception_type != debug_ipc::ExceptionType::kNone &&
          !debug_ipc::IsDebug(current_stop_info_->exception_type)) {
        // Debug exceptions are excluded since we never want to forward those.
        forward_exception = true;
      }
    }

    request.how = forward_exception ? debug_ipc::ResumeRequest::How::kForwardAndContinue
                                    : debug_ipc::ResumeRequest::How::kResolveAndContinue;
  } else {
    // When there are thread controllers, ask the most recent one for how to continue.
    //
    // Theoretically we're running with all controllers at once and we want to stop at the first one
    // that triggers, which means we want to compute the most restrictive intersection of all of
    // them.
    //
    // This is annoying to implement and it's difficult to construct a situation where this would be
    // required. The controller that doesn't involve breakpoints is "step in range" and generally
    // ranges refer to code lines that will align. Things like "until" are implemented with
    // breakpoints so can overlap arbitrarily with other operations with no problem.
    //
    // A case where this might show up:
    //  1. Do "step into" which steps through a range of instructions.
    //  2. In the middle of that range is a breakpoint that's hit.
    //  3. The user does "finish." We'll ask the finish controller what to do and it will say
    //     "continue" and the range from step 1 is lost.
    // However, in this case probably does want to end up one stack frame back rather than several
    // instructions after the breakpoint due to the original "step into" command, so even when
    // "wrong" this current behavior isn't necessarily bad.
    controllers_.back()->Log("Continuing with this controller as primary.");
    ThreadController::ContinueOp op = controllers_.back()->GetContinueOp();
    if (op.synthetic_stop_) {
      // Synthetic stop. Skip notifying the backend and broadcast a stop notification for the
      // current state.
      controllers_.back()->Log("Synthetic stop.");
      debug::MessageLoop::Current()->PostTask(FROM_HERE, [thread = weak_factory_.GetWeakPtr()]() {
        if (thread) {
          StopInfo info;
          info.exception_type = debug_ipc::ExceptionType::kSynthetic;
          thread->OnException(info);
        }
      });
      return;
    } else {
      // Dispatch the continuation message.
      request.how = op.how;
      request.range_begin = op.range.begin();
      request.range_end = op.range.end();
    }
  }

  ClearState();
  session()->remote_api()->Resume(request, [](const Err& err, debug_ipc::ResumeReply) {});
}

void ThreadImpl::AddController(std::unique_ptr<ThreadController> controller,
                               fit::callback<void(const Err&)> on_done,
                               AddControllerOptions options) {
  ThreadController* controller_ptr = controller.get();

  // Add it first so that its presence will be noted by anything its initialization function does.
  controllers_.push_back(std::move(controller));

  controller_ptr->InitWithThread(
      this, [this, controller_ptr, on_done = std::move(on_done), options](const Err& err) mutable {
        if (err.has_error()) {
          controller_ptr->Log("InitWithThread failed: %s", err.msg().c_str());
          NotifyControllerDone(controller_ptr);  // Remove the controller.
        } else {
          controller_ptr->Log("Initialized.");
          if (options.continue_) {
            this->Continue(false);
          }
        }
        if (on_done)
          on_done(err);
      });
}

void ThreadImpl::ContinueWith(std::unique_ptr<ThreadController> controller,
                              fit::callback<void(const Err&)> on_continue) {
  AddController(std::move(controller), std::move(on_continue), AddControllerOptions(true));
}

void ThreadImpl::AddPostStopTask(PostStopTask task) {
  // This function must only be called from a ThreadController::OnThreadStop() handler.
  FX_DCHECK(handling_on_stop_);
  post_stop_tasks_.push_back(std::move(task));
}

void ThreadImpl::CancelAllThreadControllers() {
  controllers_.clear();
  if (nested_stop_future_completion_) {
    // We're waiting on an async thread controller to complete but just cleared them all. Reissue
    // the exception to clean up the async state and issue stop notifications.
    OnException(async_stop_info_);
  }
}

void ThreadImpl::ResumeFromAsyncThreadController(std::optional<debug_ipc::ExceptionType> type) {
  bool debug_stepping = settings().GetBool(ClientSettings::Thread::kDebugStepping);
  if (debug_stepping)
    printf("↓↓↓↓↓↓↓↓↓↓ Resuming from async thread controller.\r\n");

  if (nested_stop_future_completion_ == 0) {
    // Not waiting on an async thread controller to finish. This could be a programming error but it
    // could also be that somebody called CancelAllThreadControllers() out from under us.
    if (debug_stepping)
      printf("No async stepping in progress, giving up.\r\n");
    return;
  }

  if (type)
    async_stop_info_.exception_type = *type;

  OnException(async_stop_info_);
}

void ThreadImpl::JumpTo(uint64_t new_address, fit::callback<void(const Err&)> cb) {
  // The register to set.
  debug_ipc::WriteRegistersRequest request;
  request.id = {.process = process_->GetKoid(), .thread = koid_};
  request.registers.emplace_back(
      GetSpecialRegisterID(session()->arch(), debug::SpecialRegisterType::kIP), new_address);

  // The "jump" command updates the thread's location so we need to recompute the stack. So once the
  // jump is complete we re-request the thread's status.
  //
  // This could be made faster by requesting status immediately after sending the update so we don't
  // have to wait for two round-trips, but that complicates the callback logic and this feature is
  // not performance- sensitive.
  //
  // Another approach is to make the register request message able to optionally request a stack
  // backtrace and include that in the reply.
  session()->remote_api()->WriteRegisters(
      request, [thread = weak_factory_.GetWeakPtr(), cb = std::move(cb)](
                   const Err& err, debug_ipc::WriteRegistersReply reply) mutable {
        if (err.has_error()) {
          cb(err);  // Transport error.
        } else if (reply.status.has_error()) {
          cb(Err("Could not set thread instruction pointer: " + reply.status.message()));
        } else if (!thread) {
          cb(Err("Thread destroyed."));
        } else {
          // Success, update the current stack before issuing the callback.
          thread->SyncFramesForStack({}, std::move(cb));
        }
      });
}

void ThreadImpl::NotifyControllerDone(ThreadController* controller) {
  controller->Log("Controller done, removing.");

  // We expect to have few controllers so brute-force is sufficient.
  for (auto cur = controllers_.begin(); cur != controllers_.end(); ++cur) {
    if (cur->get() == controller) {
      controllers_.erase(cur);
      return;
    }
  }
  FX_NOTREACHED();  // Notification for unknown controller.
}

void ThreadImpl::StepInstructions(uint64_t count) {
  debug_ipc::ResumeRequest request;
  request.ids.push_back({.process = process_->GetKoid(), .thread = koid_});
  request.how = debug_ipc::ResumeRequest::How::kStepInstruction;
  request.count = count;
  session()->remote_api()->Resume(request, [](const Err& err, debug_ipc::ResumeReply) {});
}

const Stack& ThreadImpl::GetStack() const { return stack_; }

Stack& ThreadImpl::GetStack() { return stack_; }

const AsyncTaskTree& ThreadImpl::GetAsyncTaskTree() const { return async_task_tree_; }

AsyncTaskTree& ThreadImpl::GetAsyncTaskTree() { return async_task_tree_; }

void ThreadImpl::SetMetadata(const debug_ipc::ThreadRecord& record, bool skip_frames) {
  FX_DCHECK(koid_ == record.id.thread);

  name_ = record.name;
  state_ = record.state;
  blocked_reason_ = record.blocked_reason;

  if (!skip_frames) {
    stack_.SetFrames(record.stack_amount, record.frames);
  }
}

void ThreadImpl::OnException(const StopInfo& info) {
  if (settings().GetBool(ClientSettings::Thread::kDebugStepping)) {
    printf("----------\r\nGot %s exception @ 0x%" PRIx64 " in %s\r\n",
           debug_ipc::ExceptionTypeToString(info.exception_type), stack_[0]->GetAddress(),
           ThreadController::FrameFunctionNameForLog(stack_[0]).c_str());
  }

  if (stack_.empty()) {
    // Threads can stop with no stack if the thread is killed while processing an exception. If
    // this happens (or any other error that might cause an empty stack), declare all thread
    // controllers done since they can't meaningfully continue or process this state, and forcing
    // them all to separately check for an empty stack is error-prone.
    controllers_.clear();
  }

  // Debug tracking for proper usage from OnThreadStop handlers.
  handling_on_stop_ = true;

  // When any controller says "stop" it takes precedence and the thread will stop no matter what
  // any other controllers say.
  bool should_stop = false;

  // Set when any controller says "continue". If no controller says "stop" we need to differentiate
  // the case where there are no controllers or all controllers say "unexpected" (thread should
  // stop), from where one or more said "continue" (thread should continue, any "unexpected" votes
  // are ignored).
  bool have_continue = false;

  auto controller_iter = controllers_.begin();
  while (controller_iter != controllers_.end()) {
    ThreadController* controller = controller_iter->get();
    switch (controller->OnThreadStop(info.exception_type, info.hit_breakpoints)) {
      case ThreadController::kContinue:
        // Try the next controller.
        controller->Log("Reported continue on exception.");
        have_continue = true;
        controller_iter++;
        break;
      case ThreadController::kStopDone:
        // Once a controller tells us to stop, we assume the controller no longer applies and delete
        // it.
        //
        // Need to continue with checking all controllers even though we know we should stop at this
        // point. Multiple controllers should say "stop" at the same time and we need to be able to
        // delete all that no longer apply (say you did "finish", hit a breakpoint, and then
        // "finish" again, both finish commands would be active and you would want them both to be
        // completed when the current frame actually finishes).
        controller->Log("Reported stop on exception, stopping and removing it.");
        controller_iter = controllers_.erase(controller_iter);
        should_stop = true;
        break;
      case ThreadController::kUnexpected:
        // An unexpected exception means the controller is still active but doesn't know what to do
        // with this exception.
        controller->Log("Reported unexpected exception.");
        controller_iter++;
        break;
      case ThreadController::kFuture:
        controller->Log("Returned kFuture, waiting for async completion.");

        nested_stop_future_completion_++;
        if (nested_stop_future_completion_ >= kMaxNestedFutureCompletion) {
          // The thread controllers issued too many sequential "future" stop completions. It's easy
          // to accidentally get into an infinite loop by continuing to return kFuture from the same
          // stop type. This code detects that case and gives up.
          controller->Log(
              "Hit limit for nested 'future' thread controllers. Clearing state and stopping.");
          controllers_.clear();
          should_stop = true;
        } else {
          // Normal good case. Don't do anything and wait for the controller to call
          // ResumeFromAsyncThreadController() to continue.
          //
          // In this case we keep handling_on_stop_ true because we can continue to accumulate
          // post-stop tasks.
          async_stop_info_ = info;
          return;
        }
    }
  }

  handling_on_stop_ = false;
  nested_stop_future_completion_ = 0;

  if (!have_continue && !controllers_.empty()) {
    // No controller voted to continue (maybe all active controllers reported "unexpected"). Such
    // cases should stop.
    should_stop = true;
  }

  // This joiner is responsible for collecting all of the conditional breakpoint evaluation results
  // at this location. Only a single conditional breakpoint needs to evaluate to true to trigger a
  // stop. Unconditional breakpoints will always stop, and if there are only unconditional
  // breakpoints at this location, then this evaluation will happen synchronously.
  auto conditional_breakpoints_callback = fxl::MakeRefCounted<JoinCallbacks<bool>>();

  StopInfo external_info = info;

  // The existence of any non-internal, unconditional breakpoints being hit means the thread should
  // always stop. Breakpoints that have conditional expressions will be evaluated and stop only if
  // the condition is true. This check happens after notifying the controllers so if a controller
  // triggers, it's counted as a "hit" (otherwise, doing "run until" to a line with a normal
  // breakpoint on it would keep the "run until" operation active even after it was hit).
  //
  // Also, filter out internal breakpoints in the notification sent to the observers.
  for (auto it = external_info.hit_breakpoints.begin(); it != external_info.hit_breakpoints.end();
       /* nothing */) {
    if (*it && !it->get()->IsInternal()) {
      auto bp = it->get();

      if (const auto& cond = bp->GetSettings().condition; !cond.empty()) {
        ResolveConditionalBreakpoint(cond, bp, conditional_breakpoints_callback->AddCallback());
      } else {
        // Non-internal, unconditional breakpoints should always stop. Conditional breakpoints will
        // evaluate their expressions (there could be multiple, but only one has to vote to stop)
        // and then decide to stop or not.
        should_stop = true;
      }
      ++it;
    } else {
      // Remove deleted weak pointers and internal breakpoints.
      it = external_info.hit_breakpoints.erase(it);
    }
  }

  // If there are any conditional breakpoints that need to evaluate an expression, the callback(s)
  // will be issued asynchronously and collected into a vector. In the case no asynchronous
  // evaluations need to occur, this will return immediately and issue the callback above.
  conditional_breakpoints_callback->Ready([weak_this = weak_factory_.GetWeakPtr(), should_stop,
                                           external_info](std::vector<bool> results) mutable {
    if (weak_this) {
      bool should_forward = false;

      // This is the case where there's some external exception from the remote thread. We want to
      // ask our owning process what to do too in this case, which may call |Continue| with a
      // different forwarding argument.
      switch (weak_this->process()->OnException(external_info)) {
        case debug_ipc::ResumeRequest::How::kForwardAndContinue: {
          should_forward = true;
          should_stop = false;
          break;
        }
        case debug_ipc::ResumeRequest::How::kResolveAndContinue: {
          should_forward = false;
          should_stop = false;
          break;
        }
        case debug_ipc::ResumeRequest::How::kLast: {
          // The process has no opinion about how to continue, the following checks will determine
          // if we actually stop or not.
          //
          // Non-debug exceptions (most likely a crash is happening) also mean the thread should
          // always stop (check this after running the controllers for the same reason as the
          // breakpoint check above).
          if (external_info.exception_type != debug_ipc::ExceptionType::kNone &&
              !debug_ipc::IsDebug(external_info.exception_type)) {
            should_stop = true;
          }

          if (std::any_of(results.cbegin(), results.cend(), [](bool result) { return result; }))
            should_stop = true;

          // If there are no conditional breakpoints installed here, and there are no running
          // controllers, then we should stop. This case catches things like __builtin_debugtrap()
          // or "int 3" on x86_64 architectures.
          if (!should_stop && results.empty() && weak_this->controllers_.empty()) {
            should_stop = true;
          }
          break;
        }
        default: {
          FX_NOTREACHED();
          break;
        }
      }

      // Execute the chain of post-stop tasks (may be asynchronous) and then dispatch the stop
      // notification or continue operation.
      weak_this->RunNextPostStopTaskOrNotify(external_info, should_stop, should_forward);
    }
  });
}

void ThreadImpl::ResolveConditionalBreakpoint(const std::string& cond, Breakpoint* bp,
                                              fit::callback<void(bool)> cb) {
  // Update the evaluation context to the current stack frame and schedule the expression
  // evaluation.
  auto ctx = GetStack()[0]->GetEvalContext();
  EvalExpression(cond, ctx, true, [this, ctx, bp, cb = std::move(cb)](ErrOrValue val) mutable {
    std::string_view msg;

    if (val.ok()) {
      if (auto cast_result = CastNumericExprValueToBool(ctx, val.value()); cast_result.ok()) {
        // This is the normal good case. The expression resolved to a value that successfully
        // casted to a boolean. We're done.
        cb(cast_result.value());
        return;
      } else {
        // The expression evaluated successfully, but couldn't be cast to a bool.
        msg = cast_result.err().msg();
      }
    } else {
      // The expression couldn't be evaluated.
      msg = val.err().msg();
    }

    // If we get here, the conditional expression failed to evaluate somehow. Show a warning
    // message if the expression resolution fails for some reason. Note: we still want to stop
    // execution here so the user can fix whatever went wrong.
    Err err(
        "Hit conditional breakpoint, but expression evaluation failed:\n%s\nThis location "
        "could have been optimized out, or this variable might not exist in the current scope."
        "\nCheck the expression with `bp <id> get condition`, or modify the expression with "
        "`bp <id> set condition <expr>`.",
        msg.data());

    for (auto& observer : session()->breakpoint_observers()) {
      observer.OnBreakpointUpdateFailure(bp, err);
    }

    cb(true);
  });
}

void ThreadImpl::SyncFramesForStack(const Stack::SyncFrameOptions& options,
                                    fit::callback<void(const Err&)> callback) {
  if (options.remote_unwind) {
    return SyncFramesFromTarget(std::move(callback));
  }

  debug_ipc::ReadRegistersRequest request;
  request.categories = {debug::RegisterCategory::kGeneral};
  request.id = {.process = GetProcess()->GetKoid(), .thread = GetKoid()};

  // Request the registers that will make up the top-most stack frame. The unwinder needs these
  // for all unwinding strategies so fetching them is a prerequisite. We'll try to use local
  // debug_info before deferring to the live process for unwinding.
  session()->remote_api()->ReadRegisters(
      request, [weak_this = weak_factory_.GetWeakPtr(), cb = std::move(callback)](
                   const Err& err, debug_ipc::ReadRegistersReply reply) mutable {
        if (!weak_this) {
          return cb(Err("Thread died while fetching registers"));
        }

        if (err.has_error() || reply.registers.empty()) {
          // This commonly happens for unit tests which historically expect the frames to only be
          // available remotely and/or don't necessarily provide registers.
          return weak_this->SyncFramesFromTarget(std::move(cb));
        }

        if (weak_this->GetProcess()->GetSymbolStatus() == Process::SymbolStatus::kInProgress) {
          // The unwinder requires debuginfo to be present to unwind on the host. If there is a
          // download in progress we have to wait until the download is finished before
          // continuing.
          return weak_this->session()->system().AddPostDownloadTask(
              [weak_this, regs = reply.registers, cb = std::move(cb)]() mutable {
                if (!weak_this) {
                  return cb(Err("Thread disappeared while waiting for downloads!"));
                }

                weak_this->UnwindWithRegisters(
                    debug_ipc::ConvertRegisters(weak_this->session()->arch(), regs), std::move(cb));
              });
        }

        weak_this->UnwindWithRegisters(
            debug_ipc::ConvertRegisters(weak_this->session()->arch(), reply.registers),
            std::move(cb));
      });
}

void ThreadImpl::UnwindWithRegisters(unwinder::Registers regs, fit::callback<void(const Err&)> cb) {
  std::vector<debug_ipc::StackFrame> frames;

  auto loaded_modules = GetProcess()->GetSymbols()->GetLoadedModuleSymbols();

  std::vector<unwinder::Module> modules;
  modules.reserve(loaded_modules.size());
  std::vector<std::unique_ptr<unwinder::Memory>> unwinder_memory;
  unwinder_memory.reserve(loaded_modules.size());

  for (const auto& loaded_module : loaded_modules) {
    auto build_id_entry = session()->system().GetSymbols()->build_id_index().EntryForBuildID(
        loaded_module->build_id());

    // It's unlikely, but possible that the (likely stripped) binary file has some debug sections
    // present, so we should use the ElfMemoryRegion object which knows how to decompress debug
    // sections when that's enabled.
    std::unique_ptr<ElfMemoryRegion> binary_file_memory = nullptr;
    std::unique_ptr<ElfMemoryRegion> debug_info_file_memory = nullptr;
    if (!build_id_entry.binary.empty()) {
      binary_file_memory =
          std::make_unique<ElfMemoryRegion>(loaded_module->load_address(), build_id_entry.binary);
    }

    if (!build_id_entry.debug_info.empty()) {
      debug_info_file_memory = std::make_unique<ElfMemoryRegion>(loaded_module->load_address(),
                                                                 build_id_entry.debug_info);
    }

    modules.emplace_back(loaded_module->load_address(), binary_file_memory.get(),
                         debug_info_file_memory.get(), unwinder::Module::AddressMode::kFile);

    unwinder_memory.push_back(std::move(binary_file_memory));
    unwinder_memory.push_back(std::move(debug_info_file_memory));
  }

  // This probably doesn't need to be configurable, when someone types "bt" or "frame", they
  // want to see the whole stack, and the PrettyStackManager already does a good job of
  // removing the less helpful frames. For right now this matches what DebugAgent sends to
  // the unwinder for remote unwinds.
  constexpr size_t kMaxDepth = 256;

  // Here we try unwinding using local debug info before deferring to the target. This lets us use
  // potentially better .debug_frame section that is not loaded in the target executable, if
  // present, or deal with cases where the .eh_frame section has been stripped from the target but
  // remains in an unstripped binary that we have access to on the host.
  auto unwinder = std::make_unique<unwinder::AsyncUnwinder>(GetProcess(), modules);
  unwinder->Unwind(
      regs, kMaxDepth,
      // Move the unwinder itself and the memory into the callback so they stay alive for the
      // duration of unwinding.
      // In the typical case the reference counts for the unwinder drops to zero once unwinding
      // completes and the `fit::callback` is invoked, since `fit::callback`'s destructor is
      // automatically called after it's invoked once.
      //
      // TODO(https://fxbug.dev/454693056): Investigate the following:
      // In a rare case that the process is killed while in the process of unwinding, this
      // callback might not be invoked -- which we're not certain of, due to the handling of
      // `!weak_this` below -- leading to a memory leak since `AsyncUnwinder::on_done_` and
      // `AsyncUnwinder::this` will now hold cyclic references to each other. However, even if
      // were was the case, this actually isn't such a huge deal since:
      //  a. This scenario is extremely unlikely.
      //  b. Even if this were to happen, developers typically debug one process per zxdb session,
      //     causing zxdb to exit after the process dies.
      //  c. Even if this were not the case, this memory leak would not cost a significant amount
      //  of
      //     memory on the developer's host machine.
      [weak_this = weak_factory_.GetWeakPtr(), cb = std::move(cb), unwinder = std::move(unwinder),
       unwinder_memory =
           std::move(unwinder_memory)](std::vector<unwinder::Frame> unwinder_frames) mutable {
        // The unwinder's asynchronous API calls all callbacks reentrantly, so post a
        // task to the message loop to make sure we don't blow our own stack.
        debug::MessageLoop::Current()->PostTask(
            FROM_HERE, [weak_this, unwinder_frames = std::move(unwinder_frames),
                        cb = std::move(cb)]() mutable {
              if (!weak_this) {
                if (cb) {
                  cb(Err("Thread died during unwinding."));
                }

                return;
              }

              // If something went wrong, the last frame will have an error condition.
              // In the typical unwinding termination case, the error is cleared.
              if (unwinder_frames.back().error.has_err()) {
                weak_this->SyncFramesFromTarget(std::move(cb));
                return;
              }

              weak_this->stack_.SetFrames(debug_ipc::ThreadRecord::StackAmount::kFull,
                                          debug_ipc::ConvertFrames(unwinder_frames));

              if (cb) {
                cb(Err());
              }
            });
      });
}

void ThreadImpl::SyncFramesFromTarget(fit::callback<void(const Err&)> callback) {
  debug_ipc::ThreadStatusRequest request;
  request.id = {.process = process_->GetKoid(), .thread = koid_};

  session()->remote_api()->ThreadStatus(
      request, [callback = std::move(callback), thread = weak_factory_.GetWeakPtr()](
                   const Err& err, debug_ipc::ThreadStatusReply reply) mutable {
        if (!thread) {
          callback(Err("Thread destroyed."));
          return;
        }

        if (err.has_error()) {
          callback(err);
          return;
        }

        thread->SetMetadata(reply.record);
        callback(Err());
      });
}

std::unique_ptr<Frame> ThreadImpl::MakeFrameForStack(const debug_ipc::StackFrame& input,
                                                     Location location) {
  return std::make_unique<FrameImpl>(this, input, std::move(location));
}

Location ThreadImpl::GetSymbolizedLocationForAddress(uint64_t address) {
  auto vect = GetProcess()->GetSymbols()->ResolveInputLocation(InputLocation(address), {});

  // Symbolizing an address should always give exactly one result.
  FX_DCHECK(vect.size() == 1u);
  return vect[0];
}

void ThreadImpl::DidUpdateStackFrames() {
  if (allow_notifications_) {
    for (auto& observer : session()->thread_observers()) {
      observer.DidUpdateStackFrames(this);
    }
  }
}

void ThreadImpl::SyncAsyncTasks(fit::callback<void(const Err&, const Frame* frame)> callback) {
  if (stack_.empty()) {
    return callback(Err("No stack frames available to fetch async task tree."), nullptr);
  }

  // Look for a frame that can handle the async task tree.
  for (size_t i = 0; i < stack_.size(); ++i) {
    auto frame = stack_[i];
    auto providers =
        process()->GetAsyncTaskProvidersForLanguage(frame->GetEvalContext()->GetLanguage());
    for (auto* provider : providers) {
      if (provider->CanHandle(frame)) {
        provider->GetTasks(
            frame, [tree_ptr = async_task_tree_.GetWeakPtr(), weak_frame = frame->GetWeakPtr(),
                    cb = std::move(callback)](
                       const Err& err, std::vector<std::unique_ptr<AsyncTask>> tasks) mutable {
              if (err.has_error()) {
                cb(err, nullptr);
                return;
              } else if (!tree_ptr) {
                cb(Err("AsyncTaskTree destroyed while fetching tasks."), nullptr);
                return;
              } else if (!weak_frame) {
                cb(Err("Frame destroyed while fetching tasks."), nullptr);
                return;
              }

              tree_ptr->SetTasks(std::move(tasks));
              cb(Err(), weak_frame.get());
            });
        return;
      }
    }
  }

  callback(Err("No async task provider found for any frame in the current stack."), nullptr);
}

void ThreadImpl::ClearState() {
  current_stop_info_ = std::nullopt;
  state_ = std::nullopt;
  blocked_reason_ = debug_ipc::ThreadRecord::BlockedReason::kNotBlocked;
  stack_.ClearFrames();
  async_task_tree_.ClearTasks();
}

void ThreadImpl::RunNextPostStopTaskOrNotify(const StopInfo& info, bool should_stop,
                                             bool should_forward) {
  // It's possible that the user is typing "pause" or "continue" during any asynchronous tasks
  // so the thread state doesn't match what we thought it was. Even though we haven't sent the
  // notifications, things still could have happened.
  //
  // Therefore, we don't do anything if the thread has started running from underneath us (it
  // should always be stopped when the thread controllers are notified unless the user has done
  // something), and cancel other pending stop tasks.
  //
  // The other half of the race condition is user has requested a manual stop while we were
  // processing these post-stop tasks and we shouldn't continue it. This needs extra logic to
  // detect.
  //
  // TODO(https://fxbug.dev/42160760) don't automatically continue if the user has stopped the
  // thread.
  if (state_ == debug_ipc::ThreadRecord::State::kRunning) {
    post_stop_tasks_.clear();
    return;
  }

  bool debug_stepping = settings().GetBool(ClientSettings::Thread::kDebugStepping);

  if (post_stop_tasks_.empty()) {
    // No post-stop tasks left to run, dispatch the stop notification or continue.
    if (should_stop) {
      current_stop_info_ = info;
      // Stay stopped and notify the observers.
      bool debug_stepping = settings().GetBool(ClientSettings::Thread::kDebugStepping);
      if (debug_stepping)
        printf(" → Dispatching stop notification.\r\n");
      if (allow_notifications_) {
        for (auto& observer : session()->thread_observers()) {
          observer.OnThreadStopped(this, info);
        }
      }
    } else {
      // Controllers all say to continue.
      if (debug_stepping)
        printf(" → Sending continue request.\r\n");
      Continue(should_forward);
    }
  } else {
    // Run the next post-stop task.
    PostStopTask task = std::move(post_stop_tasks_.front());
    post_stop_tasks_.pop_front();
    task(fit::defer_callback(
        [weak_this = weak_factory_.GetWeakPtr(), info, should_stop, should_forward]() mutable {
          if (weak_this)
            weak_this->RunNextPostStopTaskOrNotify(info, should_stop, should_forward);
        }));
  }
}

}  // namespace zxdb
