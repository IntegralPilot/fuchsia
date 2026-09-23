// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/enum_state_recorder.h>
#include <lib/power/state_recorder/cpp/numeric_state_recorder.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/event.h>
#include <lib/zx/time.h>
#include <zircon/syscalls.h>

#include <cstdint>
#include <map>

namespace {

using power_observability::EnumStateRecorder;
using power_observability::NumericStateRecorder;
using power_observability::Units;

enum class ChargingState : uint8_t {
  kDischarging = 0,
  kCharging = 1,
  kFullyCharged = 2,
};

const std::map<ChargingState, std::string> kChargingStates = {
    {ChargingState::kDischarging, "Discharging"},
    {ChargingState::kCharging, "Charging"},
    {ChargingState::kFullyCharged, "FullyCharged"},
};

class ExampleComponent {
 public:
  ExampleComponent()
      : loop_(&kAsyncLoopConfigAttachToCurrentThread),
        trace_provider_(loop_.dispatcher()),
        inspector_(loop_.dispatcher(), {}),
        manager_(inspector_),
        transition_task_(this) {
    // Run tasks to ensure the trace provider can see the trace category activate before we record
    // any events.
    loop_.Run(zx::deadline_after(zx::sec(1)));

    {
      // This recorder is not lazy while the other one is, as a way of demonstrating both
      // possibilities in this example.
      auto result = EnumStateRecorder<ChargingState>::Create(
          {
              .name = "charging_state",
              .states = kChargingStates,
              .trace_category_literal = "power_example",
          },
          {.capacity = 10, .lazy_record = false}, manager_);
      ZX_ASSERT_MSG(result.is_ok(), "Failed with status %s", result.status_string());
      charging_state_recorder_ = std::move(result.value());
    }

    {
      auto result = NumericStateRecorder<uint8_t>::Create(
          {
              .name = "battery_level",
              .units = Units::Percent(),
              .trace_category_literal = "power_example",
          },
          {.capacity = 30, .lazy_record = true}, manager_);
      ZX_ASSERT_MSG(result.is_ok(), "Failed with status %s", result.status_string());
      battery_level_recorder_ = std::move(result.value());
    }
  }

  ~ExampleComponent() { loop_.Shutdown(); }

  void Run() {
    transition_task_.PostDelayed(loop_.dispatcher(), zx::msec(500));
    loop_.Run();
  }

 private:
  void Transition(async_dispatcher_t* dispatcher, async::TaskBase* task, zx_status_t status) {
    if (status != ZX_OK) {
      return;
    }

    // Simulate a charging interval, followed by an interval at full charge, followed by an interval
    // discharging.
    ChargingState charging_state;
    uint8_t battery_level;
    if (loop_counter_ < 10) {
      charging_state = ChargingState::kCharging;
      battery_level = 90 + loop_counter_;
    } else if (loop_counter_ < 15) {
      charging_state = ChargingState::kFullyCharged;
      battery_level = 100;
    } else if (loop_counter_ < 25) {
      charging_state = ChargingState::kDischarging;
      battery_level = 100 - loop_counter_ + 15;
    } else {
      task->PostDelayed(dispatcher, zx::sec(1000));
      return;
    }

    if (charging_state != last_charging_state_) {
      charging_state_recorder_->Record(charging_state);
    }
    last_charging_state_ = charging_state;

    battery_level_recorder_->Record(battery_level);

    loop_counter_++;
    task->PostDelayed(dispatcher, zx::msec(500));
  }

  async::Loop loop_;
  trace::TraceProviderWithFdio trace_provider_;
  inspect::ComponentInspector inspector_;
  power_observability::StateRecorderManager manager_;
  std::optional<EnumStateRecorder<ChargingState>> charging_state_recorder_;
  std::optional<ChargingState> last_charging_state_;
  std::optional<NumericStateRecorder<uint8_t>> battery_level_recorder_;
  uint8_t loop_counter_ = 0;

  async::TaskMethod<ExampleComponent, &ExampleComponent::Transition> transition_task_;
};

}  // namespace

int main(int argc, const char** argv) {
  ExampleComponent component;
  component.Run();
  return 0;
}
