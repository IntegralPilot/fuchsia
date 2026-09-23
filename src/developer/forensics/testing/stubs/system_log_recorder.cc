// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/system_log_recorder.h"

#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <utility>

#include "src/lib/fsl/vmo/strings.h"

namespace forensics::stubs {

void SystemLogRecorder::SetResponse(
    fit::result<fuchsia_feedback_internal::RecorderError, std::string> response,
    fuchsia_feedback_internal::SystemLogMetadata metadata) {
  if (response.is_error()) {
    response_ = fit::error(response.error_value());
    return;
  }

  fsl::SizedVmo vmo;
  FX_CHECK(fsl::VmoFromString(response.value(), &vmo));

  fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse fidl_response;
  fidl_response.logs(std::move(vmo.vmo()));
  fidl_response.metadata(std::move(metadata));
  response_ = fit::ok(std::move(fidl_response));
}

void SystemLogRecorder::GetCurrentBootLogs(GetCurrentBootLogsCompleter::Sync& completer) {
  if (response_.is_error()) {
    completer.Reply(fit::error(response_.error_value()));
    return;
  }

  fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse response;
  if (response_->logs().has_value()) {
    if (response_->logs()->is_valid()) {
      zx::vmo vmo_dup;
      if (const zx_status_t status = response_->logs()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup);
          status == ZX_OK) {
        response.logs(std::move(vmo_dup));
      } else {
        FX_PLOGS(FATAL, status) << "Failed to duplicate VMO in stub";
      }
    } else {
      response.logs(zx::vmo());
    }
  }

  if (response_->metadata().has_value()) {
    response.metadata(response_->metadata().value());
  }

  completer.Reply(fit::ok(std::move(response)));
}

}  // namespace forensics::stubs
