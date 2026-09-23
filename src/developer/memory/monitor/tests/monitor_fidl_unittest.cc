// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/executor.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <cstddef>

#include <gtest/gtest.h>

#include "lib/fpromise/result.h"
#include "src/developer/memory/monitor/monitor.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace monitor::test {

class FakeRamDevice : public fidl::Server<fuchsia_hardware_ram_metrics::Device> {
 public:
  void MeasureBandwidth(MeasureBandwidthRequest& request,
                        MeasureBandwidthCompleter::Sync& completer) override {
    auto mul = request.config().cycles_to_measure() / 1024;

    fuchsia_hardware_ram_metrics::BandwidthInfo info{
        {.timestamp = zx::msec(1234).to_nsecs(),
         .frequency = static_cast<uint64_t>(256 * 1024 * 1024),
         .bytes_per_cycle = 1,
         .channels = {{{{.readwrite_cycles = 10 * mul}},
                       {{.readwrite_cycles = 20 * mul}},
                       {{.readwrite_cycles = 30 * mul}},
                       {{.readwrite_cycles = 40 * mul}},
                       {{.readwrite_cycles = 50 * mul}},
                       {{.readwrite_cycles = 60 * mul}},
                       {{.readwrite_cycles = 70 * mul}},
                       {{.readwrite_cycles = 80 * mul}}}}}};
    fuchsia_hardware_ram_metrics::DeviceMeasureBandwidthResponse response{{info}};
    completer.Reply(fit::success(response));
  }

  void GetDdrWindowingResults(GetDdrWindowingResultsCompleter::Sync& completer) override {
    FAIL() << "Not implemented: GetDdrWindowingResults";
  }
};

class MockLogger : public fidl::Server<fuchsia_metrics::MetricEventLogger> {
 public:
  void LogMetricEvents(LogMetricEventsRequest& request,
                       LogMetricEventsCompleter::Sync& completer) override {
    num_calls_++;
    num_events_ += request.events().size();
    completer.Reply(fit::success());
  }
  size_t num_calls() const { return num_calls_; }
  size_t num_events() const { return num_events_; }

  void LogOccurrence(LogOccurrenceRequest& request,
                     LogOccurrenceCompleter::Sync& completer) override {
    FAIL() << "Not implemented";
  }
  void LogString(LogStringRequest& request, LogStringCompleter::Sync& completer) override {
    FAIL() << "Not implemented";
  }
  void LogInteger(LogIntegerRequest& request, LogIntegerCompleter::Sync& completer) override {
    FAIL() << "Not implemented";
  }
  void LogIntegerHistogram(LogIntegerHistogramRequest& request,
                           LogIntegerHistogramCompleter::Sync& completer) override {
    FAIL() << "Not implemented";
  }

 private:
  size_t num_calls_ = 0;
  size_t num_events_ = 0;
};

class MockLoggerFactory : public fidl::Server<fuchsia_metrics::MetricEventLoggerFactory> {
 public:
  explicit MockLoggerFactory(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}
  uint32_t received_project_id() const { return received_project_id_; }

  void CreateMetricEventLogger(CreateMetricEventLoggerRequest& request,
                               CreateMetricEventLoggerCompleter::Sync& completer) override {
    received_project_id_ = *request.project_spec().project_id();
    fidl::BindServer(dispatcher_, std::move(request.logger()), &logger_);
    completer.Reply(fit::success());
  }

  void CreateMetricEventLoggerWithExperiments(
      CreateMetricEventLoggerWithExperimentsRequest& request,
      CreateMetricEventLoggerWithExperimentsCompleter::Sync& completer) override {
    FAIL() << "Not implemented";
  }

 private:
  uint32_t received_project_id_;
  MockLogger logger_;
  async_dispatcher_t* dispatcher_;
};

class MemoryBandwidthInspectTest : public gtest::TestLoopFixture {
 public:
  MemoryBandwidthInspectTest() : executor_(dispatcher()), logger_factory_(dispatcher()) {
    auto logger_endpoints = fidl::CreateEndpoints<fuchsia_metrics::MetricEventLoggerFactory>();
    fidl::BindServer<fuchsia_metrics::MetricEventLoggerFactory>(
        dispatcher(), std::move(logger_endpoints->server), &logger_factory_);
    auto ram_endpoints = fidl::CreateEndpoints<fuchsia_hardware_ram_metrics::Device>();
    fidl::BindServer<fuchsia_hardware_ram_metrics::Device>(
        dispatcher(), std::move(ram_endpoints->server), &fake_device_);
    monitor_ = std::make_unique<Monitor>(
        dispatcher(),
        // Arbitrary non-zero delays
        memory_monitor_config::Config{{.critical_capture_delay_s = 10,
                                       .imminent_oom_capture_delay_s = 10,
                                       .normal_capture_delay_s = 10,
                                       .warning_capture_delay_s = 10}},
        *memory::CaptureMaker::Create(memory::CreateDefaultOS()),
        /* pressure_provider */ std::nullopt, /* imminent_oom_observer */ nullptr,
        fidl::Client<fuchsia_metrics::MetricEventLoggerFactory>{std::move(logger_endpoints->client),
                                                                dispatcher()},
        fidl::Client(std::move(ram_endpoints->client), dispatcher()));
    RunLoopUntilIdle();
  }

  void RunPromiseToCompletion(fpromise::promise<> promise) {
    bool done = false;
    executor_.schedule_task(std::move(promise).and_then([&]() { done = true; }));
    RunLoopUntilIdle();
    ASSERT_TRUE(done);
  }

  fpromise::result<inspect::Hierarchy> GetHierachyFromInspect() {
    fpromise::result<inspect::Hierarchy> hierarchy;
    RunPromiseToCompletion(inspect::ReadFromInspector(Inspector())
                               .then([&](fpromise::result<inspect::Hierarchy>& result) {
                                 hierarchy = std::move(result);
                               }));
    return hierarchy;
  }

  inspect::Inspector Inspector() { return monitor_->inspector_.inspector(); }

 private:
  async::Executor executor_;
  FakeRamDevice fake_device_;
  MockLoggerFactory logger_factory_;
  std::unique_ptr<Monitor> monitor_;
};

TEST_F(MemoryBandwidthInspectTest, MemoryBandwidth) {
  RunLoopUntilIdle();
  fpromise::result<inspect::Hierarchy> result = GetHierachyFromInspect();
  ASSERT_TRUE(result.is_ok());
  auto hierarchy = result.take_value();

  auto* metric_node = hierarchy.GetByPath({Metrics::kInspectPlatformNodeName});
  ASSERT_TRUE(metric_node);

  auto* metric_memory = metric_node->GetByPath({Metrics::kMemoryBandwidthNodeName});
  ASSERT_TRUE(metric_memory);

  auto* readings = metric_memory->node().get_property<inspect::UintArrayValue>(Metrics::kReadings);
  ASSERT_TRUE(readings);
  EXPECT_EQ(Metrics::kMemoryBandwidthArraySize, readings->value().size());
  EXPECT_EQ(94369704u, readings->value()[0]);

  auto* timestamp = metric_memory->node().get_property<inspect::IntPropertyValue>(
      Metrics::kReadingMemoryTimestamp);
  ASSERT_TRUE(timestamp);
}

}  // namespace monitor::test
