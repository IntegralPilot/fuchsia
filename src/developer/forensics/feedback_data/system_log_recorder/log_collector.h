// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_LOG_COLLECTOR_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_LOG_COLLECTOR_H_

#include <fuchsia/diagnostics/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>

#include <cstdint>
#include <memory>
#include <optional>

#include "src/developer/forensics/feedback_data/log_buffer.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/diagnostics/accessor2logger/log_message.h"

namespace forensics::feedback_data::system_log_recorder {

// Collects a snapshot of system logs from Archivist and returns them as a VMO. Does NOT
// continuously stream logs after the snapshot has been collected.
class LogCollector {
 public:
  enum class Error : uint8_t {
    kStreamError,
    kVmoError,
  };

  struct Logs {
    zx::vmo vmo;
    std::optional<zx::time_boot> first_timestamp;
    std::optional<zx::time_boot> last_timestamp;
  };

  using Callback = fit::callback<void(fit::result<Error, Logs>)>;

  LogCollector(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
               StorageSize buffer_size, RedactorBase* redactor);

  // Starts collecting the log snapshot from Archivist. Once collection completes or an error
  // occurs, |callback| is invoked on the dispatcher provided at construction with the resulting
  // logs or an error.
  //
  // Can only be called once.
  void Start(Callback callback);

 private:
  void GetNext();
  void OnError(Error error);
  void Complete(fit::result<Error, Logs> result);

  async_dispatcher_t* dispatcher_;
  std::shared_ptr<sys::ServiceDirectory> services_;
  feedback::LogBuffer buffer_;
  Callback callback_;
  bool started_;

  fuchsia::diagnostics::ArchiveAccessorPtr archive_accessor_;
  std::unique_ptr<diagnostics::accessor2logger::LogBatchIterator> iterator_;
};

}  // namespace forensics::feedback_data::system_log_recorder

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_LOG_COLLECTOR_H_
