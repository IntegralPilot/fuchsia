// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.memorypressure/cpp/fidl.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/developer/memory/monitor/monitor.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace pressure_signaler {
// Teach gtest how to print levels
extern void PrintTo(const pressure_signaler::Level& level, std::ostream* os) {
  std::string level_string;
  switch (level) {
    case pressure_signaler::Level::kImminentOOM:
      level_string = "ImminentOOM";
      break;
    case pressure_signaler::Level::kCritical:
      level_string = "Critical";
      break;
    case pressure_signaler::Level::kWarning:
      level_string = "Warning";
      break;
    case pressure_signaler::Level::kNormal:
      level_string = "Normal";
      break;
    case pressure_signaler::Level::kNumLevels:
      level_string = "Unset";
      break;
    default:
      level_string = "unknown";
      break;
  }
  *os << level_string;
}
}  // namespace pressure_signaler

namespace monitor::test {
namespace {
class MockPressureProvider : public fidl::Server<fuchsia_memorypressure::Provider>,
                             public fidl::AsyncEventHandler<fuchsia_memorypressure::Watcher> {
 public:
  explicit MockPressureProvider(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}
  MockPressureProvider(MockPressureProvider&& other) = default;
  void RegisterWatcher(RegisterWatcherRequest& request,
                       RegisterWatcherCompleter::Sync& completer) override {
    watcher.emplace(std::move(request.watcher()), dispatcher_, this);
  }

  void NotifyPressure(fuchsia_memorypressure::Level level) {
    (*watcher)->OnLevelChanged({{.level = level}}).Then([](auto) {});
  }

 private:
  std::optional<fidl::Client<fuchsia_memorypressure::Watcher>> watcher;
  async_dispatcher_t* dispatcher_;
};

class MockImminentOomObserver : public ImminentOomObserver {
 public:
  void SetImminentOom(bool imminent_oom) { imminent_oom_ = imminent_oom; }
  bool IsImminentOom() override { return imminent_oom_; }

 private:
  bool imminent_oom_;
};
}  // namespace

class MonitorPressureTest : public gtest::TestLoopFixture {
 public:
  std::tuple<std::unique_ptr<Monitor>, std::unique_ptr<MockPressureProvider>,
             std::unique_ptr<MockImminentOomObserver>>
  CreateMonitorAndPressureHelpers() {
    auto pressure_provider = std::make_unique<MockPressureProvider>(dispatcher());
    auto pressure_provider_endpoints = fidl::CreateEndpoints<fuchsia_memorypressure::Provider>();
    fidl::BindServer<fuchsia_memorypressure::Provider>(
        dispatcher(), std::move(pressure_provider_endpoints->server), pressure_provider.get());
    auto imminent_oom_observer = std::make_unique<MockImminentOomObserver>();
    auto monitor = std::make_unique<Monitor>(
        dispatcher(),
        memory_monitor_config::Config{{
            // Arbitrary non-zero values; cannot use default 0 initialization because it would
            // create a busy loop; Monitor checks for this and errors out.
            .critical_capture_delay_s = 10,
            .imminent_oom_capture_delay_s = 10,
            .normal_capture_delay_s = 10,
            .warning_capture_delay_s = 10,
        }},
        memory::CaptureMaker::Create(memory::CreateDefaultOS()).value(),
        fidl::Client<fuchsia_memorypressure::Provider>(
            std::move(pressure_provider_endpoints->client), dispatcher()),
        imminent_oom_observer.get());
    RunLoopUntilIdle();
    return {std::move(monitor), std::move(pressure_provider), std::move(imminent_oom_observer)};
  }
  static pressure_signaler::Level ExposeLevel(Monitor* monitor) { return monitor->level_; }
};

TEST_F(MonitorPressureTest, ImminentOom) {
  auto [monitor, pressure_provider, imminent_oom_observer] = CreateMonitorAndPressureHelpers();
  ASSERT_NE(ExposeLevel(monitor.get()), pressure_signaler::Level::kImminentOOM);
  imminent_oom_observer->SetImminentOom(true);
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kCritical);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kImminentOOM);
}

TEST_F(MonitorPressureTest, Critical) {
  auto [monitor, pressure_provider, imminent_oom_observer] = CreateMonitorAndPressureHelpers();
  ASSERT_NE(ExposeLevel(monitor.get()), pressure_signaler::Level::kCritical);
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kCritical);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kCritical);
}

TEST_F(MonitorPressureTest, AlternatingCriticalAndImminentOom) {
  auto [monitor, pressure_provider, imminent_oom_observer] = CreateMonitorAndPressureHelpers();
  ASSERT_NE(ExposeLevel(monitor.get()), pressure_signaler::Level::kCritical);

  // Critical
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kCritical);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kCritical);

  // While in critical, switch to imminent oom
  imminent_oom_observer->SetImminentOom(true);
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kCritical);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kImminentOOM);

  // Switch back to critical
  imminent_oom_observer->SetImminentOom(false);
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kCritical);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kCritical);
}

TEST_F(MonitorPressureTest, OnlyTestImminentOomDuringCritical) {
  auto [monitor, pressure_provider, imminent_oom_observer] = CreateMonitorAndPressureHelpers();
  ASSERT_NE(ExposeLevel(monitor.get()), pressure_signaler::Level::kCritical);
  imminent_oom_observer->SetImminentOom(true);

  // Normal
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kNormal);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kNormal);

  // Warning
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kWarning);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kWarning);

  // Critical
  pressure_provider->NotifyPressure(fuchsia_memorypressure::Level::kCritical);
  RunLoopUntilIdle();
  EXPECT_EQ(ExposeLevel(monitor.get()), pressure_signaler::Level::kImminentOOM);
}

}  // namespace monitor::test
