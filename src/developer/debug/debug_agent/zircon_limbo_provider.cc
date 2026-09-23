// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/zircon_limbo_provider.h"

#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <zircon/status.h>

#include "src/developer/debug/debug_agent/zircon_exception_handle.h"
#include "src/developer/debug/debug_agent/zircon_process_handle.h"
#include "src/developer/debug/debug_agent/zircon_thread_handle.h"
#include "src/developer/debug/shared/logging/logging.h"

namespace debug_agent {

namespace {

LimboProvider::Record MetadataToRecord(fuchsia_exception::ProcessExceptionMetadata metadata) {
  return ZirconLimboProvider::Record{
      .process = std::make_unique<ZirconProcessHandle>(std::move(*metadata.process())),
      .thread = std::make_unique<ZirconThreadHandle>(std::move(*metadata.thread()))};
}

}  // namespace

ZirconLimboProvider::ZirconLimboProvider(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir)
    : svc_dir_(svc_dir) {
  // Connect to process limbo.
  auto connect_res = component::ConnectAt<fuchsia_exception::ProcessLimbo>(svc_dir_);
  if (connect_res.is_error()) {
    // Connecting to a service is asynchronous and won't be handled here.
    LOGS(Error) << "Failed to connect to ProcessLimbo: " << connect_res.error_value();
    return;
  }
  fidl::SyncClient process_limbo(std::move(*connect_res));

  // Check if the limbo is active.
  auto active_res = process_limbo->WatchActive();
  if (active_res.is_error()) {
    if (active_res.error_value().is_peer_closed()) {
      LOGS(Warn) << "ProcessLimbo is not available";
      return;
    }
    LOGS(Error) << "Failed to WatchActive: " << active_res.error_value();
    return;
  }

  is_limbo_active_ = active_res->is_active();
  if (is_limbo_active_) {
    // Get the current set of process in exceptions.
    auto processes_res = process_limbo->WatchProcessesWaitingOnException();
    if (processes_res.is_error()) {
      LOGS(Error) << "Failed to WatchProcessesWaitingOnException: " << processes_res.error_value();
      return;
    }

    // Add all the current exceptions.
    for (auto& exception : processes_res->exception_list())
      limbo_[exception.info()->process_koid()] = MetadataToRecord(std::move(exception));
  }

  // Now that we were able to get the current state of the limbo, we move to an async binding.
  connection_.Bind(process_limbo.TakeClientEnd(), async_get_default_dispatcher());

  WatchActive();
  if (is_limbo_active_) {
    WatchLimbo();
  }

  valid_ = true;
}

void ZirconLimboProvider::WatchActive() {
  // |this| owns the connection, so it's guaranteed to outlive it.
  connection_->WatchActive().Then(
      [this](fidl::Result<fuchsia_exception::ProcessLimbo::WatchActive>& res) {
        if (!res.is_ok()) {
          LOGS(Error) << "Failed to WatchActive: " << res.error_value();
          return;
        }
        is_limbo_active_ = res->is_active();
        if (is_limbo_active_) {
          WatchLimbo();
        } else {
          limbo_.clear();
        }

        // Re-issue the hanging get.
        WatchActive();
      });
}

void ZirconLimboProvider::WatchLimbo() {
  connection_->WatchProcessesWaitingOnException().Then(
      // |this| owns the connection, so we're guaranteed to outlive it.
      [this](fidl::Result<fuchsia_exception::ProcessLimbo::WatchProcessesWaitingOnException>& res) {
        if (!res.is_ok()) {
          LOGS(Error) << "Failed to WatchProcessesWaitingOnException: " << res.error_value();
          return;
        }

        // The callback provides the full current list every time.
        RecordMap new_limbo;
        std::vector<zx_koid_t> new_exceptions;
        for (auto& exception : res->exception_list()) {
          zx_koid_t process_koid = exception.info()->process_koid();
          // Record if this is a new one we don't have yet.
          if (auto it = limbo_.find(process_koid); it == limbo_.end())
            new_exceptions.push_back(process_koid);

          new_limbo.insert({process_koid, MetadataToRecord(std::move(exception))});
        }

        limbo_ = std::move(new_limbo);

        if (on_enter_limbo_) {
          // Notify for the new exceptions.
          for (zx_koid_t process_koid : new_exceptions) {
            // Even though we added the exception above and expect it to be in limbo_, re-check
            // that we found it in case the callee consumed the exception out from under us.
            if (auto found = limbo_.find(process_koid); found != limbo_.end())
              on_enter_limbo_(found->second);
          }
        }

        // Re-issue the hanging get.
        WatchLimbo();
      });
}

bool ZirconLimboProvider::IsProcessInLimbo(zx_koid_t process_koid) const {
  const auto& records = GetLimboRecords();
  return records.find(process_koid) != records.end();
}

fit::result<debug::Status, ZirconLimboProvider::RetrievedException>
ZirconLimboProvider::RetrieveException(zx_koid_t process_koid) {
  if (!is_limbo_active_)
    return fit::error(debug::ZxStatus(ZX_ERR_NOT_FOUND));

  // Re-connect to process limbo so we could use the sync client.
  auto connect_res = component::ConnectAt<fuchsia_exception::ProcessLimbo>(svc_dir_);
  if (connect_res.is_error()) {
    LOGS(Error) << "Failed to connect to ProcessLimbo: " << connect_res.error_value();
    return fit::error(debug::ZxStatus(connect_res.error_value()));
  }
  fidl::SyncClient process_limbo(std::move(*connect_res));

  auto result = process_limbo->RetrieveException(process_koid);
  if (result.is_error()) {
    return fit::error(debug::Status(result.error_value().FormatDescription()));
  }

  fuchsia_exception::ProcessException& exception = result->process_exception();

  // Convert from the FIDL ExceptionInfo to the kernel zx_exception_info_t.
  const auto& source_info = exception.info();
  zx_exception_info_t info = {};
  info.pid = source_info->process_koid();
  info.tid = source_info->thread_koid();
  info.type = static_cast<zx_excp_type_t>(source_info->type());

  // Get the exception report from the thread.
  std::optional<zx_exception_report_t> report;
  if (exception.thread()) {
    zx_exception_report_t stack_report;
    if (exception.thread()->get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, &stack_report,
                                     sizeof(stack_report), nullptr, nullptr) == ZX_OK) {
      report = stack_report;
    }
  }

  RetrievedException retrieved;
  retrieved.process = std::make_unique<ZirconProcessHandle>(std::move(*exception.process()));
  retrieved.thread = std::make_unique<ZirconThreadHandle>(std::move(*exception.thread()));

  retrieved.exception =
      std::make_unique<ZirconExceptionHandle>(std::move(*exception.exception()), info, report);

  return fit::ok(std::move(retrieved));
}

debug::Status ZirconLimboProvider::ReleaseProcess(zx_koid_t process_koid) {
  // Re-connect to process limbo so we could use the sync client.
  auto connect_res = component::ConnectAt<fuchsia_exception::ProcessLimbo>(svc_dir_);
  if (connect_res.is_error()) {
    LOGS(Error) << "Failed to connect to ProcessLimbo: " << connect_res.error_value();
    return debug::ZxStatus(connect_res.error_value());
  }
  fidl::SyncClient process_limbo(std::move(*connect_res));

  auto result = process_limbo->ReleaseProcess(process_koid);
  if (result.is_error())
    return debug::Status(result.error_value().FormatDescription());

  limbo_.erase(process_koid);
  return debug::Status();
}

}  // namespace debug_agent
