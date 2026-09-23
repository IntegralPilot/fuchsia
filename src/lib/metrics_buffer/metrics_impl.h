// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_METRICS_BUFFER_METRICS_IMPL_H_
#define SRC_LIB_METRICS_BUFFER_METRICS_IMPL_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/fidl.h>

namespace cobalt {

// This class connects to the MetricsEventLoggerFactory and MetricsEventLogger fidl endpoints, and
// will re-connect starting with the fuchsia.io.Directory if an error occurs. The failing call is
// not retried.
class MetricsImpl {
 public:
  // called on dispatcher thread
  explicit MetricsImpl(async_dispatcher_t* dispatcher,
                       fidl::ClientEnd<fuchsia_io::Directory> directory, uint32_t project_id);
  // This is called on the dispatcher thread, with guarantee that anything queued to the dispatcher
  // during LogMetricEvents will run before ~MetricsImpl.
  ~MetricsImpl() = default;

  // This is called on an arbitrary thread which is not the dispatcher thread, with guarantee that
  // ~MetricsImpl will be queued to the dispatcher after anything queued to the dispatcher by this
  // call.
  void LogMetricEvents(std::vector<fuchsia_metrics::MetricEvent> events);

 private:
  const uint32_t project_id_ = 0;
  async_dispatcher_t* dispatcher_ = nullptr;

  fidl::ClientEnd<fuchsia_io::Directory> directory_;

  // These are wrapped in std::optional<> because destruction is (as of this comment) the only way
  // to unbind even if there's an async response in flight.
  std::optional<fidl::Client<fuchsia_metrics::MetricEventLoggerFactory>> logger_factory_;
  std::optional<fidl::Client<fuchsia_metrics::MetricEventLogger>> logger_;

  // called on dispatcher thread
  bool EnsureConnected();
  // called on dispatcher thread
  void Connect();
};

}  // namespace cobalt

#endif  // SRC_LIB_METRICS_BUFFER_METRICS_IMPL_H_
