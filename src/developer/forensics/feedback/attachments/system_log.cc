// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/system_log.h"

#include <fuchsia/logger/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/function.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <string>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/purge_memory.h"
#include "src/lib/backoff/exponential_backoff.h"

namespace forensics::feedback {

SystemLog::SystemLog(async_dispatcher_t* dispatcher,
                     std::shared_ptr<sys::ServiceDirectory> services, timekeeper::Clock* clock,
                     RedactorBase* redactor, const zx::duration active_period,
                     cobalt::Logger* cobalt, LogBuffer* buffer)
    : dispatcher_(dispatcher),
      buffer_(buffer),
      source_(dispatcher, services, buffer_,
              std::make_unique<backoff::ExponentialBackoff>(zx::min(1), 2u, zx::hour(1))),
      clock_(clock),
      cobalt_(cobalt),
      active_period_(active_period) {}

namespace {

// Creates a callable object that can be used to complete the system log collection flow with an
// ok status or a timeout and a promise to consume that result.
auto CompletesAndConsume() {
  ::fpromise::bridge<void, Error> bridge;
  auto completer =
      std::make_shared<::fpromise::completer<void, Error>>(std::move(bridge.completer));

  return std::make_tuple(
      [completer] {
        if (!*(completer)) {
          return;
        }

        completer->complete_ok();
      },
      [completer](const Error error) {
        if (!*(completer)) {
          return;
        }

        FX_LOGS(WARNING) << "System log collection error " << ToString(error);
        completer->complete_error(error);
      },
      std::move(bridge.consumer).promise_or(::fpromise::error(Error::kLogicError)));
}

}  // namespace

::fpromise::promise<AttachmentData> SystemLog::Get(const uint64_t ticket) {
  FX_CHECK(!completers_.contains(ticket)) << "Ticket used twice: " << ticket;

  if (!is_active_) {
    is_active_ = true;
    source_.Start();
  }

  auto [complete_ok, complete_error, consume] = CompletesAndConsume();

  completers_[ticket] = std::move(complete_error);

  // Cancel the outstanding |make_inactive_| because logs are being requested.
  make_inactive_.Cancel();

  fxl::WeakPtr<SystemLog> self = ptr_factory_.GetWeakPtr();

  buffer_->ExecuteAfter(clock_->BootNow(), std::move(complete_ok));

  return consume.then([self, ticket](const ::fpromise::result<void, Error>& result)
                          -> ::fpromise::result<AttachmentData> {
    const AttachmentMetadata metadata{
        {
            feedback_data::kAttachmentMetadataSourceKey,
            feedback_data::kAttachmentMetadataSourceStream,
        },
    };

    if (!self) {
      return ::fpromise::ok(AttachmentData(Error::kLogicError, metadata));
    }

    if (result.is_error() && result.error() == Error::kLogicError) {
      FX_LOGS(FATAL) << "Log collection promise was incorrectly dropped";
    }

    self->completers_.erase(ticket);

    // Cancel the outstanding |make_inactive_| because the "active" period should be extended.
    self->make_inactive_.Cancel();
    self->make_inactive_.PostDelayed(self->dispatcher_, self->active_period_);

    std::string system_log = self->buffer_->ToString();

    // ToString sorts the logs, so we can assume the platform's log buffer is at capacity if the
    // first log has a timestamp > 0. Note that this is not referring to Feedback's buffer's
    // capacity (checked by EnforceCapacity).
    if (const std::optional<zx::time_boot> first_timestamp = self->buffer_->FirstTimestamp();
        first_timestamp.has_value() && first_timestamp.value() > zx::time_boot(0)) {
      self->cobalt_->LogIntegerEvent(cobalt_registry::kSyslogBytesAtCapacityMetricId,
                                     system_log.size());

      if (const std::optional<zx::time_boot> last_timestamp = self->buffer_->LastTimestamp();
          last_timestamp.has_value()) {
        const int64_t duration = (*last_timestamp - *first_timestamp).to_mins();
        self->cobalt_->LogIntegerEvent(cobalt_registry::kSyslogDurationAtCapacityMetricId,
                                       duration);
      }
    }

    if (system_log.empty()) {
      const Error error = (result.is_ok()) ? Error::kMissingValue : result.error();
      return ::fpromise::ok(AttachmentData(error, metadata));
    }

    return ::fpromise::ok(result.is_ok()
                              ? AttachmentData(std::move(system_log), metadata)
                              : AttachmentData(std::move(system_log), result.error(), metadata));
  });
}

void SystemLog::ForceCompletion(const uint64_t ticket, const Error error) {
  if (completers_.contains(ticket)) {
    completers_[ticket](error);
  }
}

void SystemLog::MakeInactive() {
  FX_LOGS(INFO) << "System log not requested for " << active_period_.to_secs()
                << " seconds after last collection terminated, stopping streaming";

  is_active_ = false;
  source_.Stop();

  PurgeAllMemoryAfter(dispatcher_, zx::sec(0));
}

}  // namespace forensics::feedback
