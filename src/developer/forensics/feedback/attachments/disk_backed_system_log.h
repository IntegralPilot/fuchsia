// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_DISK_BACKED_SYSTEM_LOG_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_DISK_BACKED_SYSTEM_LOG_H_

#include <fidl/fuchsia.feedback.internal/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>
#include <lib/sys/cpp/service_directory.h>

#include <functional>
#include <map>
#include <memory>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/backoff/backoff.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace forensics::feedback {

// Retrieves disk-backed current boot system logs via FIDL from
// fuchsia.feedback.internal.SystemLogRecorder.
class DiskBackedSystemLog
    : public AttachmentProvider,
      public fidl::AsyncEventHandler<fuchsia_feedback_internal::SystemLogRecorder> {
 public:
  DiskBackedSystemLog(async_dispatcher_t* dispatcher,
                      std::shared_ptr<sys::ServiceDirectory> system_log_recorder_services,
                      std::unique_ptr<backoff::Backoff> backoff, RedactorBase* redactor,
                      cobalt::Logger* cobalt);

  // Returns a promise to the system log and allows collection to be terminated early with
  // |ticket|.
  ::fpromise::promise<AttachmentData> Get(uint64_t ticket) override;

  // Completes the system log collection promise associated with |ticket| early, if it hasn't
  // already completed.
  void ForceCompletion(uint64_t ticket, Error error) override;

  void on_fidl_error(fidl::UnbindInfo error) override;

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_feedback_internal::SystemLogRecorder> metadata) override;

 private:
  void Connect();

  async_dispatcher_t* dispatcher_;
  std::shared_ptr<sys::ServiceDirectory> system_log_recorder_services_;
  std::unique_ptr<backoff::Backoff> backoff_;
  RedactorBase* redactor_;
  cobalt::Logger* cobalt_;

  fidl::Client<fuchsia_feedback_internal::SystemLogRecorder> client_;

  async::TaskClosureMethod<DiskBackedSystemLog, &DiskBackedSystemLog::Connect> reconnect_task_{
      this};

  std::map<uint64_t, std::function<void(Error)>> completers_;
  fxl::WeakPtrFactory<DiskBackedSystemLog> ptr_factory_{this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_DISK_BACKED_SYSTEM_LOG_H_
