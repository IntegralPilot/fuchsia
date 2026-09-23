// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <assert.h>
#include <inttypes.h>
#include <lib/fit/defer.h>
#include <stdio.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <arch/exception.h>
#include <kernel/restricted.h>
#include <ktl/array.h>
#include <ktl/utility.h>
#include <object/exception_dispatcher.h>
#include <object/job_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

namespace {
const char* excp_type_to_string(uint type) {
  switch (type) {
    case ZX_EXCP_FATAL_PAGE_FAULT:
      return "fatal page fault";
    case ZX_EXCP_UNDEFINED_INSTRUCTION:
      return "undefined instruction";
    case ZX_EXCP_GENERAL:
      return "general fault";
    case ZX_EXCP_SW_BREAKPOINT:
      return "software breakpoint";
    case ZX_EXCP_HW_BREAKPOINT:
      return "hardware breakpoint";
    case ZX_EXCP_UNALIGNED_ACCESS:
      return "alignment fault";
    case ZX_EXCP_POLICY_ERROR:
      return "policy error";
    case ZX_EXCP_PROCESS_STARTING:
      return "process starting";
    case ZX_EXCP_THREAD_STARTING:
      return "thread starting";
    case ZX_EXCP_THREAD_EXITING:
      return "thread exiting";
    default:
      return "unknown fault";
  }
}

bool HasRestrictedInThreadHandler() { return Thread::Current::Get()->in_restricted(); }
}  // namespace

// This isn't an "iterator" in the pure c++ sense. We don't need all that
// complexity. I just couldn't think of a better term.
//
// Exception handlers are tried in the following order:
// - debugger
// - thread
// - process
// - debugger (in dealing with a second-chance exception)
// - job (first owning job, then its parent job, and so on up to root job)
class ExceptionHandlerIterator final {
 public:
  explicit ExceptionHandlerIterator(ThreadDispatcher* thread,
                                    fbl::RefPtr<ExceptionDispatcher> exception)
      : thread_(thread), exception_(ktl::move(exception)) {}

  // Sends the exception to the next registered handler, starting with
  // ZX_EXCEPTION_CHANNEL_TYPE_DEBUGGER (the process debug channel) on the first
  // call.
  //
  // Returns true with and fills |result| if the exception was sent to a
  // handler, or returns false if there are no more to try. Do not call this
  // function again after it returns false.
  bool Next(zx_status_t* result) {
    bool sent = false;

    while (true) {
      // This state may change during the handling of the debugger exception
      // channel. Accordingly, we check its value before the next round of
      // handling to be sure of the proper sequencing.
      bool second_chance = exception_->IsSecondChance();

      switch (next_method_) {
        case ExceptionDeliveryMethod::kFirstChanceDebugChannel:
          *result =
              thread_->HandleException(thread_->process()->debug_exceptionate(), exception_, &sent);
          if (HasRestrictedInThreadHandler()) {
            next_method_ = ExceptionDeliveryMethod::kRestrictedModeVectoredException;
          } else {
            next_method_ = ExceptionDeliveryMethod::kThreadChannel;
          }
          break;
        case ExceptionDeliveryMethod::kSecondChanceDebugChannel:
          *result =
              thread_->HandleException(thread_->process()->debug_exceptionate(), exception_, &sent);
          next_method_ = ExceptionDeliveryMethod::kJobChannel;
          next_job_ = thread_->process()->job();
          break;
        case ExceptionDeliveryMethod::kRestrictedModeVectoredException: {
          RestrictedState* rs = Thread::Current::restricted_state();
          DEBUG_ASSERT(rs != nullptr);

          zx_exception_report_t report{};
          const bool fill_report_success = exception_->FillReport(&report);
          ASSERT(fill_report_success);
          RedirectRestrictedExceptionToNormalMode(rs, report);

          // Handle the exception on behalf of restricted mode.
          //
          // Note this implies that restricted exceptions can never propagate
          // further and as such are ineligible for subsequent handlers.
          *result = ZX_OK;
          return true;
        }
        case ExceptionDeliveryMethod::kThreadChannel:
          *result = thread_->HandleException(thread_->exceptionate(), exception_, &sent);
          next_method_ = ExceptionDeliveryMethod::kProcessChannel;
          break;
        case ExceptionDeliveryMethod::kProcessChannel:
          *result = thread_->HandleException(thread_->process()->exceptionate(), exception_, &sent);

          if (second_chance) {
            next_method_ = ExceptionDeliveryMethod::kSecondChanceDebugChannel;
          } else {
            next_method_ = ExceptionDeliveryMethod::kJobChannel;
            next_job_ = thread_->process()->job();
          }
          break;
        case ExceptionDeliveryMethod::kJobChannel:
          if (next_job_ == nullptr) {
            // Reached the root job and there was no handler.
            return false;
          }
          *result = thread_->HandleException(next_job_->exceptionate(), exception_, &sent);
          next_job_ = next_job_->parent();
          break;
      }

      // Return to the caller once a handler was activated.
      if (sent) {
        return true;
      }
    }
    __UNREACHABLE;
  }

 private:
  enum class ExceptionDeliveryMethod {
    kFirstChanceDebugChannel,
    kRestrictedModeVectoredException,
    kThreadChannel,
    kProcessChannel,
    kSecondChanceDebugChannel,
    kJobChannel,
  };

  ThreadDispatcher* thread_;
  fbl::RefPtr<ExceptionDispatcher> exception_;
  ExceptionDeliveryMethod next_method_ = ExceptionDeliveryMethod::kFirstChanceDebugChannel;
  fbl::RefPtr<JobDispatcher> next_job_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(ExceptionHandlerIterator);
};

// Subroutine of dispatch_user_exception to simplify the code.
// One useful thing this does is guarantee ExceptionHandlerIterator is properly
// destructed.
//
// |*out_processed| is set to a boolean indicating if at least one
// handler processed the exception.
//
// Returns:
//   ZX_OK if the thread has been resumed.
//   ZX_ERR_NEXT if we ran out of handlers before the thread resumed.
//   ZX_ERR_INTERNAL_INTR_KILLED if the thread was killed.
//   ZX_ERR_NO_MEMORY on allocation failure (TODO(https://fxbug.dev/42108776): remove this case)
static zx_status_t exception_handler_worker(uint exception_type,
                                            const arch_exception_context_t* context,
                                            ThreadDispatcher* thread, bool* out_processed) {
  DEBUG_ASSERT(context != nullptr);

  *out_processed = false;

  zx_exception_report_t report = ExceptionDispatcher::BuildArchReport(exception_type, *context);

  fbl::RefPtr<ExceptionDispatcher> exception =
      ExceptionDispatcher::Create(fbl::RefPtr(thread), exception_type, &report, context);
  if (!exception) {
    // No memory to create the exception, we just have to drop it which
    // will kill the process.
    printf("KERN: failed to allocate memory for %s exception in user thread %lu.%lu\n",
           excp_type_to_string(exception_type), thread->process()->get_koid(), thread->get_koid());
    return ZX_ERR_NO_MEMORY;
  }

  // Most of the time we'll be holding the last reference to the exception
  // when this function exits, but if the task is killed we return HS_KILLED
  // without waiting for the handler which means someone may still have a
  // handle to the exception.
  //
  // For simplicity and to catch any unhandled status cases below, just clean
  // out the exception before returning no matter what.
  auto exception_cleaner = fit::defer([&exception]() { exception->Clear(); });

  ExceptionHandlerIterator iter(thread, exception);
  zx_status_t status = ZX_ERR_NEXT;
  while (iter.Next(&status)) {
    LTRACEF("handler returned %d\n", status);

    // ZX_ERR_NEXT means the handler wants to pass it up to the next in the
    // chain, keep looping but mark that at least one handler saw the exception.
    if (status == ZX_ERR_NEXT) {
      *out_processed = true;
      continue;
    }

    // ZX_ERR_STOP means the handler wants to kill the thread.
    if (status == ZX_ERR_STOP) {
      *out_processed = true;
      DEBUG_ASSERT(thread == ThreadDispatcher::GetCurrent());
      ThreadDispatcher::KillCurrent();
      return ZX_ERR_INTERNAL_INTR_KILLED;
    }

    // Anything other than ZX_ERR_NEXT means we're done.
    return status;
  }

  if (status != ZX_ERR_NEXT) {
    // If the iterator returned false, but status got changed from ZX_ERR_NEXT, then this means it
    // attempted to send to a handler, but the sending process failed. We still have no way to
    // handle the exception but we can at least make noise in the debuglog.
    printf(
        "KERN: Failed to deliver %s exception to exception handler in user thread %lu.%lu with "
        "status: %d\n",
        excp_type_to_string(exception_type), thread->process()->get_koid(), thread->get_koid(),
        status);
  }

  // If we got here we ran out of handlers and nobody resumed the thread.
  return ZX_ERR_NEXT;
}

// Dispatches an exception to the appropriate handler. Called by arch code
// when it cannot handle an exception.
//
// If we return ZX_OK, the caller is expected to resume the thread "as if"
// nothing happened, the handler is expected to have modified state such that
// resumption is possible.
//
// If we return ZX_ERR_BAD_STATE, the current thread is not a user thread
// (i.e., not associated with a ThreadDispatcher).
//
// Otherwise, we cause the current thread to exit and do not return at all.
//
// TODO(dje): Support unwinding from this exception and introducing a different
// exception?
zx_status_t dispatch_user_exception(uint exception_type,
                                    const arch_exception_context_t* arch_context) {
  LTRACEF("type %u, context %p\n", exception_type, arch_context);

  ThreadDispatcher* thread = ThreadDispatcher::GetCurrent();
  if (unlikely(!thread)) {
    // The current thread is not a user thread; bail.
    return ZX_ERR_BAD_STATE;
  }

  // From now until the exception is resolved the thread is in an exception.
  ThreadDispatcher::AutoBlocked by(ThreadDispatcher::Blocked::EXCEPTION);

  constexpr auto get_name = [](auto* task) -> ktl::array<char, ZX_MAX_NAME_LEN> {
    char name[ZX_MAX_NAME_LEN];
    [[maybe_unused]] zx_status_t status = task->get_name(name);
    // get_name cannot fail for a process or for a thread. We are processing an exception on the
    // current thread, so its state should remain RUNNING until we've decided to kill the thread
    // below.
    DEBUG_ASSERT(status == ZX_OK);
    return ktl::to_array(name);
  };

  auto dump_context = [&](const auto& pname) {
    printf("KERN: %s in user thread '%s' (%lu) in process '%s'\n",
           excp_type_to_string(exception_type), get_name(thread).data(), thread->get_koid(),
           pname.data());
    arch_dump_exception_context(arch_context);
  };

  bool processed = false;
  zx_status_t status = ZX_ERR_INTERNAL;
  {
    // We're about to handle the exception.  Use a |ScopedThreadExceptionContext| to make the
    // thread's user register state available to debuggers and exception handlers while the thread
    // is "in exception".
    ScopedThreadExceptionContext context(arch_context);
    status = exception_handler_worker(exception_type, arch_context, thread, &processed);
  }

  if (status == ZX_OK) {
    return ZX_OK;
  }

  // If the thread wasn't resumed or explicitly killed, kill the whole process.
  // If the process is critical to the root job, any fatal exception merits
  // logging all the details available whether the handler decided to kill the
  // process, or no handler processed it at all.
  ProcessDispatcher* process = thread->process();
  if (status == ZX_ERR_INTERNAL_INTR_KILLED) {
    // An exception handler has decided the process should be terminated.
    if (process->CriticalToRootJob()) {
      // Terminating a critical process will likely reboot the system so dump some extra diagnostics
      // to assist in debugging.
      printf("KERN: fatal exception in process critical to root job!\n");
      dump_context(get_name(process));
    }
  } else {
    auto pname = get_name(process);

    if (!processed) {
      // The exception was not handled. Normally, at least crashsvc would handle the exception and
      // make a smarter decision about what to do with it, but in case it didn't, dump some info to
      // the kernel logs.
      printf("KERN: exception_handler_worker returned %d\n", status);
      dump_context(pname);
    }

    // This log message is load-bearing server-side as it's used to identify the
    // critical component responsible for the reboot.
    // Please notify //src/developer/forensics/OWNERS upon changing.
    printf("KERN: terminating process '%s' (%lu)\n", pname.data(), process->get_koid());
    process->Kill(ZX_TASK_RETCODE_EXCEPTION_KILL);
  }

  // Either the current thread or its whole process was killed, we can now stop
  // it from running.
  ThreadDispatcher::ExitCurrent();
  panic("fell out of thread exit somehow!\n");
  __UNREACHABLE;
}

void dump_common_exception_context(const arch_exception_context_t* context) {
  // Print the common fields in arch_exception_context.
  //
  // In case of a page fault exception, the error code is typecast from zx_status_t to uint32_t when
  // populating user_synth_code. So also log the value typecast to zx_status_t to make debugging
  // easier.
  printf("synth_code %u (%d), synth_data %u\n", context->user_synth_code,
         static_cast<zx_status_t>(context->user_synth_code), context->user_synth_data);
}
