// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_HRTIMER_DRIVERS_AML_HRTIMER_AML_HRTIMER_SERVER_H_
#define SRC_DEVICES_HRTIMER_DRIVERS_AML_HRTIMER_AML_HRTIMER_SERVER_H_

#include <fidl/fuchsia.hardware.hrtimer/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/async/cpp/irq.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/fit/result.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/interrupt.h>

#include <optional>
#include <variant>

#include "lib/inspect/component/cpp/component.h"

namespace hrtimer {
constexpr size_t kTimersAll[] = {0, 1, 2, 3, 4, 5, 6, 7, 8};
constexpr size_t kTimersSupportWait[] = {0, 1, 2, 3, 5, 6, 7, 8};
static constexpr size_t kNumberOfTimers = std::size(kTimersAll);

class AmlHrtimerServer : public fidl::Server<fuchsia_hardware_hrtimer::Device> {
 public:
  // Cast is needed because PowerLevel and BinaryPowerLevel are distinct types.
  static const fuchsia_power_broker::PowerLevel kWakeHandlingLeaseOn =
      static_cast<fuchsia_power_broker::PowerLevel>(fuchsia_power_broker::BinaryPowerLevel::kOn);

  AmlHrtimerServer(async_dispatcher_t* dispatcher, fdf::MmioBuffer mmio,
                   std::optional<fidl::SyncClient<fuchsia_power_system::ActivityGovernor>> sag,
                   zx::interrupt irq_a, zx::interrupt irq_b, zx::interrupt irq_c,
                   zx::interrupt irq_d, zx::interrupt irq_f, zx::interrupt irq_g,
                   zx::interrupt irq_h, zx::interrupt irq_i, inspect::ComponentInspector& inspect);

  void ShutDown();

  // For unit testing.
  static size_t GetNumberOfTimers() { return kNumberOfTimers; }
  bool HasWaitCompleter(size_t timer_index) {
    ZX_ASSERT(timer_index < kNumberOfTimers);
    return !std::holds_alternative<std::monostate>(
        timers_[timer_index].power_enabled_wait_completer);
  }
  bool StartTicksLeftFitInHardware(size_t timer_index) {
    ZX_ASSERT(timer_index < kNumberOfTimers);
    // This unit testing method is only meant to be used when extending max ticks.
    ZX_ASSERT(timers_[timer_index].properties.extend_max_ticks);
    return timers_[timer_index].start_ticks_left <= std::numeric_limits<uint16_t>::max();
  }

 protected:
  // FIDL natural C++ methods for fuchsia.hardware.hrtimer.
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void Stop(StopRequest& request, StopCompleter::Sync& completer) override;
  void ReadTimer(ReadTimerRequest& request, ReadTimerCompleter::Sync& completer) override;
  void ReadClock(ReadClockRequest& request, ReadClockCompleter::Sync& completer) override;
  void GetTicksLeft(GetTicksLeftRequest& request, GetTicksLeftCompleter::Sync& completer) override;
  void SetEvent(SetEventRequest& request, SetEventCompleter::Sync& completer) override;
  void StartAndWait(StartAndWaitRequest& request, StartAndWaitCompleter::Sync& completer) override;
  void StartAndWait2(StartAndWait2Request& request,
                     StartAndWait2Completer::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_hrtimer::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  static constexpr size_t kMaxInspectEvents = 256;  // Arbitrary.
  enum class MaxTicks : uint8_t {
    k16Bit,
    k64Bit,
  };
  enum class EventType : uint8_t {
    None,
    Start,
    StartAndWait,
    StartAndWait2,
    StartHardware,
    RetriggerIrq,
    TriggerIrqWait,
    TriggerIrqWait2,
    TriggerIrq,
    Stop,
    StopWait,
    StopWait2,
  };

  struct TimersProperties {
    uint64_t id;
    bool supports_notifications;
    bool supports_system_clock;
    bool supports_1usec;
    bool supports_10usecs;
    bool supports_100usecs;
    bool supports_1msec;
    MaxTicks max_ticks_support;
    bool always_on_domain;
    bool watchdog;
    bool extend_max_ticks;
  };
  struct Timer {
    Timer(AmlHrtimerServer& server, TimersProperties& props);
    void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                   const zx_packet_interrupt_t* interrupt);
    AmlHrtimerServer& parent;
    TimersProperties& properties;
    uint64_t resolution_nsecs;
    std::optional<zx::event> event;
    zx::interrupt irq;
    async::IrqMethod<Timer, &Timer::HandleIrq> irq_handler{this};
    // Completer saved to reply to a StartAndWait power aware FIDL call.
    std::variant<std::monostate, StartAndWaitCompleter::Async, StartAndWait2Completer::Async>
        power_enabled_wait_completer;
    uint64_t start_ticks_left = 0;
    uint64_t last_ticks = 0;
  };

  static size_t TimerIndexFromId(uint64_t id) { return id; }

  inspect::UintProperty& IrqEntries() { return irq_entries_; }
  inspect::UintProperty& IrqExits() { return irq_exits_; }

  bool IsTimerStarted(size_t id);
  fit::result<const fuchsia_hardware_hrtimer::DriverError> StartHardware(size_t timer_index);
  void RecordEvent(int64_t now, uint64_t id, EventType type, uint64_t data);

  TimersProperties timers_properties_[kNumberOfTimers] = {
      // clang-format off
      // id| notif|system|   1us|  10us| 100us|   1ms|        max ticks| AOdom|   WDT|extend|
      {   0,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, false},  // A.
      {   1,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, false},  // B.
      {   2,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, false},  // C.
      {   3,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, false},  // D.
      {   4, false,  true,  true,  true,  true, false, MaxTicks::k64Bit, false, false, false},  // E.
      {   5,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, true },  // F.
      {   6,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, true },  // G.
      {   7,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, true },  // H.
      {   8,  true, false,  true,  true,  true,  true, MaxTicks::k16Bit, false, false, true },  // I.
      // The timers below are available in the hardware but not supported by this driver.
      // Timer id 9 is a WDT 24MHz.
      // {   9,  true, false, false, false, false, false, MaxTicks::k16Bit,false, true , false},
      // {  10,  true, false,  true,  true,  true, false, MaxTicks::k16Bit, true, false, false},  // AO_A.
      // {  11,  true, false,  true,  true,  true, false, MaxTicks::k16Bit, true, false, false},  // AO_B.
      // {  12, false, false,  true,  true,  true, false, MaxTicks::k16Bit, true, false, false},  // AO_C.
      // // There is no AO_D.
      // {  13, false,  true, false, false, false, false, MaxTicks::k64Bit, true, false, false},  // AO_E.
      // {  14, false,  true, false, false, false, false, MaxTicks::k64Bit, true, false, false},  // AO_F.
      // {  15, false,  true, false, false, false, false, MaxTicks::k64Bit, true, false, false},  // AO_G.
      // Timer id 16 is a AO_WDT.
      // {  16, true,   true, false, false, false, false, MaxTicks::k16Bit, true, true , false},
      // clang-format on
  };

  std::array<Timer, kNumberOfTimers> timers_ = {
      Timer(*this, timers_properties_[0]), Timer(*this, timers_properties_[1]),
      Timer(*this, timers_properties_[2]), Timer(*this, timers_properties_[3]),
      Timer(*this, timers_properties_[4]), Timer(*this, timers_properties_[5]),
      Timer(*this, timers_properties_[6]), Timer(*this, timers_properties_[7]),
      Timer(*this, timers_properties_[8])};
  std::optional<fdf::MmioBuffer> mmio_;
  // FIDL client used to request wake leases directly from SAG.
  std::optional<fidl::SyncClient<fuchsia_power_system::ActivityGovernor>> sag_;
  async_dispatcher_t* dispatcher_;

  inspect::Node inspect_node_;
  struct Event {
    int64_t timestamp;
    uint64_t id;
    uint64_t data;
    EventType type;
  } events_[kMaxInspectEvents] = {};
  size_t event_index_ = 0;
  // TODO(b/369886005): These inspect properties exist to help diagnose b/369886005
  // and can probably be safely removed once that bug is resolved.
  inspect::UintProperty lease_requests_;
  inspect::UintProperty lease_replies_;
  inspect::UintProperty update_requests_;
  inspect::UintProperty update_replies_;
  inspect::UintProperty irq_entries_;
  inspect::UintProperty irq_exits_;
};

}  // namespace hrtimer
#endif  // SRC_DEVICES_HRTIMER_DRIVERS_AML_HRTIMER_AML_HRTIMER_SERVER_H_
