// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-sink-policy.h"

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstdint>
#include <optional>
#include <utility>

#include "src/devices/power/drivers/fusb302/usb-pd-message-objects.h"

namespace usb_pd {

bool SinkPolicyInfo::IsValid() const {
  if (min_voltage_mv <= 0) {
    return false;
  }
  if (max_voltage_mv < min_voltage_mv) {
    return false;
  }

  if (max_power_mw <= 0) {
    return false;
  }
  return true;
}

SinkPolicy::SinkPolicy(const SinkPolicyInfo& policy_info) : policy_info_(policy_info) {
  ZX_DEBUG_ASSERT(policy_info.IsValid());

  PopulateSinkCapabilities();
}

SinkPolicy::~SinkPolicy() = default;

void SinkPolicy::DidReceiveSourceCapabilities(const Message& capabilities) {
  // clear() doesn't currently work for elements without a default constructor.
  source_capabilities_ = {};

  for (uint32_t capability : capabilities.data_objects()) {
    source_capabilities_.push_back(PowerData(capability));
  }
  LogSourcePowerCapabilities();
}

namespace {

// `supply_data` must be the first PDO in a USB PD source capabilities list.
void LogSourceAttributes(FixedPowerSupplyData supply_data) {
  fdf::info(
      "Source Attributes: USB suspend {}, "
      "unconstrained power: %s, dual role power: %s, power range: %s, "
      "usb communication: %s, dual role data: %s",
      supply_data.requires_usb_suspend() ? "required" : "not required",
      supply_data.has_unconstrained_power() ? "yes" : "no",
      supply_data.supports_dual_role_power() ? "yes" : "no",
      supply_data.supports_extended_power_range() ? "extended" : "standard",
      supply_data.supports_usb_communications() ? "yes" : "no",
      supply_data.supports_dual_role_data() ? "yes" : "no");
}

void LogSourcePowerCapability(PowerData power_data, int power_data_object_index) {
  switch (power_data.supply_type()) {
    case PowerSupplyType::kFixedSupply: {
      FixedPowerSupplyData fixed_supply(power_data);
      fdf::info("Source Capability #{}: Fixed Power - {} mV @ {} mA", power_data_object_index,
                fixed_supply.voltage_mv(), fixed_supply.maximum_current_ma());
      break;
    }

    case PowerSupplyType::kBattery: {
      BatteryPowerSupplyData battery_supply(power_data);
      fdf::info("Source Capability #{}: Battery - [{} - {}] mV, {} mW", power_data_object_index,
                battery_supply.minimum_voltage_mv(), battery_supply.maximum_voltage_mv(),
                battery_supply.maximum_power_mw());
      break;
    }

    case PowerSupplyType::kVariableSupply: {
      VariablePowerSupplyData variable_supply(power_data);
      fdf::info("Source Capability #{}: Variable Power - [{} - {}] mV @ {} mA",
                power_data_object_index, variable_supply.minimum_voltage_mv(),
                variable_supply.maximum_voltage_mv(), variable_supply.maximum_current_ma());
      break;
    }

    case PowerSupplyType::kAugmentedPowerDataObject:
      fdf::info("Source Capability #{}: Augmented PDO (unsupported) - {:#08x}",
                power_data_object_index, power_data.bits);
      break;
  }
}

void LogSinkPowerRequestData(PowerRequestData request_data,
                             cpp20::span<const PowerData> source_capabilities) {
  // Casting will not overflow because the position is a 4-bit integer.
  int related_source_pdo_position =
      static_cast<int>(request_data.related_power_data_object_position());

  fdf::info(
      "Power Request Attributes: source PDO #{}, power give back: {}, capability mismatch: {}, "
      "usb communication: %s, usb suspend: %s, PHY message support: %s, power range: %s",
      related_source_pdo_position,
      request_data.supports_power_give_back() ? "supported" : "not supported",
      request_data.capability_mismatch() ? "yes" : "no",
      request_data.supports_usb_communications() ? "yes" : "no",
      request_data.prefers_waiving_usb_suspend() ? "waive asked" : "ok",
      request_data.supports_unchunked_extended_messages() ? "extended" : "standard",
      request_data.supports_extended_power_range() ? "extended" : "standard");

  if (related_source_pdo_position == 0) {
    fdf::error("Non-standard Power Request: using reserved related source PDO value #0");
    return;
  }

  // Casting will not overflow because the position is a positive 4-bit integer.
  if (static_cast<size_t>(related_source_pdo_position) > source_capabilities.size()) {
    fdf::error("Invalid Power Request: related source PDO value #{} exceeds PDO list size {}",
               related_source_pdo_position, source_capabilities.size());
    return;
  }

  const PowerData source_power_data = source_capabilities[related_source_pdo_position - 1];
  switch (source_power_data.supply_type()) {
    case PowerSupplyType::kFixedSupply: {
      FixedVariableSupplyPowerRequestData fixed_request(request_data);
      FixedPowerSupplyData supply_data(source_power_data);

      int64_t operating_power_mw =
          (int64_t{supply_data.voltage_mv()} * fixed_request.operating_current_ma()) / 1'000;
      int64_t limit_power_mw =
          (int64_t{supply_data.voltage_mv()} * fixed_request.limit_current_ma()) / 1'000;
      fdf::info(
          "Fixed Power Request: Operating {} mV @ {} mA => {} mW, "
          "Limit {} mV @ {} mA => {} mW",
          supply_data.voltage_mv(), fixed_request.operating_current_ma(), operating_power_mw,
          supply_data.voltage_mv(), fixed_request.limit_current_ma(), limit_power_mw);
      break;
    }

    case PowerSupplyType::kVariableSupply: {
      FixedVariableSupplyPowerRequestData fixed_request(request_data);
      fdf::info("Variable Power Request: Operating {} mA, Limit {} mA",
                fixed_request.operating_current_ma(), fixed_request.limit_current_ma());
      break;
    }

    case PowerSupplyType::kBattery: {
      BatteryPowerRequestData battery_request(request_data);
      fdf::info("Battery Power Request: Operating {} mW, Limit {} mW",
                battery_request.operating_power_mw(), battery_request.limit_power_mw());

      break;
    }

    case PowerSupplyType::kAugmentedPowerDataObject:
      fdf::info("Augmented Power Request: Augmented RDO (unsupported) - {:#08x}",
                static_cast<uint32_t>(request_data));
      break;
  }
}

}  // namespace

void SinkPolicy::LogSourcePowerCapabilities() {
  fdf::info("Received USB PD source capabilities");
  // The cast does not overflow (causing UB) because the maximum vector size is
  // `Header::kMaxDataObjectCount`.
  const int32_t capabilities_count = static_cast<int32_t>(source_capabilities_.size());

  if (source_capabilities_.empty()) {
    fdf::error("USB PD SourceCapabilities message has no capabilities (empty PDO list)!");
    return;
  }

  const PowerData first_power_data = source_capabilities_.front();
  if (first_power_data.supply_type() == PowerSupplyType::kFixedSupply) {
    LogSourceAttributes(FixedPowerSupplyData(first_power_data));
  } else {
    fdf::warn("Non-standard USB PD source! First capability must be Fixed Power");
  }

  for (int i = 0; i < capabilities_count; ++i) {
    LogSourcePowerCapability(source_capabilities_[i], i + 1);
  }
}

// A score for how well a power source offering suits our sink's needs.
struct SinkPolicy::PowerDataSuitability {
  int32_t power_mw = 0;
  int32_t voltage_mv = 0;

  bool operator<(const PowerDataSuitability& rhs) const {
    if (power_mw != rhs.power_mw) {
      return power_mw < rhs.power_mw;
    }
    // Prefer the lowest voltage that hits a power goal.
    return voltage_mv > rhs.voltage_mv;
  }
};

PowerRequestData SinkPolicy::GetPowerRequest() const {
  ZX_DEBUG_ASSERT_MSG(source_capabilities_.size() != 0,
                      "DidReceiveSourceCapabilities() not called");

  PowerDataSuitability best_suitability;
  std::optional<PowerRequestData> best_request_data;

  // The cast does not overflow (causing UB) because the maximum vector size is
  // `Header::kMaxDataObjectCount`.
  const int32_t capabilities_count = static_cast<int32_t>(source_capabilities_.size());
  for (int32_t i = 0; i < capabilities_count; ++i) {
    const int32_t position = i + 1;
    const PowerData power_data = source_capabilities_[i];

    switch (power_data.supply_type()) {
      case PowerSupplyType::kFixedSupply: {
        auto [request_data, suitability] =
            ScoreFixedPower(FixedPowerSupplyData(power_data), position);
        if (best_suitability < suitability) {
          best_suitability = suitability;
          best_request_data = request_data;
        }
        break;
      }
      default:
        fdf::debug("Skipping unsupported power type {}",
                   static_cast<uint8_t>(power_data.supply_type()));
        break;
    }
  }

  // We call value() unconditionally on the std::optional, because usbpd3.1
  // guarantees that we'll find at least one suitable source.
  PowerRequestData final_result = SetCommonRequestFields(best_request_data.value());
  LogSinkPowerRequestData(final_result, source_capabilities_);
  return final_result;
}

cpp20::span<const uint32_t> SinkPolicy::GetSinkCapabilities() const { return sink_capabilities_; }

void SinkPolicy::PopulateSinkCapabilities() {
  ZX_DEBUG_ASSERT_MSG(sink_capabilities_.empty(), "PopulateSinkCapabilities() already called");

  // Fixed Supply PDOs must come first in a Capabilities message.
  static constexpr int32_t kVoltagesMv[] = {5'000, 9'000, 10'1000, 12'000, 15'000, 20'000};
  for (int32_t voltage_mv : kVoltagesMv) {
    if (voltage_mv < policy_info_.min_voltage_mv || voltage_mv > policy_info_.max_voltage_mv) {
      continue;
    }

    const int32_t max_power_uw = policy_info_.max_power_mw * 1'000;

    // Using ceiling division to get an upper bound on the power consumption.
    const int32_t current_ma = (max_power_uw + voltage_mv - 1) / voltage_mv;

    SinkFixedPowerSupplyData fixed_supply;
    fixed_supply.set_voltage_mv(voltage_mv).set_maximum_current_ma(current_ma);
    sink_capabilities_.push_back(static_cast<uint32_t>(fixed_supply));
  }

  // The first PDO must be a Fixed Supply PDO with vSafe5V.
  ZX_DEBUG_ASSERT(!sink_capabilities_.empty());
  const PowerData first_power_data(sink_capabilities_[0]);
  const SinkFixedPowerSupplyData first_fixed_supply(first_power_data);
  ZX_DEBUG_ASSERT(first_fixed_supply.voltage_mv() == 5'000);

  sink_capabilities_[0] = static_cast<uint32_t>(SetAdditionalInformation(first_fixed_supply));
}

std::pair<PowerRequestData, SinkPolicy::PowerDataSuitability> SinkPolicy::ScoreFixedPower(
    FixedPowerSupplyData power_data, int32_t data_position) const {
  auto request_data = FixedVariableSupplyPowerRequestData::CreateForPosition(data_position);
  int32_t voltage_mv = power_data.voltage_mv();
  if (voltage_mv < policy_info_.min_voltage_mv || voltage_mv > policy_info_.max_voltage_mv) {
    return {request_data, {}};
  }

  const int32_t max_power_uw = policy_info_.max_power_mw * 1'000;

  // We use ceiling division because the requested current must be an upper
  // bound for the actual consumption.
  int32_t requested_current_ma =
      std::min((max_power_uw + voltage_mv - 1) / voltage_mv, power_data.maximum_current_ma());

  request_data.set_limit_current_ma(requested_current_ma)
      .set_operating_current_ma(requested_current_ma);

  // The multiplication does not overflow (causing UB) because the result is at
  // most 523,264,500 uV. Due to the USB PD format, `voltage_mv` is at most
  // 51,150 mV, and `requested_current_ma` is at most 10,230 mA.
  const int32_t power_uw = (requested_current_ma * voltage_mv);

  // The requested power may be slightly higher than the maximum consumption,
  // due to the ceiling division above. We don't want this rounding error to
  // favor a higher voltage over a lower voltage, so we fix it up here.
  const int32_t power_mw = std::min(power_uw / 1'000, policy_info_.max_power_mw);

  PowerDataSuitability suitability = {.power_mw = power_mw, .voltage_mv = voltage_mv};
  return {request_data, suitability};
}

SinkFixedPowerSupplyData SinkPolicy::SetAdditionalInformation(
    SinkFixedPowerSupplyData fixed_power) const {
  fixed_power.set_supports_dual_role_power(false)
      .set_requires_more_than_5v(false)
      .set_supports_usb_communications(policy_info_.supports_usb_communications)
      .set_supports_dual_role_data(false)
      .set_fast_swap_current_requirement(FastSwapCurrentRequirement::kNotSupported);
  return fixed_power;
}

PowerRequestData SinkPolicy::SetCommonRequestFields(PowerRequestData request) const {
  request.set_supports_power_give_back(false)
      .set_prefers_waiving_usb_suspend(true)
      .set_supports_usb_communications(policy_info_.supports_usb_communications)
      .set_supports_unchunked_extended_messages(false)
      .set_supports_extended_power_range(false);
  return request;
}

}  // namespace usb_pd
