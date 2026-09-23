// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_MONITOR_MONITOR_H_
#define SRC_DEVELOPER_MEMORY_MONITOR_MONITOR_H_

#include <fidl/fuchsia.hardware.ram.metrics/cpp/fidl.h>
#include <fidl/fuchsia.memory.inspection/cpp/fidl.h>
#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/trace/observer.h>
#include <lib/zx/socket.h>
#include <zircon/types.h>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/digest.h"
#include "src/developer/memory/monitor/high_water.h"
#include "src/developer/memory/monitor/imminent_oom_observer.h"
#include "src/developer/memory/monitor/logger.h"
#include "src/developer/memory/monitor/memory_monitor_config.h"
#include "src/developer/memory/monitor/metrics.h"

namespace monitor {

namespace test {
class MemoryBandwidthInspectTest;
class MonitorPressureTest;
}  // namespace test

class Monitor : public fidl::Server<fuchsia_memory_inspection::Collector>,
                public fidl::Server<fuchsia_memorypressure::Watcher> {
 public:
  Monitor(
      async_dispatcher_t* dispatcher, memory_monitor_config::Config config,
      memory::CaptureMaker capture_maker,
      std::optional<fidl::Client<fuchsia_memorypressure::Provider>> pressure_provider =
          std::nullopt,
      ImminentOomObserver* imminent_oom_observer = nullptr,
      std::optional<fidl::Client<fuchsia_metrics::MetricEventLoggerFactory>>
          metric_event_logger_factory = std::nullopt,
      std::optional<fidl::Client<fuchsia_hardware_ram_metrics::Device>> ram_device = std::nullopt);
  ~Monitor() override = default;

  // Writes a memory capture and the bucket definition to |socket| in JSON,
  // in UTF-8.
  // See the fuchsia.memory.inspection FIDL library for a
  // description of the format of the JSON.
  void CollectJsonStats(CollectJsonStatsRequest& request,
                        CollectJsonStatsCompleter::Sync& completer) override;

  void CollectJsonStatsWithOptions(CollectJsonStatsWithOptionsRequest& request,
                                   CollectJsonStatsWithOptionsCompleter::Sync& completer) override;

  // fuchsia_memorypressure::Watcher implementation; this callback gets called
  // whenever a memory pressure change is signaled.
  void OnLevelChanged(OnLevelChangedRequest& request,
                      OnLevelChangedCompleter::Sync& completer) override;

  static const char kTraceName[];

 private:
  void CollectJsonStatsWithOptions(zx::socket socket);

  void CreateMetrics();

  void UpdateState();

  void SampleAndPost();
  void MeasureBandwidthAndPost();
  void PeriodicMeasureBandwidth();
  inspect::Inspector Inspect();

  void OnLevelChanged(pressure_signaler::Level level);

  memory::CaptureMaker capture_maker_;
  HighWater high_water_;
  bool tracing_;
  zx_handle_t root_;
  async_dispatcher_t* dispatcher_;
  trace::TraceObserver trace_observer_;
  memory_monitor_config::Config config_;
  inspect::ComponentInspector inspector_;
  Logger logger_;
  std::optional<fidl::Client<fuchsia_metrics::MetricEventLoggerFactory>>
      metric_event_logger_factory_;
  std::optional<Metrics> metrics_;
  std::vector<memory::BucketMatch> bucket_matches_;
  memory::Digester digester_;
  std::optional<fidl::Client<fuchsia_hardware_ram_metrics::Device>> ram_device_;
  uint64_t pending_bandwidth_measurements_ = 0;
  pressure_signaler::Level level_;
  async::TaskClosureMethod<Monitor, &Monitor::SampleAndPost> sample_task_{this};
  async::TaskClosureMethod<Monitor, &Monitor::MeasureBandwidthAndPost> measure_bandwidth_task_{
      this};
  async::TaskClosureMethod<Monitor, &Monitor::PeriodicMeasureBandwidth>
      periodic_measure_bandwidth_task_{this};

  // Imminent OOM monitoring
  ImminentOomObserver* imminent_oom_observer_;

  friend class test::MemoryBandwidthInspectTest;
  friend class test::MonitorPressureTest;
  FXL_DISALLOW_COPY_AND_ASSIGN(Monitor);
};

}  // namespace monitor

#endif  // SRC_DEVELOPER_MEMORY_MONITOR_MONITOR_H_
