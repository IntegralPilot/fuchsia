// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/devices/hrtimer/drivers/aml-hrtimer/aml-hrtimer-server.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/fit/defer.h>

#include <variant>

#include "src/devices/hrtimer/drivers/aml-hrtimer/aml-hrtimer-regs.h"

namespace hrtimer {

// To use switch like logic for std::visitor on std::variat.
template <class... Ts>
struct overloaded : Ts... {
  using Ts::operator()...;
};

AmlHrtimerServer::AmlHrtimerServer(
    async_dispatcher_t* dispatcher, fdf::MmioBuffer mmio,
    std::optional<fidl::SyncClient<fuchsia_power_system::ActivityGovernor>> sag,
    zx::interrupt irq_a, zx::interrupt irq_b, zx::interrupt irq_c, zx::interrupt irq_d,
    zx::interrupt irq_f, zx::interrupt irq_g, zx::interrupt irq_h, zx::interrupt irq_i,
    inspect::ComponentInspector& inspect)
    : sag_(std::move(sag)), inspect_node_(inspect.root().CreateChild("hrtimer-trace")) {
  mmio_.emplace(std::move(mmio));
  dispatcher_ = dispatcher;

  lease_requests_ = inspect_node_.CreateUint("lease_requests", 0);
  lease_replies_ = inspect_node_.CreateUint("lease_replies", 0);
  update_requests_ = inspect_node_.CreateUint("update_requests", 0);
  update_replies_ = inspect_node_.CreateUint("update_replies", 0);
  irq_entries_ = inspect_node_.CreateUint("irq_entries", 0);
  irq_exits_ = inspect_node_.CreateUint("irq_exits", 0);
  inspect_node_.RecordLazyValues("lazy_values", [&]() {
    inspect::Inspector inspector;
    auto all = inspector.GetRoot().CreateChild("events");
    all.RecordUint("event_index", event_index_);

    for (size_t i = 0; i < kMaxInspectEvents; ++i) {
      if (events_[i].type == EventType::None) {
        continue;
      }
      const char* type = nullptr;
      switch (events_[i].type) {
          // clang-format off
        case EventType::None:            type = "";                break;
        case EventType::Start:           type = "Start";           break;
        case EventType::StartAndWait:    type = "StartAndWait";    break;
        case EventType::StartAndWait2:   type = "StartAndWait2";   break;
        case EventType::StartHardware:   type = "StartHardware";   break;
        case EventType::RetriggerIrq:    type = "RetriggerIrq";    break;
        case EventType::TriggerIrq:      type = "TriggerIrq";      break;
        case EventType::TriggerIrqWait:  type = "TriggerIrqWait";  break;
        case EventType::TriggerIrqWait2: type = "TriggerIrqWait2"; break;
        case EventType::Stop:            type = "Stop";            break;
        case EventType::StopWait:        type = "StopWait";        break;
        case EventType::StopWait2:       type = "StopWait2";       break;
          // clang-format on
      }
      all.RecordChild(std::to_string(i).c_str(), [&](inspect::Node& event) {
        event.RecordInt("@time", events_[i].timestamp);
        event.RecordUint("id", events_[i].id);
        event.RecordString("type", type);
        event.RecordUint("data", events_[i].data);
      });
    }
    inspector.emplace(std::move(all));
    return fpromise::make_ok_promise(std::move(inspector));
  });

  timers_[0].irq_handler.set_object(irq_a.get());
  timers_[1].irq_handler.set_object(irq_b.get());
  timers_[2].irq_handler.set_object(irq_c.get());
  timers_[3].irq_handler.set_object(irq_d.get());
  // No IRQ on timer id 4.
  timers_[5].irq_handler.set_object(irq_f.get());
  timers_[6].irq_handler.set_object(irq_g.get());
  timers_[7].irq_handler.set_object(irq_h.get());
  timers_[8].irq_handler.set_object(irq_i.get());

  timers_[0].irq = std::move(irq_a);
  timers_[1].irq = std::move(irq_b);
  timers_[2].irq = std::move(irq_c);
  timers_[3].irq = std::move(irq_d);
  // No IRQ on timer id 4.
  timers_[5].irq = std::move(irq_f);
  timers_[6].irq = std::move(irq_g);
  timers_[7].irq = std::move(irq_h);
  timers_[8].irq = std::move(irq_i);

  timers_[0].irq_handler.Begin(dispatcher_);
  timers_[1].irq_handler.Begin(dispatcher_);
  timers_[2].irq_handler.Begin(dispatcher_);
  timers_[3].irq_handler.Begin(dispatcher_);
  // No IRQ on timer id 4.
  timers_[5].irq_handler.Begin(dispatcher_);
  timers_[6].irq_handler.Begin(dispatcher_);
  timers_[7].irq_handler.Begin(dispatcher_);
  timers_[8].irq_handler.Begin(dispatcher_);
}

// This method runs on the same dispatcher as all FIDL methods like Start() and Stop().
void AmlHrtimerServer::Timer::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq_base,
                                        zx_status_t status,
                                        const zx_packet_interrupt_t* interrupt) {
  parent.IrqEntries().Add(1);
  auto on_exit = fit::defer([this]() {
    if (irq.is_valid()) {
      zx_status_t status = irq.ack();
      if (status != ZX_OK) {
        fdf::error("IRQ timer id: {} IRQ error: {}", properties.id, zx_status_get_string(status));
      }
    } else {
      fdf::error("IRQ timer id: {} invalid IRQ", properties.id);
    }
    parent.IrqExits().Add(1);
  });

  if (status != ZX_OK) {
    fdf::error("IRQ timer id: {} triggered with error: {}", properties.id,
               zx_status_get_string(status));
    return;
  }

  if (!parent.IsTimerStarted(properties.id)) {
    fdf::info("IRQ timer id: {} IRQ with stopped timer", properties.id);
    return;
  }

  // Timer extends max ticks and its ticks requires re-trigger.
  if (properties.extend_max_ticks && start_ticks_left > std::numeric_limits<uint16_t>::max()) {
    // Log re-triggering since it may wakeup the system.
    start_ticks_left -= std::numeric_limits<uint16_t>::max();
    fdf::debug("Timer id: {} IRQ re-trigger, new start ticks left: {}", properties.id,
               start_ticks_left);
    parent.RecordEvent(zx::clock::get_monotonic().get(), properties.id, EventType::RetriggerIrq,
                       start_ticks_left);
    size_t timer_index = TimerIndexFromId(properties.id);
    auto start_result = parent.StartHardware(timer_index);
    if (start_result.is_error()) {
      fdf::error("Could not restart the hardware for timer id: {}", properties.id);
      std::visit(
          overloaded{[&](StartAndWaitCompleter::Async& completer) {
                       fdf::error("Could not restart the hardware for timer id: {}", properties.id);
                       completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
                     },
                     [&](StartAndWait2Completer::Async& completer) {
                       fdf::error("Could not restart the hardware for timer id: {}", properties.id);
                       completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
                     },
                     [](std::monostate& empty) {}},
          power_enabled_wait_completer);
      power_enabled_wait_completer = std::monostate{};
    }
    return;
  }
  ZX_ASSERT(last_ticks == start_ticks_left);
  // If we have a wait, before we ack the IRQ we take a lease to prevent the system
  // from suspending while we notify any clients. The lease is passed to the completer or
  // dropped as we exit its scope which guarantees the waiting client was notified before the
  // system suspends. We don't exit on error conditions since we need to potentially signal an
  // event and ack the IRQ regardless.
  std::visit(
      overloaded{[&](StartAndWaitCompleter::Async& completer) {
                   fdf::debug("Timer id: {} IRQ w/wait triggered, last ticks: {}", properties.id,
                              last_ticks);
                   parent.RecordEvent(zx::clock::get_monotonic().get(), properties.id,
                                      EventType::TriggerIrqWait, last_ticks);
                   parent.lease_requests_.Add(1);
                   auto wake_lease = (*parent.sag_)->AcquireWakeLease(std::string("aml-hrtimer"));
                   parent.lease_replies_.Add(1);
                   if (wake_lease.is_error()) {
                     completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
                   } else {
                     completer.Reply(zx::ok(fuchsia_hardware_hrtimer::DeviceStartAndWaitResponse{
                         {.keep_alive = std::move(wake_lease.value().token())}}));
                   }
                 },
                 [&](StartAndWait2Completer::Async& completer) {
                   fdf::debug("Timer id: {} IRQ w/wait2 triggered, last ticks: {}", properties.id,
                              last_ticks);
                   parent.RecordEvent(zx::clock::get_monotonic().get(), properties.id,
                                      EventType::TriggerIrqWait2, last_ticks);
                   parent.lease_requests_.Add(1);
                   auto wake_lease = (*parent.sag_)->AcquireWakeLease(std::string("aml-hrtimer"));
                   parent.lease_replies_.Add(1);
                   if (wake_lease.is_error()) {
                     completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
                   } else {
                     completer.Reply(zx::ok(fuchsia_hardware_hrtimer::DeviceStartAndWait2Response{
                         {.expiration_keep_alive = std::move(wake_lease.value().token())}}));
                   }
                 },
                 [&](std::monostate& empty) {
                   fdf::debug("Timer id: {} IRQ triggered, last ticks: {}", properties.id,
                              last_ticks);
                   parent.RecordEvent(zx::clock::get_monotonic().get(), properties.id,
                                      EventType::TriggerIrq, last_ticks);
                 }},
      power_enabled_wait_completer);
  power_enabled_wait_completer = std::monostate();

  if (event) {
    event->signal(0, ZX_EVENT_SIGNALED);
  }
}

AmlHrtimerServer::Timer::Timer(AmlHrtimerServer& server, TimersProperties& props)
    : parent(server), properties(props) {}

void AmlHrtimerServer::ShutDown() {
  for (auto& i : timers_properties_) {
    size_t timer_index = TimerIndexFromId(i.id);
    if (timers_[timer_index].irq.is_valid()) {
      // TODO(https://fxbug.dev/374733154): Make sure no unacked IRQs remain.
      zx_status_t status = timers_[timer_index].irq_handler.Cancel();
      if (status != ZX_OK) {
        fdf::warn("Canceling IRQ for timer id: {} failed: {}", i.id, zx_status_get_string(status));
      }
    }
    std::visit(
        overloaded{[](StartAndWaitCompleter::Async& completer) {
                     completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kCanceled));
                   },
                   [](StartAndWait2Completer::Async& completer) {
                     completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kCanceled));
                   },
                   [](std::monostate& empty) {}},
        timers_[timer_index].power_enabled_wait_completer);
    timers_[timer_index].power_enabled_wait_completer = std::monostate();
  }
}

void AmlHrtimerServer::GetTicksLeft(GetTicksLeftRequest& request,
                                    GetTicksLeftCompleter::Sync& completer) {
  size_t timer_index = TimerIndexFromId(request.id());
  if (timer_index >= kNumberOfTimers) {
    fdf::error("Invalid timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
    return;
  }
  uint64_t ticks = 0;
  switch (request.id()) {
    case 0:
      ticks = IsaTimerA::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 1:
      ticks = IsaTimerB::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 2:
      ticks = IsaTimerC::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 3:
      ticks = IsaTimerD::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 4:
      // We either have not started the timer, or we stopped it.
      if (timers_[timer_index].start_ticks_left == 0) {
        ticks = 0;
      } else {
        // Must read lower 32 bits first.
        ticks = IsaTimerE::Get().ReadFrom(&*mmio_).current_count_value();
        ticks += static_cast<uint64_t>(IsaTimerEHi::Get().ReadFrom(&*mmio_).current_count_value())
                 << 32;
        if (timers_[timer_index].start_ticks_left > ticks) {
          ticks = timers_[timer_index].start_ticks_left - ticks;
        } else {
          ticks = 0;
        }
      }
      break;
    case 5:
      ticks = IsaTimerF::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 6:
      ticks = IsaTimerG::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 7:
      ticks = IsaTimerH::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    case 8:
      ticks = IsaTimerI::Get().ReadFrom(&*mmio_).current_count_value();
      break;
    default:
      completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
      return;
  }

  if (timers_properties_[timer_index].extend_max_ticks) {
    if (timers_[timer_index].start_ticks_left > std::numeric_limits<uint16_t>::max()) {
      ticks += timers_[timer_index].start_ticks_left - std::numeric_limits<uint16_t>::max();
    }
  }
  completer.Reply(zx::ok(ticks));
}

bool AmlHrtimerServer::IsTimerStarted(size_t id) {
  switch (id) {
    case 0:
      return IsaTimerMux::Get().ReadFrom(&*mmio_).TIMERA_EN();
    case 1:
      return IsaTimerMux::Get().ReadFrom(&*mmio_).TIMERB_EN();
    case 2:
      return IsaTimerMux::Get().ReadFrom(&*mmio_).TIMERC_EN();
    case 3:
      return IsaTimerMux::Get().ReadFrom(&*mmio_).TIMERD_EN();
    case 5:
      return IsaTimerMux1::Get().ReadFrom(&*mmio_).TIMERF_EN();
    case 6:
      return IsaTimerMux1::Get().ReadFrom(&*mmio_).TIMERG_EN();
    case 7:
      return IsaTimerMux1::Get().ReadFrom(&*mmio_).TIMERH_EN();
    case 8:
      return IsaTimerMux1::Get().ReadFrom(&*mmio_).TIMERI_EN();
    default:
      fdf::error("Invalid timer id: {}", id);
      return false;
  }
}

void AmlHrtimerServer::ReadTimer(ReadTimerRequest& request, ReadTimerCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_hardware_hrtimer::DriverError::kNotSupported));
}

void AmlHrtimerServer::ReadClock(ReadClockRequest& request, ReadClockCompleter::Sync& completer) {
  completer.Reply(fit::error(fuchsia_hardware_hrtimer::DriverError::kNotSupported));
}

void AmlHrtimerServer::Stop(StopRequest& request, StopCompleter::Sync& completer) {
  size_t timer_index = TimerIndexFromId(request.id());
  if (timer_index >= kNumberOfTimers) {
    fdf::error("Invalid timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
    return;
  }
  switch (request.id()) {
    case 0:
      IsaTimerMux::Get().ReadFrom(&*mmio_).set_TIMERA_EN(false).WriteTo(&*mmio_);
      break;
    case 1:
      IsaTimerMux::Get().ReadFrom(&*mmio_).set_TIMERB_EN(false).WriteTo(&*mmio_);
      break;
    case 2:
      IsaTimerMux::Get().ReadFrom(&*mmio_).set_TIMERC_EN(false).WriteTo(&*mmio_);
      break;
    case 3:
      IsaTimerMux::Get().ReadFrom(&*mmio_).set_TIMERD_EN(false).WriteTo(&*mmio_);
      break;
    case 4:
      // Since there is no way to stop the ticking in the hardware we emulate a stop by clearing
      // the ticks requested in start.
      timers_[timer_index].start_ticks_left = 0;
      break;
    case 5:
      IsaTimerMux1::Get().ReadFrom(&*mmio_).set_TIMERF_EN(false).WriteTo(&*mmio_);
      break;
    case 6:
      IsaTimerMux1::Get().ReadFrom(&*mmio_).set_TIMERG_EN(false).WriteTo(&*mmio_);
      break;
    case 7:
      IsaTimerMux1::Get().ReadFrom(&*mmio_).set_TIMERH_EN(false).WriteTo(&*mmio_);
      break;
    case 8:
      IsaTimerMux1::Get().ReadFrom(&*mmio_).set_TIMERI_EN(false).WriteTo(&*mmio_);
      break;
    default:
      fdf::error("Invalid internal stop timer id: {}", request.id());
      completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInternalError));
      return;
  }
  std::visit(
      overloaded{
          [&](StartAndWaitCompleter::Async& completer) {
            fdf::debug("Received Stop canceling wait for timer id: {}", request.id());
            RecordEvent(zx::clock::get_monotonic().get(), request.id(), EventType::StopWait, 0);
            completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kCanceled));
          },
          [&](StartAndWait2Completer::Async& completer) {
            fdf::debug("Received Stop canceling wait2 for timer id: {}", request.id());
            RecordEvent(zx::clock::get_monotonic().get(), request.id(), EventType::StopWait2, 0);
            completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kCanceled));
          },
          [&](std::monostate& empty) {
            fdf::debug("Received Stop canceling timer id: {}", request.id());
            RecordEvent(zx::clock::get_monotonic().get(), request.id(), EventType::Stop, 0);
          }},
      timers_[timer_index].power_enabled_wait_completer);
  timers_[timer_index].power_enabled_wait_completer = std::monostate();
  completer.Reply(zx::ok());
}
void AmlHrtimerServer::RecordEvent(int64_t now, uint64_t id, EventType type, uint64_t data) {
  events_[event_index_].timestamp = now;
  events_[event_index_].id = id;
  events_[event_index_].type = type;
  events_[event_index_].data = data;
  if (++event_index_ >= kMaxInspectEvents) {
    event_index_ = 0;
  }
}

void AmlHrtimerServer::Start(StartRequest& request, StartCompleter::Sync& completer) {
  completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kNotSupported));
}

void AmlHrtimerServer::StartAndWait(StartAndWaitRequest& request,
                                    StartAndWaitCompleter::Sync& completer) {
  fdf::debug("Timer id: {} start and wait, requested ticks: {}", request.id(), request.ticks());
  RecordEvent(zx::clock::get_monotonic().get(), request.id(), EventType::StartAndWait,
              request.ticks());
  size_t timer_index = TimerIndexFromId(request.id());
  if (timer_index >= kNumberOfTimers) {
    fdf::error("Invalid timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
    return;
  }
  // Fail power enabled StartAndWait if power management is not functional.
  if (!sag_) {
    fdf::error("Power management not functional. StartAndWait failed for timer id: {}",
               request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
    return;
  }
  if (!timers_properties_[timer_index].supports_notifications) {
    fdf::error("Notifications not supported for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kNotSupported));
    return;
  }
  if (!timers_[timer_index].irq.is_valid()) {
    fdf::error("Invalid IRQ for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInternalError));
    return;
  }
  if (!std::holds_alternative<std::monostate>(timers_[timer_index].power_enabled_wait_completer)) {
    fdf::error("Invalid state for wait, already waiting for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
    return;
  }
  timers_[timer_index].start_ticks_left = request.ticks();
  if (!request.resolution().duration()) {
    fdf::error("Invalid resolution, no duration for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
    return;
  }
  timers_[timer_index].resolution_nsecs = request.resolution().duration().value();
  auto start_result = StartHardware(timer_index);
  if (start_result.is_error()) {
    completer.Reply(zx::error(start_result.error_value()));
    return;
  }
  if (request.setup_event().is_valid()) {
    request.setup_event().signal(0, ZX_EVENT_SIGNALED);
  } else {
    fdf::warn("Invalid setup_event for timer id: {}", request.id());
  }
  timers_[timer_index].power_enabled_wait_completer = completer.ToAsync();
}

void AmlHrtimerServer::StartAndWait2(StartAndWait2Request& request,
                                     StartAndWait2Completer::Sync& completer) {
  fdf::debug("Timer id: {} start and wait2, requested ticks: {}", request.id(), request.ticks());
  RecordEvent(zx::clock::get_monotonic().get(), request.id(), EventType::StartAndWait2,
              request.ticks());
  size_t timer_index = TimerIndexFromId(request.id());
  if (timer_index >= kNumberOfTimers) {
    fdf::error("Invalid timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
    return;
  }
  // Fail power enabled StartAndWait2 if power management is not functional.
  if (!sag_) {
    fdf::error("Power management not functional. StartAndWait2 failed for timer id: {}",
               request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
    return;
  }
  if (!timers_properties_[timer_index].supports_notifications) {
    fdf::error("Notifications not supported for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kNotSupported));
    return;
  }
  if (!timers_[timer_index].irq.is_valid()) {
    fdf::error("Invalid IRQ for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInternalError));
    return;
  }
  if (!std::holds_alternative<std::monostate>(timers_[timer_index].power_enabled_wait_completer)) {
    fdf::error("Invalid state for wait, already waiting for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kBadState));
    return;
  }
  timers_[timer_index].start_ticks_left = request.ticks();
  if (!request.resolution().duration()) {
    fdf::error("Invalid resolution, no duration for timer id: {}", request.id());
    completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs));
    return;
  }
  timers_[timer_index].resolution_nsecs = request.resolution().duration().value();
  auto start_result = StartHardware(timer_index);
  if (start_result.is_error()) {
    completer.Reply(zx::error(start_result.error_value()));
    return;
  }
  timers_[timer_index].power_enabled_wait_completer = completer.ToAsync();
  {
    // request.setup_keep_alive() is dropped here since the timer has been setup.
    auto to_drop = std::move(request.setup_keep_alive());
  }
}

fit::result<const fuchsia_hardware_hrtimer::DriverError> AmlHrtimerServer::StartHardware(
    size_t timer_index) {
  const uint64_t start_ticks = timers_[timer_index].start_ticks_left;
  uint32_t current_ticks = 0;
  uint64_t id = timers_properties_[timer_index].id;
  switch (timers_properties_[timer_index].max_ticks_support) {
    case MaxTicks::k16Bit:
      if (timers_properties_[timer_index].extend_max_ticks &&
          start_ticks > std::numeric_limits<uint16_t>::max()) {
        current_ticks = std::numeric_limits<uint16_t>::max();
      } else {
        if (start_ticks > std::numeric_limits<uint16_t>::max()) {
          fdf::error("Invalid ticks range: {} for timer id: {}", start_ticks, id);
          return fit::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs);
        }
        current_ticks = static_cast<uint32_t>(start_ticks);
      }
      break;
    case MaxTicks::k64Bit:
      break;
  }
  uint32_t input_clock_selection = 0;
  const uint64_t resolution_nsecs = timers_[timer_index].resolution_nsecs;
  if (timers_properties_[timer_index].supports_1usec &&
      timers_properties_[timer_index].supports_10usecs &&
      timers_properties_[timer_index].supports_100usecs &&
      timers_properties_[timer_index].supports_1msec) {
    switch (resolution_nsecs) {
        // clang-format off
      case zx::usec(1).to_nsecs():   input_clock_selection = 0; break;
      case zx::usec(10).to_nsecs():  input_clock_selection = 1; break;
      case zx::usec(100).to_nsecs(): input_clock_selection = 2; break;
      case zx::msec(1).to_nsecs():   input_clock_selection = 3; break;
        // clang-format on
      default:
        fdf::error("Invalid resolution: {} nsecs for timer id: {}", resolution_nsecs, id);
        return fit::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs);
    }
    fdf::trace("Timer id: {} resolution: {} nsecs", id, resolution_nsecs);
  } else if (timers_properties_[timer_index].supports_1usec &&
             timers_properties_[timer_index].supports_10usecs &&
             timers_properties_[timer_index].supports_100usecs) {
    switch (resolution_nsecs) {
        // clang-format off
          case zx::usec(1).to_nsecs():   input_clock_selection = 1; break;
          case zx::usec(10).to_nsecs():  input_clock_selection = 2; break;
          case zx::usec(100).to_nsecs(): input_clock_selection = 3; break;
        // clang-format on
      default:
        fdf::error("Invalid resolution: {} nsecs for timer id: {}", resolution_nsecs, id);
        return fit::error(fuchsia_hardware_hrtimer::DriverError::kInvalidArgs);
    }
    fdf::trace("Timer id: {} resolution: {} nsecs", id, resolution_nsecs);
  } else {
    fdf::error("Invalid resolution state, unsupported combination for timer id: {}", id);
    return fit::error(fuchsia_hardware_hrtimer::DriverError::kInternalError);
  }

  switch (timer_index) {
    case 0:
      IsaTimerA::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERA_EN(true)
          .set_TIMERA_MODE(false)
          .set_TIMERA_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 1:
      IsaTimerB::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERB_EN(true)
          .set_TIMERB_MODE(false)
          .set_TIMERB_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 2:
      IsaTimerC::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERC_EN(true)
          .set_TIMERC_MODE(false)
          .set_TIMERC_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 3:
      IsaTimerD::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERD_EN(true)
          .set_TIMERD_MODE(false)
          .set_TIMERD_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 4:
      IsaTimerMux::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERE_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      IsaTimerE::Get().ReadFrom(&*mmio_).set_current_count_value(0).WriteTo(&*mmio_);
      break;
    case 5:
      IsaTimerF::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux1::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERF_EN(true)
          .set_TIMERF_MODE(false)
          .set_TIMERF_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 6:
      IsaTimerG::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux1::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERG_EN(true)
          .set_TIMERG_MODE(false)
          .set_TIMERG_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 7:
      IsaTimerH::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux1::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERH_EN(true)
          .set_TIMERH_MODE(false)
          .set_TIMERH_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    case 8:
      IsaTimerI::Get().ReadFrom(&*mmio_).set_starting_count_value(current_ticks).WriteTo(&*mmio_);
      IsaTimerMux1::Get()
          .ReadFrom(&*mmio_)
          .set_TIMERI_EN(true)
          .set_TIMERI_MODE(false)
          .set_TIMERI_input_clock_selection(input_clock_selection)
          .WriteTo(&*mmio_);
      break;
    default:
      fdf::error("Invalid internal state for timer id: {}", id);
      return fit::error(fuchsia_hardware_hrtimer::DriverError::kInternalError);
  }
  timers_[timer_index].last_ticks = current_ticks;
  fdf::debug("Timer id: {} started, start ticks left: {} last ticks: {}", id,
             timers_[timer_index].start_ticks_left, timers_[timer_index].last_ticks);
  RecordEvent(zx::clock::get_monotonic().get(), id, EventType::StartHardware,
              timers_[timer_index].last_ticks);
  return fit::success();
}

void AmlHrtimerServer::SetEvent(SetEventRequest& request, SetEventCompleter::Sync& completer) {
  completer.Reply(zx::error(fuchsia_hardware_hrtimer::DriverError::kNotSupported));
}

void AmlHrtimerServer::GetProperties(GetPropertiesCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_hrtimer::TimerProperties> timers_properties;
  for (auto& i : timers_properties_) {
    fuchsia_hardware_hrtimer::TimerProperties timer_properties;
    timer_properties.id(i.id);

    std::vector<fuchsia_hardware_hrtimer::Resolution> resolutions;
    if (i.supports_1usec) {
      resolutions.emplace_back(
          fuchsia_hardware_hrtimer::Resolution::WithDuration(zx::usec(1).to_nsecs()));
    }
    if (i.supports_10usecs) {
      resolutions.emplace_back(
          fuchsia_hardware_hrtimer::Resolution::WithDuration(zx::usec(10).to_nsecs()));
    }
    if (i.supports_100usecs) {
      resolutions.emplace_back(
          fuchsia_hardware_hrtimer::Resolution::WithDuration(zx::usec(100).to_nsecs()));
    }
    if (i.supports_1msec) {
      resolutions.emplace_back(
          fuchsia_hardware_hrtimer::Resolution::WithDuration(zx::msec(1).to_nsecs()));
    }
    timer_properties.supported_resolutions(std::move(resolutions));
    switch (i.max_ticks_support) {
      case MaxTicks::k16Bit:
        if (i.extend_max_ticks) {
          timer_properties.max_ticks(std::numeric_limits<uint64_t>::max());
        } else {
          timer_properties.max_ticks(std::numeric_limits<uint16_t>::max());
        }
        break;
      case MaxTicks::k64Bit:
        timer_properties.max_ticks(std::numeric_limits<uint64_t>::max());
        break;
    }
    timer_properties.supports_event(false);
    // Only support wait if we can return a lease in StartAndWait.
    timer_properties.supports_wait(sag_ && i.supports_notifications);
    timer_properties.supports_read(false);
    timers_properties.emplace_back(std::move(timer_properties));
  }

  fuchsia_hardware_hrtimer::Properties properties = {};
  properties.timers_properties(std::move(timers_properties));
  completer.Reply(std::move(properties));
}

void AmlHrtimerServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_hrtimer::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

}  // namespace hrtimer
