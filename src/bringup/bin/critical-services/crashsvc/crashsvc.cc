// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/critical-services/crashsvc/crashsvc.h"

#include <lib/async/cpp/task.h>
#include <lib/zx/channel.h>
#include <lib/zx/exception.h>
#include <lib/zx/handle.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

#include <memory>

#include <inspector/inspector.h>

#include "src/bringup/bin/critical-services/crashsvc/exception_handler.h"
#include "src/bringup/bin/critical-services/crashsvc/logging.h"
#include "src/lib/debug/backtrace-request-utils.h"
#include "src/lib/fsl/handles/object_info.h"

namespace {

// crashsvc gives 8s to the exception handler to respond that it is active, otherwise it releases
// the exception to prevent potentially sending the exception into oblivion in the channel in case
// the handler is truly unresponsive.
//
// This value was picked as something that seems reasonable. It may need to be adjusted.
constexpr auto kExceptionHandlerTimeout = zx::sec(8);

// Cleans up and resumes a thread in a manual backtrace request.
//
// This may modify |regs| via cleanup_backtrace_request().
//
// Returns true and sets |exception| to resume on close on success.
bool ResumeIfBacktraceRequest(const zx::thread& thread, const zx::exception& exception,
                              const zx_exception_info& info, zx_thread_state_general_regs_t* regs) {
  if (is_backtrace_request(info.type, regs)) {
    if (const zx_status_t status = cleanup_backtrace_request(thread.get(), regs); status != ZX_OK) {
      LogError("failed to cleanup backtrace", info, status);
      return false;
    }

    // Mark the exception as handled so the thread resumes execution.
    uint32_t state = ZX_EXCEPTION_STATE_HANDLED;
    if (const zx_status_t status =
            exception.set_property(ZX_PROP_EXCEPTION_STATE, &state, sizeof(state));
        status != ZX_OK) {
      LogError("failed to resume from backtrace", info, status);
      return false;
    }

    return true;
  }

  return false;
}

void HandOffException(async_dispatcher_t* dispatcher, zx::exception exception,
                      const zx_exception_info_t& info, ExceptionHandler& handler,
                      bool& early_bailout) {
  zx::process process;
  if (const zx_status_t status = exception.get_process(&process); status != ZX_OK) {
    LogError("failed to get exception process when receiving exception", info, status);
    return;
  }

  zx::thread thread;
  if (const zx_status_t status = exception.get_thread(&thread); status != ZX_OK) {
    LogError("failed to get exception thread when receiving exception", info, status);
    return;
  }

  // A backtrace request should just dump and continue.
  zx_thread_state_general_regs_t regs;
  if (const zx_status_t status = inspector_read_general_regs(thread.get(), &regs);
      status != ZX_OK) {
    LogError("failed to get general registers", info, status);
  }

  // If this is a backtrace request, we print the requested thread(s) and then return.
  if (ResumeIfBacktraceRequest(thread, exception, info, &regs)) {
    if (is_backtrace_request_current_thread(&regs)) {
      inspector_print_debug_info(stdout, process.get(), thread.get());
    } else if (is_backtrace_request_specific_thread(&regs)) {
      inspector_print_debug_info_for_thread(stdout, process.get(),
                                            get_backtrace_request_koid(&regs));
    } else {
      inspector_print_debug_info_for_all_threads(stdout, process.get());
    }
    return;
  }

  // Dump the crash info to the logs whether we have a FIDL handler or not.
  const std::string process_name = fsl::GetObjectName(process.get());
  if (early_bailout) {
    fprintf(
        stdout,
        "crashsvc: component manager already crashed, won't process exception for process \"%s\"\n",
        process_name.c_str());
    return;
  }

  fprintf(stdout, "crashsvc: exception received for process \"%s\", processing\n",
          process_name.c_str());
  inspector_print_debug_info(stdout, process.get(), thread.get());

  // If the process serving fuchsia.exception.Handler crashes, the system will send future requests
  // for the protocol to the crashes server because it isn't techically terminated. As a result, the
  // exception for the server is stuck in the channel and cannot be released to terminate the
  // process. The exception is released here to terminate the process as early as possible (not via
  // timeout) and allow the system to recover quickly and gracefully.
  //
  // This needs to be kept in sync with the name of the process serving
  // fuchsia.exception.Handler.
  //
  // DO NOT REMOVE: Removing this will lead to delays in exception handlings.
  if (process_name == "exceptions.cm") {
    LogError("cannot handle exception for the process serving fuchsia.exception.Handler",
             ZX_ERR_NOT_SUPPORTED);

    // Release the exception to let the kernel terminate the process.
    exception.reset();
    return;
  }

  // If an ancestor of the component serving fuchsia.exception.Handler crashes, the system may be
  // unable to meaningfully handle the exception because entries in exception handler's "/svc"
  // directory may be unserviceable and synchronous operations may hang indefinitely.
  //
  // DO NOT REMOVE: exceptions.cm has a synchronous dependency on component_manager. Removing this
  // may cause component_manager crashes to become unrecoverable and the device stuck in a bad
  // state.
  if (process_name == "bin/component_manager") {
    LogError(
        "cannot handle exception for a process that is a necessary dependency of the process "
        "serving fuchsia.exception.Handler",
        ZX_ERR_NOT_SUPPORTED);

    // Delay releasing the exception to give the stack trace a chance to propagate to the previous
    // boot logs. The kernel won't terminate the process until we release the exception.
    async::PostDelayedTask(
        dispatcher, [exception = std::move(exception)]() mutable { exception.reset(); },
        zx::sec(5));

    // Once component manager has crashed, any subsequent crashes are likely just noise and fill up
    // the DLOG.
    early_bailout = true;
    return;
  }

  // If the process serving fuchsia.logger.LogSink crashes, an exception handler down the line
  // could get spawned and try to set up its syslog on startup, which is currently done via a
  // blocking call, so it would hang and then the exception would be held forever.
  //
  // This needs to be kept in sync with the name of the process serving fuchsia.logger.LogSink.
  //
  // Given that as of August 1, 2023, archivist.cm is a critical process to the root job, the
  // system will reboot upon the exception release and a crash report won't have time to be filed
  // anyway so we might as well bail out here earlier without loss of reporting.
  //
  // DO NOT REMOVE: Removing this will lead to archivist crashes leaving the device stuck in a bad
  // state instead of rebooting.
  if (process_name == "archivist.cm") {
    LogError("cannot handle exception for the server of fuchsia.logger.LogSink",
             ZX_ERR_NOT_SUPPORTED);

    // Release the exception to let the kernel terminate the process.
    exception.reset();
    return;
  }

  // Send over the exception to the handler.
  // From this point on, crashsvc has no ownership over the exception and it's up to the handler to
  // decide when and how to resume it.
  //
  // This is done asynchronously to give queued tasks, like the reconnection logic, a chance to
  // execute.
  async::PostTask(dispatcher, [&handler, info, exception = std::move(exception)]() mutable {
    handler.Handle(std::move(exception), info);
  });
}

}  // namespace

zx::result<std::unique_ptr<async::Wait>> start_crashsvc(
    async_dispatcher_t* dispatcher, zx::channel exception_channel,
    fidl::ClientEnd<fuchsia_io::Directory> exception_handler_svc) {
  std::unique_ptr wait = std::make_unique<async::Wait>(
      exception_channel.get(), ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, /*options=*/0u,
      [exception_channel = std::move(exception_channel),
       handler = std::make_unique<ExceptionHandler>(dispatcher, std::move(exception_handler_svc),
                                                    kExceptionHandlerTimeout),
       early_bailout = false](async_dispatcher_t* dispatcher, async::Wait* wait, zx_status_t status,
                              const zx_packet_signal_t* signal) mutable {
        if (status == ZX_ERR_CANCELED) {
          return;
        }

        if (signal->observed & ZX_CHANNEL_PEER_CLOSED) {
          // We should only get here in crashsvc's unit tests. In production, our job is actually
          // the root job so the system will halt before closing its exception channel.
          return;
        }

        zx_exception_info_t info;
        zx::exception exception;
        if (const zx_status_t status = exception_channel.read(
                0, &info, exception.reset_and_get_address(), sizeof(info), 1, nullptr, nullptr);
            status != ZX_OK) {
          LogError("failed to read from the exception channel", status);
          return;
        }

        HandOffException(dispatcher, std::move(exception), info, *handler, early_bailout);

        if (const zx_status_t status = wait->Begin(dispatcher); status != ZX_OK) {
          LogError("Failed to restart wait, crashsvc won't continue", status);
          return;
        }
      });

  if (const zx_status_t status = wait->Begin(dispatcher); status != ZX_OK) {
    LogError("Failed to begin wait, crashsvc won't start", status);
    return zx::error(status);
  }

  return zx::ok(std::move(wait));
}
