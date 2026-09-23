// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_SYSTEM_LOG_RECORDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_SYSTEM_LOG_RECORDER_H_

#include <fidl/fuchsia.feedback.internal/cpp/fidl.h>
#include <fidl/fuchsia.feedback.internal/cpp/test_base.h>
#include <lib/fit/result.h>

#include <string>

#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics::stubs {

class SystemLogRecorder
    : public SingleBindingFidlServer<fuchsia_feedback_internal::SystemLogRecorder> {
 public:
  void GetCurrentBootLogs(GetCurrentBootLogsCompleter::Sync& completer) override;

  void SetResponse(fit::result<fuchsia_feedback_internal::RecorderError, std::string> response,
                   fuchsia_feedback_internal::SystemLogMetadata metadata = {});

  void SetResponse(
      fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse response) {
    response_ = fit::ok(std::move(response));
  }

 private:
  fit::result<fuchsia_feedback_internal::RecorderError,
              fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse>
      response_{fit::ok(fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse{})};
};

}  // namespace forensics::stubs

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_SYSTEM_LOG_RECORDER_H_
