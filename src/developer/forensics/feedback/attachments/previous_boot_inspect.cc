// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/previous_boot_inspect.h"

#include <lib/async/cpp/task.h>
#include <lib/fdio/fd.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <utility>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/files/file.h"

namespace forensics::feedback {

PreviousBootInspect::PreviousBootInspect(async_dispatcher_t* dispatcher,
                                         std::shared_ptr<sys::ServiceDirectory> services,
                                         std::unique_ptr<backoff::Backoff> backoff,
                                         RedactorBase* redactor, std::string path)
    : backoff_(std::move(backoff)), redactor_(redactor), path_(std::move(path)) {
  // Don't fetch previous boot Inspect if it has already been written to path_.
  if (files::IsFile(path_)) {
    return;
  }

  data_provider_.set_error_handler([this, dispatcher, services](const zx_status_t status) {
    FX_PLOGS(WARNING, status)
        << "Lost connection to fuchsia.diagnostics.persistence.PreviousBootDataProvider";

    async::PostDelayedTask(
        dispatcher,
        [self = ptr_factory_.GetWeakPtr(), dispatcher, services] {
          if (self) {
            self->WatchPreviousBootData(dispatcher, services);
          }
        },
        backoff_->GetNext());
  });

  WatchPreviousBootData(dispatcher, std::move(services));
}

void PreviousBootInspect::WatchPreviousBootData(async_dispatcher_t* dispatcher,
                                                std::shared_ptr<sys::ServiceDirectory> services) {
  services->Connect(data_provider_.NewRequest(dispatcher));

  fuchsia::diagnostics::persistence::PreviousBootDataProviderOptions options;
  options.set_format(fuchsia::diagnostics::persistence::InspectFormat::JSON);

  data_provider_->WatchPreviousBootData(std::move(options), [this](auto result) {
    if (result.is_response()) {
      OnDataReceived(std::move(result.response().data));
    } else {
      OnError();
    }
  });
}

void PreviousBootInspect::OnError() {
  FX_LOGS(WARNING) << "Failed to watch previous boot inspect data";
  error_ = Error::kMissingValue;
  if (data_provider_.is_bound()) {
    data_provider_.Unbind();
  }
  auto completers = std::move(completers_);
  for (auto& [ticket, completer] : completers) {
    if (completer != nullptr) {
      completer(AttachmentData(Error::kMissingValue));
    }
  }
}

void PreviousBootInspect::OnDataReceived(fuchsia::diagnostics::persistence::PreviousBootData data) {
  if (!data.has_inspect() || !data.inspect().is_valid()) {
    FX_LOGS(WARNING) << "Previous boot inspect data is missing or invalid";
    error_ = Error::kMissingValue;
  } else {
    zx::channel channel = data.mutable_inspect()->TakeChannel();
    int fd = -1;
    if (const zx_status_t status = fdio_fd_create(channel.release(), &fd); status != ZX_OK) {
      FX_PLOGS(WARNING, status) << "Failed to create fd from previous boot inspect file";
      error_ = Error::kFileReadFailure;
    } else {
      std::string inspect_json;
      if (!files::ReadFileDescriptorToString(fd, &inspect_json) || inspect_json.empty()) {
        FX_LOGS(WARNING) << "Failed to read previous boot inspect file content";
        error_ = Error::kMissingValue;
      } else {
        redactor_->RedactJson(inspect_json);
        if (!files::WriteFile(path_, inspect_json)) {
          FX_LOGS(WARNING) << "Failed to write previous boot inspect to file: " << path_;
          error_ = Error::kFileWriteFailure;
        }
      }
      close(fd);
    }
  }

  if (data_provider_.is_bound()) {
    data_provider_.Unbind();
  }

  auto completers = std::move(completers_);
  for (auto& [ticket, completer] : completers) {
    if (completer != nullptr) {
      completer(error_.has_value() ? AttachmentData(*error_) : ReadAttachmentData());
    }
  }
}

AttachmentData PreviousBootInspect::ReadAttachmentData() const {
  std::string inspect_json;
  if (!files::ReadFileToString(path_, &inspect_json)) {
    FX_LOGS(WARNING) << "Failed to read previous boot inspect file: " << path_;
    return AttachmentData(Error::kFileReadFailure);
  }

  if (inspect_json.empty()) {
    FX_LOGS(WARNING) << "Previous boot inspect file was empty: " << path_;
    return AttachmentData(Error::kMissingValue);
  }

  return AttachmentData(std::move(inspect_json));
}

::fpromise::promise<AttachmentData> PreviousBootInspect::Get(const uint64_t ticket) {
  FX_CHECK(!completers_.contains(ticket)) << "Ticket used twice: " << ticket;

  if (files::IsFile(path_)) {
    return ::fpromise::make_ok_promise(ReadAttachmentData());
  }

  if (error_.has_value()) {
    return ::fpromise::make_ok_promise(AttachmentData(*error_));
  }

  ::fpromise::bridge<AttachmentData, void> bridge;
  completers_[ticket] = [completer = std::move(bridge.completer)](AttachmentData value) mutable {
    completer.complete_ok(std::move(value));
  };

  return bridge.consumer.promise_or(::fpromise::ok(AttachmentData(Error::kLogicError)));
}

void PreviousBootInspect::ForceCompletion(const uint64_t ticket, const Error error) {
  if (completers_.contains(ticket) && completers_[ticket] != nullptr) {
    auto completer = std::move(completers_[ticket]);
    completers_.erase(ticket);
    completer(AttachmentData(error));
  }
}

}  // namespace forensics::feedback
