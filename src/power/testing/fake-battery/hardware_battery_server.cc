// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hardware_battery_server.h"

#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/logging/cpp/logger.h>

#include <algorithm>

namespace fake_battery {

HardwareBatteryServer::HardwareBatteryServer(async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher) {
  battery_spec_.design_capacity_uah(test_hardwarepowercontrol::kDefaultFullCapacityUah);
  fuchsia_hardware_power_battery::Status supported_interest;
  supported_interest.level_percent(0.0f);
  supported_interest.charge_status(fuchsia_hardware_power_battery::ChargeStatus::kCharging);
  supported_interest.health(fuchsia_hardware_power_battery::HealthStatus::kGood);
  fuchsia_hardware_power_battery::WatchOptions supported_options;
  supported_options.interest(std::move(supported_interest));
  battery_spec_.supported_options(std::move(supported_options));

  battery_status_.present(true);
  battery_status_.charge_status(fuchsia_hardware_power_battery::ChargeStatus::kCharging);
  battery_status_.voltage_uv(test_hardwarepowercontrol::kDefaultPresentVoltageMv * 1000);
  battery_status_.current_ua(test_hardwarepowercontrol::kDefaultChargingCurrentUa);
  battery_status_.level_percent(test_hardwarepowercontrol::kDefaultLevelPercent);
  battery_status_.health(fuchsia_hardware_power_battery::HealthStatus::kGood);
  battery_status_.time_remaining(zx::sec(59).to_nsecs());
  battery_status_.remaining_capacity_uah(test_hardwarepowercontrol::kDefaultRemainingChargeUah);
  battery_status_.full_charge_capacity_uah(test_hardwarepowercontrol::kDefaultFullCapacityUah);
  battery_status_.temp_celsius(
      static_cast<float>(test_hardwarepowercontrol::kDefaultTemperatureMc) / 1000.0f);
}

zx_status_t HardwareBatteryServer::Init(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
  auto result = outgoing->AddService<fuchsia_hardware_power_battery::Service>(
      fuchsia_hardware_power_battery::Service::InstanceHandler({
          .battery =
              [this](fidl::ServerEnd<fuchsia_hardware_power_battery::Battery> server) {
                auto conn = std::make_shared<BatteryConnection>(this);
                battery_connections_.push_back(conn);
                fidl::BindServer(dispatcher_, std::move(server), conn,
                                 [this](BatteryConnection* impl, fidl::UnbindInfo,
                                        fidl::ServerEnd<fuchsia_hardware_power_battery::Battery>) {
                                   std::erase_if(battery_connections_,
                                                 [impl](const auto& c) { return c.get() == impl; });
                                 });
              },
      }));
  if (result.is_error()) {
    return result.status_value();
  }

  auto control_result = outgoing->AddService<test_hardwarepowercontrol::Service>(
      test_hardwarepowercontrol::Service::InstanceHandler({
          .control =
              [this](fidl::ServerEnd<test_hardwarepowercontrol::Control> server) {
                fdf::info("Control connection request received");

                control_bindings_.AddBinding(dispatcher_, std::move(server), this,
                                             fidl::kIgnoreBindingClosure);
              },
      }));
  return control_result.status_value();
}

namespace {

// Every field of `fuchsia.hardware.power.battery/Status`, so the mask operations below stay in
// step with the table instead of each keeping its own hand-written copy.
#define BATTERY_STATUS_FIELDS(X) \
  X(present)                     \
  X(voltage_uv)                  \
  X(current_ua)                  \
  X(level_percent)               \
  X(temp_celsius)                \
  X(charge_status)               \
  X(remaining_capacity_uah) X(full_charge_capacity_uah) X(health) X(cycle_count) X(time_remaining)

// Only field presence is meaningful in a mask; the value each present field holds is ignored.
bool HasAnyField(const fuchsia_hardware_power_battery::Status& mask) {
#define X(field)                \
  if (mask.field().has_value()) \
    return true;
  BATTERY_STATUS_FIELDS(X)
#undef X
  return false;
}

// Fields present in both `requested` and `supported`.
fuchsia_hardware_power_battery::Status Intersect(
    const fuchsia_hardware_power_battery::Status& requested,
    const fuchsia_hardware_power_battery::Status& supported) {
  fuchsia_hardware_power_battery::Status out;
#define X(field)                                                      \
  if (requested.field().has_value() && supported.field().has_value()) \
    out.field(requested.field());
  BATTERY_STATUS_FIELDS(X)
#undef X
  return out;
}

// Adds every field present in `from` to `into` if not already set.
void UnionInto(const fuchsia_hardware_power_battery::Status& from,
               fuchsia_hardware_power_battery::Status& into) {
#define X(field)                                             \
  if (from.field().has_value() && !into.field().has_value()) \
    into.field(from.field());
  BATTERY_STATUS_FIELDS(X)
#undef X
}

// Overwrites every field present in `from` onto `into`, preserving unset fields.
void MergeInto(const fuchsia_hardware_power_battery::Status& from,
               fuchsia_hardware_power_battery::Status& into) {
#define X(field)                \
  if (from.field().has_value()) \
    into.field(from.field());
  BATTERY_STATUS_FIELDS(X)
#undef X
}

#undef BATTERY_STATUS_FIELDS

// The masks this fake advertises in `Spec.supported_options`.
fuchsia_hardware_power_battery::Status SupportedInterest(
    const fuchsia_hardware_power_battery::Spec& spec) {
  if (spec.supported_options().has_value() && spec.supported_options()->interest().has_value()) {
    return *spec.supported_options()->interest();
  }
  return {};
}

fuchsia_hardware_power_battery::Status SupportedWakeOn(
    const fuchsia_hardware_power_battery::Spec& spec) {
  if (spec.supported_options().has_value() && spec.supported_options()->wake_on().has_value()) {
    return *spec.supported_options()->wake_on();
  }
  return {};
}

}  // namespace

void HardwareBatteryServer::SetBatteryStatus(scontrol::SetBatteryStatusRequest& request,
                                             scontrol::SetBatteryStatusCompleter::Sync& completer) {
  fdf::info("SetBatteryStatus called");
  MergeInto(request.status(), battery_status_);
  NotifyAll(request.status());
  completer.Reply();
}

void HardwareBatteryServer::NotifyAll(const fuchsia_hardware_power_battery::Status& status) {
  for (auto& conn : battery_connections_) {
    conn->Notify(status);
  }
}

BatteryConnection::BatteryConnection(HardwareBatteryServer* server) : server_(server) {
  interest_ = SupportedInterest(server_->battery_spec());
}

void BatteryConnection::GetSpec(GetSpecCompleter::Sync& completer) {
  completer.Reply(fit::ok(server_->battery_spec()));
}

void BatteryConnection::GetStatus(GetStatusCompleter::Sync& completer) {
  completer.Reply(fit::ok(server_->battery_status()));
}

void BatteryConnection::ConfigureWatch(ConfigureWatchRequest& request,
                                       ConfigureWatchCompleter::Sync& completer) {
  const fuchsia_hardware_power_battery::Status supported_interest =
      SupportedInterest(server_->battery_spec());
  const fuchsia_hardware_power_battery::Status supported_wake_on =
      SupportedWakeOn(server_->battery_spec());

  const auto& requested_interest = request.options().interest();
  const auto& requested_wake_on = request.options().wake_on();
  const bool had_requested_fields =
      (requested_interest.has_value() && HasAnyField(*requested_interest)) ||
      (requested_wake_on.has_value() && HasAnyField(*requested_wake_on));

  // An absent `interest` means every supported field.
  fuchsia_hardware_power_battery::Status effective_interest =
      requested_interest.has_value() ? *requested_interest : supported_interest;
  // Any field present in `wake_on` is implicitly included in `interest`.
  if (requested_wake_on.has_value()) {
    UnionInto(*requested_wake_on, effective_interest);
  }

  effective_interest = Intersect(effective_interest, supported_interest);
  fuchsia_hardware_power_battery::Status effective_wake_on =
      requested_wake_on.has_value() ? Intersect(*requested_wake_on, supported_wake_on)
                                    : fuchsia_hardware_power_battery::Status{};

  // Non-empty masks were requested but nothing survived filtering. Leave the connection's
  // configuration untouched, matching the FIDL's all-or-nothing contract.
  if (had_requested_fields && !HasAnyField(effective_interest) && !HasAnyField(effective_wake_on)) {
    completer.Reply(fit::error(fuchsia_hardware_power_battery::Error::kNotSupported));
    return;
  }

  interest_ = effective_interest;
  wake_on_ = effective_wake_on;

  // Absent in, absent out: the client is left on the default rather than pinned to today's
  // supported set. An explicitly empty mask is echoed as empty.
  fuchsia_hardware_power_battery::WatchOptions effective_options;
  if (requested_interest.has_value()) {
    effective_options.interest(effective_interest);
  }
  if (requested_wake_on.has_value()) {
    effective_options.wake_on(effective_wake_on);
  }
  completer.Reply(fit::ok(std::move(effective_options)));
}

void BatteryConnection::Watch(WatchRequest& request, WatchCompleter::Sync& completer) {
  if (watch_completer_) {
    completer.Reply(fit::error(fuchsia_hardware_power_battery::Error::kAlreadyWatching));
    return;
  }
  watch_completer_ = completer.ToAsync();

  if (first_watch_ || state_changed_) {
    Notify(server_->battery_status());
    first_watch_ = false;
    state_changed_ = false;
  }
}

void BatteryConnection::Notify(const fuchsia_hardware_power_battery::Status& updated_fields) {
  // The first response on a connection always resolves immediately; after that, an explicit
  // `SetBatteryStatus` test injection notifies watchers whenever any injected field intersects
  // `interest_` or `wake_on_` (including when a fake-clock test re-injects the same battery level
  // after advancing time).
  if (last_status_.has_value() && !HasAnyField(Intersect(updated_fields, interest_)) &&
      !HasAnyField(Intersect(updated_fields, wake_on_))) {
    return;
  }

  if (!watch_completer_) {
    state_changed_ = true;
    return;
  }

  last_status_ = server_->battery_status();
  watch_completer_->Reply(fit::ok(fuchsia_hardware_power_battery::BatteryWatchResponse{
      {.status = server_->battery_status(), .wake_lease = {}}}));
  watch_completer_.reset();
}

void BatteryConnection::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_power_battery::Battery> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method called: ordinal={}", metadata.method_ordinal);
}

}  // namespace fake_battery
