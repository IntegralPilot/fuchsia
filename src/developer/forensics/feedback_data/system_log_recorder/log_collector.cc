// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/log_collector.h"

#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <utility>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/vmo.h"

namespace forensics::feedback_data::system_log_recorder {

LogCollector::LogCollector(async_dispatcher_t* dispatcher,
                           std::shared_ptr<sys::ServiceDirectory> services, StorageSize buffer_size,
                           RedactorBase* redactor)
    : dispatcher_(dispatcher),
      services_(std::move(services)),
      buffer_(buffer_size, redactor),
      started_(false) {}

void LogCollector::Start(Callback callback) {
  FX_CHECK(!started_) << "Start can only be called once";
  started_ = true;
  callback_ = std::move(callback);

  services_->Connect(archive_accessor_.NewRequest(dispatcher_), kArchiveAccessorName);

  archive_accessor_.set_error_handler([this](zx_status_t status) {
    FX_LOGS(WARNING) << "Lost connection to " << kArchiveAccessorName << ": "
                     << zx_status_get_string(status);
    OnError(Error::kStreamError);
  });

  const auto format = fuchsia::diagnostics::Format::FXT;
  fuchsia::diagnostics::StreamParameters params;
  params.set_data_type(fuchsia::diagnostics::DataType::LOGS)
      .set_format(format)
      .set_stream_mode(fuchsia::diagnostics::StreamMode::SNAPSHOT)
      .set_client_selector_configuration(
          fuchsia::diagnostics::ClientSelectorConfiguration::WithSelectAll(true));

  fuchsia::diagnostics::BatchIteratorPtr batch_iterator;
  archive_accessor_->StreamDiagnostics(std::move(params), batch_iterator.NewRequest(dispatcher_));

  iterator_ = std::make_unique<diagnostics::accessor2logger::LogBatchIterator>(
      std::move(batch_iterator), format);

  iterator_->set_error_handler([this](zx_status_t status) {
    FX_LOGS(WARNING) << "Lost connection to fuchsia.diagnostics.BatchIterator: "
                     << zx_status_get_string(status);
    OnError(Error::kStreamError);
  });

  GetNext();
}

void LogCollector::OnError(LogCollector::Error error) { Complete(fit::error(error)); }

void LogCollector::Complete(fit::result<LogCollector::Error, Logs> result) {
  if (!callback_) {
    return;
  }

  // Defer invoking the callback to the next dispatcher loop to ensure that any caller destroying
  // this LogCollector from within the callback will not trigger a use-after-free while unwinding
  // the current call stack.
  async::PostTask(dispatcher_, [cb = std::move(callback_), result = std::move(result)]() mutable {
    cb(std::move(result));
  });
}

void LogCollector::GetNext() {
  if (!callback_) {
    return;
  }

  iterator_->GetNext([this](auto result) {
    if (!callback_) {
      return;
    }

    if (result.is_error()) {
      FX_LOGS(ERROR) << "BatchIterator GetNext failed: " << result.error();
      OnError(Error::kStreamError);
      return;
    }

    if (result.value().empty()) {
      zx::result<zx::vmo> vmo = VmoFromString(buffer_.ToString());
      if (vmo.is_error()) {
        Complete(fit::error(Error::kVmoError));
        return;
      }

      Logs logs{
          .vmo = std::move(*vmo),
          .first_timestamp = buffer_.FirstTimestamp(),
          .last_timestamp = buffer_.LastTimestamp(),
      };
      Complete(fit::ok(std::move(logs)));
      return;
    }

    for (fpromise::result<fuchsia::logger::LogMessage, std::string>& msg : result.value()) {
      buffer_.Add(std::move(msg));
    }

    GetNext();
  });
}

}  // namespace forensics::feedback_data::system_log_recorder
