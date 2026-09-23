// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/tests/stub_attachment_provider.h"

#include <lib/fpromise/bridge.h>

namespace forensics::feedback_data {

using feedback::AttachmentData;

StubAttachmentProvider::StubAttachmentProvider(std::string timeout_value)
    : timeout_value_(std::move(timeout_value)) {}

fpromise::promise<AttachmentData> StubAttachmentProvider::Get(const uint64_t ticket) {
  FX_CHECK(!completers_.contains(ticket)) << "Ticket used twice: " << ticket;

  fpromise::bridge<std::string, Error> bridge;

  // Construct a promise and an object that can be used to complete the promise with a value at a
  // later point in time.
  auto consume = bridge.consumer.promise_or(fpromise::error(Error::kLogicError));
  fit::callback<void(std::variant<std::string, Error>)> complete =
      [completer = std::move(bridge.completer)](std::variant<std::string, Error> result) mutable {
        if (std::holds_alternative<std::string>(result)) {
          completer.complete_ok(std::move(std::get<std::string>(result)));
        } else {
          completer.complete_error(std::get<Error>(result));
        }
      };

  completers_[ticket] = complete.share();

  fxl::WeakPtr<StubAttachmentProvider> self = ptr_factory_.GetWeakPtr();

  return consume
      .and_then([self, ticket](std::string& success_value) {
        if (self) {
          self->completers_.erase(ticket);
        }

        return fpromise::ok(AttachmentData(success_value));
      })
      .or_else([timeout_value = timeout_value_](const Error& error) {
        return fpromise::ok(AttachmentData(timeout_value, error));
      });
}

void StubAttachmentProvider::CompleteSuccessfully(std::string success_value) {
  for (auto& [ticket, completer] : completers_) {
    completer(std::move(success_value));
  }
}

void StubAttachmentProvider::ForceCompletion(const uint64_t ticket, const Error error) {
  if (completers_.contains(ticket) && completers_[ticket] != nullptr) {
    completers_[ticket](error);
  }
}

}  // namespace forensics::feedback_data
