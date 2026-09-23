// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/metrics_buffer/metrics_impl.h"

#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>

#include "src/lib/metrics_buffer/log.h"

namespace cobalt {

using fuchsia_metrics::MetricEventLogger;
using fuchsia_metrics::MetricEventLoggerFactory;

using MetricsResult = ::fit::result<fidl::internal::ErrorsInImpl<fuchsia_metrics::Error>>;

MetricsImpl::MetricsImpl(async_dispatcher_t* dispatcher,
                         fidl::ClientEnd<fuchsia_io::Directory> directory, uint32_t project_id)
    : project_id_(project_id), dispatcher_(dispatcher), directory_(std::move(directory)) {}

void MetricsImpl::LogMetricEvents(std::vector<fuchsia_metrics::MetricEvent> events) {
  // failure to post intentionally drops events
  (void)async::PostTask(dispatcher_, [this, events = std::move(events)] {
    if (!EnsureConnected()) {
      // drop events
      return;
    }
    (*logger_)->LogMetricEvents({events}).Then([](MetricsResult result) {
      if (result.is_error() && result.error_value().is_framework_error()) {
        LOG(INFO, "LogMetricEvents failed (async): %s",
            result.error_value().FormatDescription().c_str());
        // A later call to LogMetricEvents will notice that the logger_ server end closed.
      }
      // drop events
    });
  });
}

// "Ensure" but only to the extent we can ensure without forcing a round-trip. If this returns true,
// the logger_.has_value() and is_valid(), at least until the caller returns back to the dispatcher.
bool MetricsImpl::EnsureConnected() {
  if (logger_.has_value() && logger_->is_valid()) {
    return true;
  }
  Connect();
  return logger_.has_value() && logger_->is_valid();
}

void MetricsImpl::Connect() {
  logger_factory_.reset();
  logger_.reset();

  auto logger_factory_result =
      component::ConnectAt<fuchsia_metrics::MetricEventLoggerFactory>(directory_);
  if (!logger_factory_result.is_ok()) {
    LOG(INFO, "!logger_factory_result.is_ok() - %s", logger_factory_result.status_string());
    return;
  }
  auto logger_factory = fidl::Client(std::move(logger_factory_result.value()), dispatcher_);

  auto logger_endpoints = fidl::CreateEndpoints<fuchsia_metrics::MetricEventLogger>();
  if (!logger_endpoints.is_ok()) {
    LOG(INFO, "!logger_endpoints.is_ok() - %s", logger_endpoints.status_string());
    return;
  }

  fuchsia_metrics::MetricEventLoggerFactoryCreateMetricEventLoggerRequest request;
  fuchsia_metrics::ProjectSpec project_spec;
  project_spec.project_id() = project_id_;
  request.project_spec() = std::move(project_spec);
  request.logger() = std::move(logger_endpoints->server);
  logger_factory->CreateMetricEventLogger(std::move(request)).Then([](MetricsResult result) {
    if (result.is_error()) {
      LOG(INFO, "CreateMetricEventLogger failed (async): %s",
          result.error_value().FormatDescription().c_str());
      // A later call to LogMetricEvents will notice that the logger_ server end closed.
    }
  });

  logger_factory_ = std::move(logger_factory);
  logger_ = fidl::Client(std::move(logger_endpoints->client), dispatcher_);
}

}  // namespace cobalt
