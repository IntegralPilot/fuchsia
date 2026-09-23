// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/disk_backed_system_log.h"

#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <string>
#include <utility>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/vmo.h"

namespace forensics::feedback {
namespace {

struct LogData {
  std::string contents;
  std::optional<zx::time_boot> first_timestamp;
  std::optional<zx::time_boot> last_timestamp;
  std::optional<std::string> source;
};

// Creates a callable object that can be used to complete the system log collection flow with an
// ok status or a timeout and a promise to consume that result.
auto CompletesAndConsume() {
  ::fpromise::bridge<LogData, Error> bridge;
  auto completer =
      std::make_shared<::fpromise::completer<LogData, Error>>(std::move(bridge.completer));

  return std::make_tuple(
      [completer](LogData data) {
        if (!*completer) {
          return;
        }

        completer->complete_ok(std::move(data));
      },
      [completer](Error error) {
        if (!*completer) {
          return;
        }

        FX_LOGS(WARNING) << "System log collection error " << ToString(error);
        completer->complete_error(error);
      },
      std::move(bridge.consumer).promise_or(::fpromise::error(Error::kLogicError)));
}

}  // namespace

DiskBackedSystemLog::DiskBackedSystemLog(
    async_dispatcher_t* dispatcher,
    std::shared_ptr<sys::ServiceDirectory> system_log_recorder_services,
    std::unique_ptr<backoff::Backoff> backoff, RedactorBase* redactor, cobalt::Logger* cobalt)
    : dispatcher_(dispatcher),
      system_log_recorder_services_(std::move(system_log_recorder_services)),
      backoff_(std::move(backoff)),
      redactor_(redactor),
      cobalt_(cobalt) {
  Connect();
}

void DiskBackedSystemLog::Connect() {
  if (system_log_recorder_services_ == nullptr) {
    FX_LOGS(WARNING)
        << "SystemLogRecorder service directory is null, cannot connect to SystemLogRecorder";
    return;
  }

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_feedback_internal::SystemLogRecorder>();
  if (endpoints.is_error()) {
    FX_LOGS(ERROR) << "Failed to create endpoints: " << endpoints.status_string();
    return;
  }

  system_log_recorder_services_->Connect(
      fuchsia_feedback_internal::SystemLogRecorder::kDiscoverableName,
      endpoints->server.TakeChannel());

  client_ = fidl::Client(std::move(endpoints->client), dispatcher_, this);
}

void DiskBackedSystemLog::on_fidl_error(fidl::UnbindInfo error) {
  if (error.status() == ZX_ERR_NOT_FOUND) {
    // Invalidate the client so that future requests aren't made.
    client_ = fidl::Client<fuchsia_feedback_internal::SystemLogRecorder>();
    FX_LOGS(ERROR)
        << "SystemLogRecorder not found, will not attempt to reconnect. This should never happen.";
    return;
  }

  FX_LOGS(WARNING) << "Lost connection to SystemLogRecorder: " << error;
  reconnect_task_.PostDelayed(dispatcher_, backoff_->GetNext());
}

void DiskBackedSystemLog::handle_unknown_event(
    fidl::UnknownEventMetadata<fuchsia_feedback_internal::SystemLogRecorder> metadata) {
  FX_LOGS(ERROR) << "Unexpected event ordinal: " << metadata.event_ordinal;
}

::fpromise::promise<AttachmentData> DiskBackedSystemLog::Get(uint64_t ticket) {
  FX_CHECK(!completers_.contains(ticket)) << "Ticket used twice: " << ticket;

  AttachmentMetadata metadata({
      {feedback_data::kAttachmentMetadataSourceKey, feedback_data::kAttachmentMetadataSourceDisk},
  });

  if (!client_.is_valid()) {
    return ::fpromise::make_ok_promise(
        AttachmentData(Error::kConnectionError, std::move(metadata)));
  }

  auto [complete_ok, complete_error, consume] = CompletesAndConsume();

  completers_[ticket] = complete_error;

  client_->GetCurrentBootLogs().Then(
      [this, complete_ok = std::move(complete_ok), complete_error = std::move(complete_error),
       redactor = redactor_](
          fidl::Result<fuchsia_feedback_internal::SystemLogRecorder::GetCurrentBootLogs>&
              result) mutable {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "GetCurrentBootLogs failed: "
                         << result.error_value().FormatDescription();
          const Error error = result.error_value().is_framework_error() ? Error::kConnectionError
                                                                        : Error::kFileReadFailure;
          complete_error(error);
          return;
        }

        backoff_->Reset();

        if (!result->logs().has_value() || !result->logs()->is_valid()) {
          FX_LOGS(ERROR) << "GetCurrentBootLogs returned missing or invalid VMO";
          complete_error(Error::kBadValue);
          return;
        }

        LogData log_data;
        if (result->metadata().has_value()) {
          log_data.first_timestamp = result->metadata()->first_timestamp();
          log_data.last_timestamp = result->metadata()->last_timestamp();

          if (result->metadata()->source().has_value()) {
            switch (*result->metadata()->source()) {
              case fuchsia_feedback_internal::SystemLogSource::kDisk:
                log_data.source = feedback_data::kAttachmentMetadataSourceDisk;
                break;
              case fuchsia_feedback_internal::SystemLogSource::kStream:
                log_data.source = feedback_data::kAttachmentMetadataSourceStream;
                break;
              default:
                FX_LOGS(WARNING) << "Unknown SystemLogSource enum value: "
                                 << static_cast<uint32_t>(*result->metadata()->source());
                break;
            }
          }
        }

        const zx::vmo vmo = std::move(*result->logs());

        zx::result<std::string> contents = StringFromVmo(vmo);
        if (contents.is_error()) {
          complete_error(Error::kBadValue);
          return;
        }

        redactor->Redact(*contents);

        if (contents->empty()) {
          complete_error(Error::kMissingValue);
          return;
        }

        log_data.contents = std::move(*contents);
        complete_ok(std::move(log_data));
      });

  fxl::WeakPtr<DiskBackedSystemLog> self = ptr_factory_.GetWeakPtr();

  return consume.then([self, ticket, metadata = std::move(metadata)](
                          ::fpromise::result<LogData, Error>& result) mutable
                          -> ::fpromise::result<AttachmentData> {
    if (!self) {
      return ::fpromise::ok(AttachmentData(Error::kLogicError, std::move(metadata)));
    }

    self->completers_.erase(ticket);

    if (result.is_error()) {
      if (result.error() == Error::kLogicError) {
        FX_LOGS(FATAL) << "Log collection promise was incorrectly dropped";
      }
      return ::fpromise::ok(AttachmentData(result.error(), std::move(metadata)));
    }

    LogData& data = result.value();
    if (data.source.has_value()) {
      metadata = {{feedback_data::kAttachmentMetadataSourceKey, *data.source}};
    }

    // The logs are sorted, so we can assume the platform's log buffer is at capacity if the
    // first log has a timestamp > 0.
    if (data.first_timestamp.has_value() && *data.first_timestamp > zx::time_boot(0)) {
      self->cobalt_->LogIntegerEvent(cobalt_registry::kSyslogBytesAtCapacityMetricId,
                                     data.contents.size());

      if (data.last_timestamp.has_value()) {
        const int64_t duration = (*data.last_timestamp - *data.first_timestamp).to_mins();
        self->cobalt_->LogIntegerEvent(cobalt_registry::kSyslogDurationAtCapacityMetricId,
                                       duration);
      }
    }

    return ::fpromise::ok(AttachmentData(std::move(data.contents), std::move(metadata)));
  });
}

void DiskBackedSystemLog::ForceCompletion(const uint64_t ticket, const Error error) {
  // Extract the completer from the map before calling it to avoid potential use-after-free.
  if (auto node = completers_.extract(ticket); node) {
    node.mapped()(error);
  }
}

}  // namespace forensics::feedback
