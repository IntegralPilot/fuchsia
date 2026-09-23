// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_SYSTEM_LOG_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_SYSTEM_LOG_H_

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/time.h>

#include <memory>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/log_buffer.h"
#include "src/developer/forensics/feedback_data/log_source.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics::feedback {

// Collects the system log.
//
// The system log is streamed and buffered on the first call to Get and continues streaming until
// |active_period_| past the end of the call elapses.
//
// fuchsia.diagnostics.ArchiveAccessor.feedback is expected to be in |services|.
class SystemLog : public AttachmentProvider {
 public:
  SystemLog(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
            timekeeper::Clock* clock, RedactorBase* redactor, zx::duration active_period,
            cobalt::Logger* cobalt, LogBuffer* buffer);

  // Returns a promise to the system log and allows collection to be terminated early with |ticket|.
  ::fpromise::promise<AttachmentData> Get(uint64_t ticket) override;

  // Completes the system log collection promise associated with |ticket| early, if it hasn't
  // already completed.
  void ForceCompletion(uint64_t ticket, Error error) override;

 private:
  // Terminates the stream and flushes the in-memory buffer.
  void MakeInactive();

  async_dispatcher_t* dispatcher_;

  LogBuffer* buffer_;
  feedback_data::LogSource source_;

  timekeeper::Clock* clock_;
  cobalt::Logger* cobalt_;

  zx::duration active_period_;
  bool is_active_{false};

  std::map<uint64_t, std::function<void(Error)>> completers_;

  async::TaskClosureMethod<SystemLog, &SystemLog::MakeInactive> make_inactive_{this};
  fxl::WeakPtrFactory<SystemLog> ptr_factory_{this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_SYSTEM_LOG_H_
