// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"

#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

namespace forensics {
namespace stubs {

using fuchsia_metrics::Error;

void CobaltLoggerFactoryBase::CloseLoggerConnection() {
  if (logger_binding_.has_value()) {
    logger_binding_->Close(ZX_ERR_PEER_CLOSED);
  }
}

void CobaltLoggerFactory::CreateMetricEventLogger(
    CreateMetricEventLoggerRequest& request, CreateMetricEventLoggerCompleter::Sync& completer) {
  logger_binding_.emplace(dispatcher_, std::move(request.logger()), logger_.get(),
                          [](fidl::UnbindInfo info) {});
  completer.Reply(fit::success());
}

void CobaltLoggerFactoryClosesConnection::CreateMetricEventLogger(
    CreateMetricEventLoggerRequest& request, CreateMetricEventLoggerCompleter::Sync& completer) {
  logger_binding_.emplace(dispatcher_, std::move(request.logger()), logger_.get(),
                          [](fidl::UnbindInfo info) {});
  CloseLoggerConnection();
  completer.Reply(fit::success());
}

void CobaltLoggerFactoryFailsToCreateLogger::CreateMetricEventLogger(
    CreateMetricEventLoggerRequest& request, CreateMetricEventLoggerCompleter::Sync& completer) {
  completer.Reply(fit::error(Error::kInvalidArguments));
}

void CobaltLoggerFactoryCreatesOnRetry::CreateMetricEventLogger(
    CreateMetricEventLoggerRequest& request, CreateMetricEventLoggerCompleter::Sync& completer) {
  ++num_calls_;
  if (num_calls_ >= succeed_after_) {
    logger_binding_.emplace(dispatcher_, std::move(request.logger()), logger_.get(),
                            [](fidl::UnbindInfo info) {});
    completer.Reply(fit::success());
    return;
  }

  completer.Reply(fit::error(Error::kInvalidArguments));
}

void CobaltLoggerFactoryDelaysCallback::CreateMetricEventLogger(
    CreateMetricEventLoggerRequest& request, CreateMetricEventLoggerCompleter::Sync& completer) {
  logger_binding_.emplace(dispatcher_, std::move(request.logger()), logger_.get(),
                          [](fidl::UnbindInfo info) {});
  async::PostDelayedTask(
      dispatcher_, [completer = completer.ToAsync()]() mutable { completer.Reply(fit::success()); },
      delay_);
}

}  // namespace stubs
}  // namespace forensics
