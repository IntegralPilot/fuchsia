// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-trip-device.h"

#include <fidl/fuchsia.hardware.trippoint/cpp/common_types.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/wire_types.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>

#include <algorithm>
#include <cmath>

#include <src/devices/temperature/drivers/aml-trip/util.h>

#include "aml-tsensor-regs.h"

namespace temperature {

void AmlTripDevice::Init() {
  InitSensor();

  irq_handler_.set_object(irq_.get());
  irq_handler_.Begin(dispatcher_);
}

void AmlTripDevice::Shutdown() {
  irq_handler_.Cancel();
  irq_.destroy();
}

void AmlTripDevice::QueueTripResult(TemperatureCelsius measured, uint32_t index) {
  fuchsia_hardware_trippoint::wire::TripPointResult result;

  ZX_ASSERT_MSG(index < kNumTripPoints, "QueueTripResult index out of range, index = %u", index);

  if (!configured_trip_points_[index]) {
    fdf::warn("Trip point cleared before IRQ delivered. index = {}", index);
    return;
  }

  result.measured_temperature_celsius = measured;
  result.index = index;

  pending_trips_.push_back(result);
}

TemperatureCelsius AmlTripDevice::ReadTemperatureCelsius() {
  unsigned int count = 0;
  unsigned int value_all = 0;
  constexpr unsigned int kAmlTsValueCount = 0x10;
  constexpr unsigned int kTvalueLBound = 0x18a9;
  constexpr unsigned int kTvalueUBound = 0x32a6;
  TemperatureCelsius result = 0.0f;

  for (unsigned int i = 0; i < kAmlTsValueCount; i++) {
    auto ts_stat0 = thermal::TsStat0::Get().ReadFrom(&sensor_mmio_);
    auto tvalue = ts_stat0.temperature();

    if ((tvalue >= kTvalueLBound) && (tvalue <= kTvalueUBound)) {
      count++;
      value_all += tvalue;
    }
  }

  if (count != 0) {
    result = tsensor_util::CodeToTempCelsius(value_all / count, trim_info_);
  }

  return result;
}

void AmlTripDevice::GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) {
  TemperatureCelsius temperature = ReadTemperatureCelsius();
  completer.Reply(ZX_OK, temperature);
}

void AmlTripDevice::GetSensorName(GetSensorNameCompleter::Sync& completer) {
  completer.Reply(fidl::StringView::FromExternal(name_));
}

void AmlTripDevice::GetTripPointDescriptors(GetTripPointDescriptorsCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> result(kNumTripPoints);

  for (uint32_t i = 0; i < kNumTripPoints; i++) {
    result[i].index = i;
    result[i].type = GetTripPointType(i);

    if (!configured_trip_points_[i].has_value()) {
      result[i].configuration =
          fuchsia_hardware_trippoint::wire::TripPointValue::WithClearedTripPoint(
              fuchsia_hardware_trippoint::wire::ClearedTripPoint());
      continue;
    }

    switch (configured_trip_points_[i]->direction) {
      case TripPointDirection::Rise:
        result[i].configuration =
            fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(
                fuchsia_hardware_trippoint::wire::OneshotTempAboveTripPoint{
                    .critical_temperature_celsius = configured_trip_points_[i]->temperature});
        break;
      case TripPointDirection::Fall:
        result[i].configuration =
            fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempBelowTripPoint(
                fuchsia_hardware_trippoint::wire::OneshotTempBelowTripPoint{
                    .critical_temperature_celsius = configured_trip_points_[i]->temperature});
        break;
      default:
        ZX_PANIC("Invalid configured trip point type");
    }
  }

  completer.ReplySuccess(
      ::fidl::VectorView<::fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(
          result));
}

void AmlTripDevice::SetTripPoints(SetTripPointsRequestView request,
                                  SetTripPointsCompleter::Sync& completer) {
  if (request->descriptors.empty()) {
    // No work to do.
    completer.ReplySuccess();
    return;
  }

  for (const auto& desc : request->descriptors) {
    if (desc.index >= kNumTripPoints) {
      fdf::error("Trip Point index out of bounds, (max = {}, requested = {})", kNumTripPoints,
                 desc.index);
      completer.ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }

    if (desc.type != GetTripPointType(desc.index)) {
      fdf::error("The provided index does not match the trip point type. index = {}", desc.index);
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }

    if (desc.configuration.is_oneshot_temp_below_trip_point()) {
      if (desc.type != fuchsia_hardware_trippoint::TripPointType::kOneshotTempBelow) {
        fdf::error("The provided configuration does not match the trip point type. index = {}",
                   desc.index);
        completer.ReplyError(ZX_ERR_INVALID_ARGS);
        return;
      }
      if (std::isinf(
              desc.configuration.oneshot_temp_below_trip_point().critical_temperature_celsius)) {
        completer.ReplyError(ZX_ERR_INVALID_ARGS);
        return;
      }
    } else if (desc.configuration.is_oneshot_temp_above_trip_point()) {
      if (desc.type != fuchsia_hardware_trippoint::TripPointType::kOneshotTempAbove) {
        fdf::error("The provided configuration does not match the trip point type. index = {}",
                   desc.index);
        completer.ReplyError(ZX_ERR_INVALID_ARGS);
        return;
      }
      if (std::isinf(
              desc.configuration.oneshot_temp_above_trip_point().critical_temperature_celsius)) {
        completer.ReplyError(ZX_ERR_INVALID_ARGS);
        return;
      }
    } else if (desc.configuration.is_cleared_trip_point()) {
      // Any trip point is allowed to be cleard so we deliberately skip this.
    } else {
      fdf::error("The provided configuration is unknown by this hardware. index = {}", desc.index);
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
  }

  for (const auto& desc : request->descriptors) {
    TemperatureCelsius thresh_temp;
    bool trend;
    configured_trip_point_t cfg;

    configured_trip_points_[desc.index].reset();
    // Part of the contract of this interface is also to clear any pending trip
    // points for the trip point that's being reconfigured.
    const auto new_end =
        std::remove_if(pending_trips_.begin(), pending_trips_.end(),
                       [desc](fuchsia_hardware_trippoint::wire::TripPointResult r) {
                         return r.index == desc.index;
                       });
    pending_trips_.erase(new_end, pending_trips_.end());

    if (desc.configuration.is_oneshot_temp_above_trip_point()) {
      thresh_temp = desc.configuration.oneshot_temp_above_trip_point().critical_temperature_celsius;
      cfg.direction = TripPointDirection::Rise;
      trend = true;
    } else if (desc.configuration.is_oneshot_temp_below_trip_point()) {
      thresh_temp = desc.configuration.oneshot_temp_below_trip_point().critical_temperature_celsius;
      cfg.direction = TripPointDirection::Fall;
      trend = false;
    } else if (desc.configuration.is_cleared_trip_point()) {
      AckAndDisableIrq(desc.index);
      continue;
    } else {
      ZX_PANIC("Invalid trip point type");
    }

    cfg.temperature = thresh_temp;

    AckAndDisableIrq(desc.index);

    const uint32_t temp_code = tsensor_util::TempCelsiusToCode(thresh_temp, trend, trim_info_);
    ProgramTripPoint(temp_code, desc.index);

    configured_trip_points_[desc.index] = cfg;

    EnableIrq(desc.index);
  }

  // In case the client has cleared all trip points but still has a read pending
  // we want to complete that read request.
  CompletePendingRead();

  completer.ReplySuccess();
}

void AmlTripDevice::ProgramTripPoint(uint32_t temp_code, uint32_t index) {
  switch (index) {
    case 7:
      thermal::TsCfgReg7::Get()
          .ReadFrom(&sensor_mmio_)
          .set_fall_th1(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 6:
      thermal::TsCfgReg7::Get()
          .ReadFrom(&sensor_mmio_)
          .set_fall_th0(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 5:
      thermal::TsCfgReg6::Get()
          .ReadFrom(&sensor_mmio_)
          .set_fall_th1(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 4:
      thermal::TsCfgReg6::Get()
          .ReadFrom(&sensor_mmio_)
          .set_fall_th0(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 3:
      thermal::TsCfgReg5::Get()
          .ReadFrom(&sensor_mmio_)
          .set_rise_th1(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 2:
      thermal::TsCfgReg5::Get()
          .ReadFrom(&sensor_mmio_)
          .set_rise_th0(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 1:
      thermal::TsCfgReg4::Get()
          .ReadFrom(&sensor_mmio_)
          .set_rise_th1(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
    case 0:
      thermal::TsCfgReg4::Get()
          .ReadFrom(&sensor_mmio_)
          .set_rise_th0(temp_code)
          .WriteTo(&sensor_mmio_);
      return;
  }

  ZX_PANIC("Unhandled trip index: index = %u", index);
}

void AmlTripDevice::SetRebootTemperatureCelsius(TemperatureCelsius critical_temp_celsius) {
  uint32_t reboot_val = tsensor_util::TempCelsiusToCode(critical_temp_celsius, true, trim_info_);
  auto reboot_config = thermal::TsCfgReg2::Get().ReadFrom(&sensor_mmio_);

  reboot_config.set_hi_temp_enable(1)
      .set_reset_en(1)
      .set_high_temp_times(AML_TS_REBOOT_TIME)
      .set_high_temp_threshold(reboot_val << 4)
      .WriteTo(&sensor_mmio_);
}

void AmlTripDevice::WaitForAnyTripPoint(WaitForAnyTripPointCompleter::Sync& completer) {
  // Somebody else is already waiting on a trip point.
  if (pending_read_) {
    fdf::error("WaitForTripPoint is already bound");
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }

  if (!AtLeastOneTripConfigured()) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  pending_read_ = completer.ToAsync();

  CompletePendingRead();
}

void AmlTripDevice::AckAndDisableIrq(uint32_t index) {
  ZX_ASSERT_MSG(index < kNumTripPoints, "Trip Point index out of bounds, index = %u", index);
  auto sensor_ctl = thermal::TsCfgReg1::Get().ReadFrom(&sensor_mmio_);
  auto reg_value = sensor_ctl.reg_value();

  const uint32_t en_mask = (1 << (IRQ_RISE_ENABLE_SHIFT + index));
  const uint32_t clr_mask = (1 << (IRQ_RISE_STAT_CLR_SHIFT + index));

  reg_value &= ~(en_mask);
  reg_value |= clr_mask;

  sensor_ctl.set_reg_value(reg_value);
  sensor_ctl.WriteTo(&sensor_mmio_);

  reg_value &= ~(clr_mask);
  sensor_ctl.set_reg_value(reg_value);
  sensor_ctl.WriteTo(&sensor_mmio_);
}

void AmlTripDevice::CompletePendingRead() {
  if (!pending_read_) {
    return;
  }

  if (!AtLeastOneTripConfigured()) {
    pending_read_->ReplyError(ZX_ERR_BAD_STATE);
    fidl::Status st = pending_read_->result_of_reply();
    if (!st.ok()) {
      fdf::error("Failed to complete a pending WaitForTripPoint: {}",
                 st.FormatDescription().c_str());
    }
    pending_read_.reset();
    return;
  }

  if (pending_trips_.empty()) {
    return;
  }

  fuchsia_hardware_trippoint::wire::TripPointResult result = pending_trips_.front();
  pending_trips_.pop_front();

  // Trip points are one-shot so once a trip point is consumed it is no longer
  // configured.
  configured_trip_points_[result.index].reset();

  pending_read_->ReplySuccess(result);

  fidl::Status st = pending_read_->result_of_reply();
  if (!st.ok()) {
    fdf::error("Failed to complete a pending WaitForTripPoint: {}", st.FormatDescription().c_str());
  }

  pending_read_.reset();
}

bool AmlTripDevice::AtLeastOneTripConfigured() const {
  return std::any_of(
      configured_trip_points_.cbegin(), configured_trip_points_.cend(),
      [](const std::optional<configured_trip_point_t>& tp) { return tp.has_value(); });
}

void AmlTripDevice::InitSensor() {
  // Clear all IRQ's status.
  thermal::TsCfgReg1::Get()
      .ReadFrom(&sensor_mmio_)
      .set_fall_th3_irq_stat_clr(1)
      .set_fall_th2_irq_stat_clr(1)
      .set_fall_th1_irq_stat_clr(1)
      .set_fall_th0_irq_stat_clr(1)
      .set_rise_th3_irq_stat_clr(1)
      .set_rise_th2_irq_stat_clr(1)
      .set_rise_th1_irq_stat_clr(1)
      .set_rise_th0_irq_stat_clr(1)
      .WriteTo(&sensor_mmio_);

  thermal::TsCfgReg1::Get()
      .ReadFrom(&sensor_mmio_)
      .set_fall_th3_irq_stat_clr(0)
      .set_fall_th2_irq_stat_clr(0)
      .set_fall_th1_irq_stat_clr(0)
      .set_fall_th0_irq_stat_clr(0)
      .set_rise_th3_irq_stat_clr(0)
      .set_rise_th2_irq_stat_clr(0)
      .set_rise_th1_irq_stat_clr(0)
      .set_rise_th0_irq_stat_clr(0)
      .WriteTo(&sensor_mmio_);

  // Disable All IRQs
  thermal::TsCfgReg1::Get()
      .ReadFrom(&sensor_mmio_)
      .set_fall_th3_irq_en(0)
      .set_fall_th2_irq_en(0)
      .set_fall_th1_irq_en(0)
      .set_fall_th0_irq_en(0)
      .set_rise_th3_irq_en(0)
      .set_rise_th2_irq_en(0)
      .set_rise_th1_irq_en(0)
      .set_rise_th0_irq_en(0)
      .set_enable_irq(1)
      .WriteTo(&sensor_mmio_);

  thermal::TsCfgReg1::Get()
      .ReadFrom(&sensor_mmio_)
      .set_filter_en(1)
      .set_ts_ana_en_vcm(1)
      .set_ts_ana_en_vbg(1)
      .set_bipolar_bias_current_input(AML_TS_CH_SEL)
      .set_ts_ena_en_iptat(1)
      .set_ts_dem_en(1)
      .WriteTo(&sensor_mmio_);
}

void AmlTripDevice::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq,
                              zx_status_t status, const zx_packet_interrupt_t* interrupt) {
  if (status != ZX_OK) {
    fdf::error("IRQ wait returned: {}", zx_status_get_string(status));
    return;
  }

  TemperatureCelsius measured_temperature = ReadTemperatureCelsius();

  auto irq_stat = thermal::TsStat1::Get().ReadFrom(&sensor_mmio_);

  if (irq_stat.rise_th3_irq()) {
    QueueTripResult(measured_temperature, 3);
    AckAndDisableIrq(3);
  }
  if (irq_stat.rise_th2_irq()) {
    QueueTripResult(measured_temperature, 2);
    AckAndDisableIrq(2);
  }
  if (irq_stat.rise_th1_irq()) {
    QueueTripResult(measured_temperature, 1);
    AckAndDisableIrq(1);
  }
  if (irq_stat.rise_th0_irq()) {
    QueueTripResult(measured_temperature, 0);
    AckAndDisableIrq(0);
  }

  if (irq_stat.fall_th3_irq()) {
    QueueTripResult(measured_temperature, 7);
    AckAndDisableIrq(7);
  }
  if (irq_stat.fall_th2_irq()) {
    QueueTripResult(measured_temperature, 6);
    AckAndDisableIrq(6);
  }
  if (irq_stat.fall_th1_irq()) {
    QueueTripResult(measured_temperature, 5);
    AckAndDisableIrq(5);
  }
  if (irq_stat.fall_th0_irq()) {
    QueueTripResult(measured_temperature, 4);
    AckAndDisableIrq(4);
  }

  // Notify anybody waiting for a trip point that there may be data available.
  CompletePendingRead();
  irq_.ack();
}

void AmlTripDevice::EnableIrq(uint32_t index) {
  auto sensor_ctl = thermal::TsCfgReg1::Get().ReadFrom(&sensor_mmio_);
  auto reg_value = sensor_ctl.reg_value();

  reg_value |= (1U << (IRQ_RISE_ENABLE_SHIFT + index));
  reg_value &= ~(1U << (IRQ_RISE_STAT_CLR_SHIFT + index));

  sensor_ctl.set_reg_value(reg_value);

  sensor_ctl.WriteTo(&sensor_mmio_);
}

fuchsia_hardware_trippoint::TripPointType AmlTripDevice::GetTripPointType(uint32_t index) {
  ZX_ASSERT(index < kNumTripPoints);

  if (index >= kFallTripRange.first && index <= kFallTripRange.second) {
    return fuchsia_hardware_trippoint::TripPointType::kOneshotTempBelow;
  } else if (index >= kRiseTripRange.first && index <= kRiseTripRange.second) {
    return fuchsia_hardware_trippoint::TripPointType::kOneshotTempAbove;
  }

  __UNREACHABLE;
}

}  // namespace temperature
