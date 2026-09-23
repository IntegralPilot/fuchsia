// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/system_log_recorder.h"

#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

#include "src/developer/forensics/utils/purge_memory.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {

// No rate limiting in the first minute of recording to allow us to catch up on all the log
// messages prior to listening.
constexpr zx::duration kNoRateLimitDuration = zx::sec(60);

// 150 seconds was chosen as an appropriate delay in https://fxbug.dev/397695466. Experiments showed
// memory usage staying low after an initial surge of usage within the first 150 seconds. This surge
// could be due to the initial snapshot of logs from Diagnostics or due to rate limiting being
// disabled for the first 60 seconds.
constexpr zx::duration kPurgeMemoryDuration = zx::sec(150);

SystemLogRecorder::SystemLogRecorder(async_dispatcher_t* archive_dispatcher,
                                     async_dispatcher_t* write_dispatcher,
                                     std::shared_ptr<sys::ServiceDirectory> services,
                                     WriteParameters write_parameters,
                                     std::unique_ptr<RedactorBase> redactor,
                                     std::unique_ptr<Encoder> encoder,
                                     std::unique_ptr<Decoder> decoder)
    : archive_dispatcher_(archive_dispatcher),
      services_(services),
      redactor_(std::move(redactor)),
      write_period_(write_parameters.period),
      fallback_buffer_size_(write_parameters.fallback_buffer_size),
      store_(write_parameters.total_log_size / write_parameters.max_num_files,
             write_parameters.max_write_size, redactor_.get(), std::move(encoder)),
      log_source_(archive_dispatcher, std::move(services), &store_),
      writer_(write_dispatcher, std::in_place, write_parameters.logs_dir,
              write_parameters.max_num_files, std::move(decoder), write_parameters.metadata_path),
      receiver_(this, archive_dispatcher),
      next_collection_id_(0) {}

void SystemLogRecorder::Start() {
  log_source_.Start();
  periodic_write_task_.Post(archive_dispatcher_);

  async::PostDelayedTask(
      archive_dispatcher_, [this] { store_.TurnOnRateLimiting(); }, kNoRateLimitDuration);

  PurgeAllMemoryAfter(archive_dispatcher_, kPurgeMemoryDuration);
}

void SystemLogRecorder::Flush(const std::optional<std::string>& message,
                              ::fit::callback<void()> callback) {
  FX_LOGS(INFO) << "Received signal to flush cached logs to disk";
  if (message.has_value()) {
    store_.AppendToEnd(message.value());
  }

  // Consume the data on the main thread, then pass the data to the writer thread to ensure data is
  // safely written to disk ASAP when the system log recorder (and maybe the system) is expected to
  // stop soon.
  LogMessageStore::ConsumeResult result = store_.Consume();
  writer_.AsyncCall(&SystemLogWriter::Write, std::move(result));

  flush_callbacks_.push(std::move(callback));
  writer_.AsyncCall(&SystemLogWriter::Fsync)
      .Then(receiver_.Once(&SystemLogRecorder::OnFlushComplete));
}

void SystemLogRecorder::OnFlushComplete(bool) {
  if (flush_callbacks_.empty()) {
    return;
  }

  ::fit::callback<void()> callback = std::move(flush_callbacks_.front());
  flush_callbacks_.pop();
  if (callback) {
    callback();
  }
}

void SystemLogRecorder::PeriodicWriteTask() {
  // Consume the data on the main thread to avoid thread safety issues in store_. Move the data to
  // the writer thread.
  LogMessageStore::ConsumeResult result = store_.Consume();
  writer_.AsyncCall(&SystemLogWriter::Write, std::move(result))
      .Then(receiver_.Once(&SystemLogRecorder::OnWriteComplete));
}

void SystemLogRecorder::OnWriteComplete(const SystemLogWriter::WriteResult result) {
  if (result == SystemLogWriter::WriteResult::kCachePurge) {
    store_.Reset();
  }

  periodic_write_task_.PostDelayed(archive_dispatcher_, write_period_);
}

void SystemLogRecorder::GetCurrentBootLogs(GetCurrentBootLogsCompleter::Sync& completer) {
  LogMessageStore::ConsumeResult result = store_.Consume();
  current_boot_logs_completers_.push(completer.ToAsync());
  writer_.AsyncCall(&SystemLogWriter::FlushAndReadLogs, std::move(result))
      .Then(receiver_.Once(&SystemLogRecorder::OnFlushAndReadLogsComplete));
}

void SystemLogRecorder::OnFlushAndReadLogsComplete(SystemLogWriter::FlushAndReadLogsResult result) {
  if (result.cache_purged) {
    store_.Reset();
  }

  if (current_boot_logs_completers_.empty()) {
    FX_LOGS(ERROR) << "current_boot_logs_completers_ empty";
    return;
  }

  GetCurrentBootLogsCompleter::Async completer = std::move(current_boot_logs_completers_.front());
  current_boot_logs_completers_.pop();

  if (result.logs.is_error()) {
    switch (result.logs.error_value()) {
      case SystemLogWriter::WriterError::kIoError:
        completer.Reply(fit::error(fuchsia_feedback_internal::RecorderError::kIoError));
        return;
      case SystemLogWriter::WriterError::kDecompressionError:
        completer.Reply(fit::error(fuchsia_feedback_internal::RecorderError::kDecompressionError));
        return;
      case SystemLogWriter::WriterError::kVmoError:
        completer.Reply(fit::error(fuchsia_feedback_internal::RecorderError::kVmoError));
        return;
      case SystemLogWriter::WriterError::kInsufficientCoverage:
        CollectArchivistLogs(std::move(completer));
        return;
    }
  }

  fuchsia_feedback_internal::SystemLogMetadata metadata;
  metadata.first_timestamp(result.logs->first_timestamp);
  metadata.last_timestamp(result.logs->last_timestamp);
  metadata.source(fuchsia_feedback_internal::SystemLogSource::kDisk);

  fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse response;
  response.logs(std::move(result.logs->vmo));
  response.metadata(std::move(metadata));

  completer.Reply(fit::ok(std::move(response)));
}

void SystemLogRecorder::CollectArchivistLogs(GetCurrentBootLogsCompleter::Async completer) {
  const uint64_t id = next_collection_id_++;
  auto collector = std::make_unique<LogCollector>(archive_dispatcher_, services_,
                                                  fallback_buffer_size_, redactor_.get());

  LogCollector* collector_ptr = collector.get();
  in_flight_collections_.emplace(id, CollectOperation{
                                         .collector = std::move(collector),
                                         .completer = std::move(completer),
                                     });

  collector_ptr->Start([self = ptr_factory_.GetWeakPtr(),
                        id](fit::result<LogCollector::Error, LogCollector::Logs> result) {
    if (!self) {
      return;
    }

    auto node = self->in_flight_collections_.extract(id);
    if (node.empty()) {
      FX_LOGS(ERROR) << "No callback for collection id: " << id;
      return;
    }

    CollectOperation operation = std::move(node.mapped());

    if (result.is_error()) {
      switch (result.error_value()) {
        case LogCollector::Error::kStreamError:
          operation.completer.Reply(
              fit::error(fuchsia_feedback_internal::RecorderError::kStreamError));
          return;
        case LogCollector::Error::kVmoError:
          operation.completer.Reply(
              fit::error(fuchsia_feedback_internal::RecorderError::kVmoError));
          return;
      }
    }

    LogCollector::Logs logs = std::move(result.value());

    fuchsia_feedback_internal::SystemLogMetadata metadata;
    metadata.first_timestamp(logs.first_timestamp);
    metadata.last_timestamp(logs.last_timestamp);
    metadata.source(fuchsia_feedback_internal::SystemLogSource::kStream);

    fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse response;
    response.logs(std::move(logs.vmo));
    response.metadata(std::move(metadata));

    operation.completer.Reply(fit::ok(std::move(response)));
  });
}

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
