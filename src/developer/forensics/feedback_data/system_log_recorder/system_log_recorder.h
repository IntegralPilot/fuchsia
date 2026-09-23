// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_SYSTEM_LOG_RECORDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_SYSTEM_LOG_RECORDER_H_

#include <fidl/fuchsia.feedback.internal/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/async_patterns/cpp/receiver.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/time.h>

#include <cstddef>
#include <cstdint>
#include <queue>
#include <string>
#include <unordered_map>

#include "src/developer/forensics/feedback_data/log_source.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/decoder.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/encoder.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/log_collector.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/log_message_store.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/writer.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {

class SystemLogRecorder : public fidl::Server<fuchsia_feedback_internal::SystemLogRecorder> {
 public:
  struct WriteParameters {
    zx::duration period;
    StorageSize max_write_size;
    std::string logs_dir;
    size_t max_num_files;
    StorageSize total_log_size;
    std::string metadata_path;
    StorageSize fallback_buffer_size;
  };

  SystemLogRecorder(async_dispatcher_t* archive_dispatcher, async_dispatcher_t* write_dispatcher,
                    std::shared_ptr<sys::ServiceDirectory> services,
                    WriteParameters write_parameters, std::unique_ptr<RedactorBase> redactor,
                    std::unique_ptr<Encoder> encoder, std::unique_ptr<Decoder> decoder);
  void Start();

  // Flushes cached logs to disk and calls |callback| when complete.
  void Flush(const std::optional<std::string>& message, ::fit::callback<void()> callback);

  // |fuchsia_feedback_internal::SystemLogRecorder|
  void GetCurrentBootLogs(GetCurrentBootLogsCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_feedback_internal::SystemLogRecorder> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "Received an unknown method with ordinal: " << metadata.method_ordinal;
  }

 private:
  void PeriodicWriteTask();
  void OnWriteComplete(SystemLogWriter::WriteResult result);
  void OnFlushComplete(bool success);
  void OnFlushAndReadLogsComplete(SystemLogWriter::FlushAndReadLogsResult result);
  void CollectArchivistLogs(GetCurrentBootLogsCompleter::Async completer);

  async_dispatcher_t* archive_dispatcher_;
  std::shared_ptr<sys::ServiceDirectory> services_;
  std::unique_ptr<RedactorBase> redactor_;
  const zx::duration write_period_;
  const StorageSize fallback_buffer_size_;

  LogMessageStore store_;
  LogSource log_source_;
  async_patterns::DispatcherBound<SystemLogWriter> writer_;
  async_patterns::Receiver<SystemLogRecorder> receiver_;
  std::queue<::fit::callback<void()>> flush_callbacks_;
  std::queue<GetCurrentBootLogsCompleter::Async> current_boot_logs_completers_;

  struct CollectOperation {
    std::unique_ptr<LogCollector> collector;
    GetCurrentBootLogsCompleter::Async completer;
  };
  uint64_t next_collection_id_;
  std::unordered_map<uint64_t, CollectOperation> in_flight_collections_;

  async::TaskClosureMethod<SystemLogRecorder, &SystemLogRecorder::PeriodicWriteTask>
      periodic_write_task_{this};
  fxl::WeakPtrFactory<SystemLogRecorder> ptr_factory_{this};
};

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_SYSTEM_LOG_RECORDER_H_
