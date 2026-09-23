// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tcs3400.h"

#include <fidl/fuchsia.hardware.lightsensor/cpp/fidl.h>
#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <fidl/fuchsia.input/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zx/clock.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/port.h>

#include <algorithm>

#include <fbl/auto_lock.h>

#include "tcs3400-regs.h"

namespace {
constexpr zx_duration_t INTERRUPTS_HYSTERESIS = ZX_MSEC(100);
constexpr uint8_t SAMPLES_TO_TRIGGER = 0x01;

// Repeat saturated log line every two minutes
constexpr zx::duration kSaturatedLogTime = zx::sec(120);
// Bright, not saturated values to return when saturated
constexpr uint16_t kMaxSaturationRed = 21'067;
constexpr uint16_t kMaxSaturationGreen = 20'395;
constexpr uint16_t kMaxSaturationBlue = 20'939;
constexpr uint16_t kMaxSaturationClear = 65'085;

constexpr zx::duration kIntegrationTimeStepSize = zx::usec(2780);
constexpr int64_t kMinIntegrationTimeStep = 1;
constexpr int64_t kMaxIntegrationTimeStep = 256;

#define GET_BYTE(val, shift) static_cast<uint8_t>(((val) >> (shift)) & 0xFF)

constexpr fuchsia_input::wire::Axis kLightSensorAxis = {
    .range = {.min = 0, .max = UINT16_MAX},
    .unit =
        {
            .type = fuchsia_input::wire::UnitType::kOther,
            .exponent = 0,
        },
};

constexpr fuchsia_input::wire::Axis kReportIntervalAxis = {
    .range = {.min = 0, .max = INT64_MAX},
    .unit =
        {
            .type = fuchsia_input::wire::UnitType::kSeconds,
            .exponent = -6,
        },
};

constexpr fuchsia_input::wire::Axis kSensitivityAxis = {
    .range = {.min = 1, .max = 64},
    .unit =
        {
            .type = fuchsia_input::wire::UnitType::kOther,
            .exponent = 0,
        },
};

constexpr fuchsia_input::wire::Axis kSamplingRateAxis = {
    .range = {.min = kIntegrationTimeStepSize.to_usecs(),
              .max = kIntegrationTimeStepSize.to_usecs() * kMaxIntegrationTimeStep},
    .unit =
        {
            .type = fuchsia_input::wire::UnitType::kSeconds,
            .exponent = -6,
        },
};

constexpr fuchsia_input_report::wire::SensorAxis MakeLightSensorAxis(
    fuchsia_input_report::wire::SensorType type) {
  return {.axis = kLightSensorAxis, .type = type};
}

template <typename T>
bool FeatureValueValid(int64_t value, const T& axis) {
  return value >= axis.range.min && value <= axis.range.max;
}

constexpr std::string_view kEventTime("event_time");
constexpr std::string_view kReportingIntervalUs("report_interval_us");
constexpr std::string_view kReportingState("reporting_state");
constexpr std::string_view kSensitivity("sensitivity");
constexpr std::string_view kThresholdHigh("threshold_high");
constexpr std::string_view kThresholdLow("threshold_low");
constexpr std::string_view kIntegrationTimeUs("integration_time_us");

void RecordReport(inspect::Node& n, tcs::Tcs3400FeatureReport report) {
  n.RecordUint(kEventTime, (report.event_time - zx::time()).to_nsecs());
  n.RecordUint(kReportingIntervalUs, report.report_interval_us);
  n.RecordUint(kSensitivity, report.sensitivity);
  n.RecordUint(kThresholdHigh, report.threshold_high);
  n.RecordUint(kThresholdLow, report.threshold_low);
  n.RecordUint(kIntegrationTimeUs, report.integration_time_us);
  switch (report.reporting_state) {
    case fuchsia_input_report::SensorReportingState::kReportNoEvents:
      n.RecordString(kReportingState, "NoEvents");
      break;
    case fuchsia_input_report::SensorReportingState::kReportAllEvents:
      n.RecordString(kReportingState, "AllEvents");
      break;
    case fuchsia_input_report::SensorReportingState::kReportThresholdEvents:
      n.RecordString(kReportingState, "ThresholdEvents");
      break;
    default:
      n.RecordString(kReportingState, "Unknown");
      break;
  }
}

}  // namespace

namespace tcs {

void Tcs3400InputReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<int64_t> values(allocator, 4);
  values[0] = illuminance;
  values[1] = red;
  values[2] = green;
  values[3] = blue;

  auto sensor_report =
      fuchsia_input_report::wire::SensorInputReport::Builder(allocator).values(values);
  input_report.event_time(event_time.get()).sensor(sensor_report.Build());
}

fuchsia_input_report::wire::FeatureReport Tcs3400FeatureReport::ToFidlFeatureReport(
    fidl::AnyArena& allocator) const {
  fidl::VectorView<int64_t> sens(allocator, 1);
  sens[0] = sensitivity;

  fidl::VectorView<int64_t> thresh_high(allocator, 1);
  thresh_high[0] = threshold_high;

  fidl::VectorView<int64_t> thresh_low(allocator, 1);
  thresh_low[0] = threshold_low;

  const auto sensor_report = fuchsia_input_report::wire::SensorFeatureReport::Builder(allocator)
                                 .report_interval(report_interval_us)
                                 .reporting_state(reporting_state)
                                 .sensitivity(sens)
                                 .threshold_high(thresh_high)
                                 .threshold_low(thresh_low)
                                 .sampling_rate(integration_time_us)
                                 .Build();

  return fuchsia_input_report::wire::FeatureReport::Builder(allocator)
      .sensor(sensor_report)
      .Build();
}

InspectTcs3400FeatureReport::InspectTcs3400FeatureReport(inspect::Node n,
                                                         const Tcs3400FeatureReport& report)
    : node(std::move(n)),
      event_time(node.CreateUint(kEventTime, 0)),
      report_interval_us(node.CreateUint(kReportingIntervalUs, 0)),
      reporting_state(node.CreateString(kReportingState, "Unknown")),
      sensitivity(node.CreateUint(kSensitivity, 0)),
      threshold_high(node.CreateUint(kThresholdHigh, 0)),
      threshold_low(node.CreateUint(kThresholdLow, 0)),
      integration_time_us(node.CreateUint(kIntegrationTimeUs, 0)) {
  event_time.Set((zx::clock::get_monotonic() - zx::time()).to_nsecs());
  threshold_low.Set(report.threshold_low);
  threshold_high.Set(report.threshold_high);
  sensitivity.Set(report.sensitivity);
  report_interval_us.Set(report.report_interval_us);
  switch (report.reporting_state) {
    case fuchsia_input_report::SensorReportingState::kReportNoEvents:
      reporting_state.Set("NoEvents");
      break;
    case fuchsia_input_report::SensorReportingState::kReportAllEvents:
      reporting_state.Set("AllEvents");
      break;
    case fuchsia_input_report::SensorReportingState::kReportThresholdEvents:
      reporting_state.Set("ThresholdEvents");
      break;
    default:
      reporting_state.Set("Unknown");
      break;
  }
  integration_time_us.Set(report.integration_time_us);
}

zx::result<Tcs3400InputReport> Tcs3400::ReadInputRpt() {
  Tcs3400InputReport report{.event_time = zx::clock::get_monotonic()};

  bool saturatedReading = false;
  struct Regs {
    int64_t* out;
    uint8_t reg_h;
    uint8_t reg_l;
  } regs[] = {
      {&report.illuminance, TCS_I2C_CDATAH, TCS_I2C_CDATAL},
      {&report.red, TCS_I2C_RDATAH, TCS_I2C_RDATAL},
      {&report.green, TCS_I2C_GDATAH, TCS_I2C_GDATAL},
      {&report.blue, TCS_I2C_BDATAH, TCS_I2C_BDATAL},
  };

  for (const auto& i : regs) {
    // Read lower byte first, the device holds upper byte of a sample in a shadow register after
    // a lower byte read
    zx::result buf_l = ReadReg(i.reg_l);
    if (buf_l.is_error()) {
      fdf::error("i2c_write_read_sync failed: {}", buf_l);
      return buf_l.take_error();
    }
    zx::result buf_h = ReadReg(i.reg_h);
    if (buf_h.is_error()) {
      fdf::error("i2c_write_read_sync failed: {}", buf_h);
      return buf_h.take_error();
    }
    auto out = static_cast<uint16_t>(
        static_cast<float>(((buf_h.value() & 0xFF) << 8) | (buf_l.value() & 0xFF)));

    // Use memcpy here because i.out is a misaligned pointer and dereferencing a
    // misaligned pointer is UB. This ends up getting lowered to a 16-bit store.
    memcpy(i.out, &out, sizeof(out));

    fdf::debug("raw: {:#04x}  again: {}  atime: {}", out, again_, atime_);
  }

  zx::result status_val = ReadReg(TCS_I2C_STATUS);
  if (status_val.is_error()) {
    fdf::error("i2c_write_read_sync failed: {}", status_val);
    return status_val.take_error();
  }
  if ((status_val.value() & TCS_I2C_STATUS_ASAT) == TCS_I2C_STATUS_ASAT) {
    if (zx::result result = WriteReg(TCS_I2C_CICLEAR, 0x00); result.is_error()) {
      fdf::error("Unable to clear saturation status: {}", result);
    }

    report.red = kMaxSaturationRed;
    report.green = kMaxSaturationGreen;
    report.blue = kMaxSaturationBlue;
    report.illuminance = kMaxSaturationClear;
    saturatedReading = true;
    if (!isSaturated_ || zx::clock::get_monotonic() - lastSaturatedLog_ >= kSaturatedLogTime) {
      fdf::info("sensor is saturated via status register");
      lastSaturatedLog_ = zx::clock::get_monotonic();
    }
  } else if (isSaturated_) {
    fdf::info("sensor is no longer saturated");
  }
  isSaturated_ = saturatedReading;

  return zx::ok(report);
}

void Tcs3400::Configure() {
  Tcs3400FeatureReport feature_report;
  {
    fbl::AutoLock lock(&feature_lock_);
    feature_report = feature_rpt_;
  }

  uint8_t control_reg = 0;
  // clang-format off
  if (feature_report.sensitivity == 4)  control_reg = 1;
  if (feature_report.sensitivity == 16) control_reg = 2;
  if (feature_report.sensitivity == 64) control_reg = 3;
  // clang-format on

  again_ = static_cast<uint8_t>(feature_report.sensitivity);

  const int64_t atime = feature_report.integration_time_us / kIntegrationTimeStepSize.to_usecs();
  atime_ = static_cast<uint8_t>(kMaxIntegrationTimeStep - atime);

  struct Setup {
    uint8_t cmd;
    uint8_t val;
  } __PACKED setup[] = {
      // First we don't set TCS_I2C_ENABLE_ADC_ENABLE to disable the sensor.
      {TCS_I2C_ENABLE, TCS_I2C_ENABLE_POWER_ON | TCS_I2C_ENABLE_INT_ENABLE},
      {TCS_I2C_AILTL, GET_BYTE(feature_report.threshold_low, 0)},
      {TCS_I2C_AILTH, GET_BYTE(feature_report.threshold_low, 8)},
      {TCS_I2C_AIHTL, GET_BYTE(feature_report.threshold_high, 0)},
      {TCS_I2C_AIHTH, GET_BYTE(feature_report.threshold_high, 8)},
      {TCS_I2C_PERS, SAMPLES_TO_TRIGGER},
      {TCS_I2C_CONTROL, control_reg},
      {TCS_I2C_ATIME, atime_},
      // We now do set TCS_I2C_ENABLE_ADC_ENABLE to re-enable the sensor.
      {TCS_I2C_ENABLE,
       TCS_I2C_ENABLE_POWER_ON | TCS_I2C_ENABLE_ADC_ENABLE | TCS_I2C_ENABLE_INT_ENABLE},
  };
  for (const auto& i : setup) {
    if (zx::result result = WriteReg(i.cmd, i.val); result.is_error()) {
      fdf::error("i2c_write_sync failed: {}", result);
      break;  // do not exit thread, future transactions may succeed
    }
  }

  // per spec 0 is device's default. we define the default as no polling.
  polling_handler_.Cancel();
  if (feature_report.report_interval_us != 0) {
    polling_handler_.PostDelayed(dispatcher(), zx::usec(feature_report.report_interval_us));
  }
}

void Tcs3400::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                        const zx_packet_interrupt_t* interrupt) {
  Tcs3400FeatureReport feature_report;
  {
    fbl::AutoLock lock(&feature_lock_);
    feature_report = feature_rpt_;
  }

  zx_interrupt_ack(irq_.get());  // rearm interrupt at the IRQ level

  const zx::result<Tcs3400InputReport> report = ReadInputRpt();
  if (report.is_error()) {
    rearm_irq_handler_.PostDelayed(dispatcher, zx::duration(INTERRUPTS_HYSTERESIS));
    return;
  }
  if (feature_report.reporting_state ==
      fuchsia_input_report::wire::SensorReportingState::kReportNoEvents) {
    rearm_irq_handler_.PostDelayed(dispatcher, zx::duration(INTERRUPTS_HYSTERESIS));
    return;
  }

  if (report->illuminance > feature_report.threshold_high ||
      report->illuminance < feature_report.threshold_low) {
    readers_.SendReportToAllReaders(std::move(*report));
  }

  fbl::AutoLock lock(&input_lock_);
  input_rpt_ = *report;

  rearm_irq_handler_.PostDelayed(dispatcher, zx::duration(INTERRUPTS_HYSTERESIS));
}

void Tcs3400::RearmIrq() {
  // rearm interrupt at the device level
  zx::result result = WriteReg(TCS_I2C_AICLEAR, 0x00);
  if (result.is_error()) {
    fdf::error("i2c_write_sync failed: {}", result);
    // Continue on error, future transactions may succeed
  }
}

void Tcs3400::HandlePoll() {
  Tcs3400FeatureReport feature_report;
  {
    fbl::AutoLock lock(&feature_lock_);
    feature_report = feature_rpt_;
  }

  if (feature_report.reporting_state ==
      fuchsia_input_report::wire::SensorReportingState::kReportAllEvents) {
    const zx::result<Tcs3400InputReport> report = ReadInputRpt();
    if (report.is_ok()) {
      readers_.SendReportToAllReaders(std::move(*report));
      fbl::AutoLock lock(&input_lock_);
      input_rpt_ = *report;
    }
  }

  polling_handler_.PostDelayed(dispatcher(), zx::usec(feature_report.report_interval_us));
}

void Tcs3400::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                    GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
}

void Tcs3400::GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                                      GetInputReportsReaderV2Completer::Sync& completer) {
  const uint16_t max_unacknowledged_reports =
      std::clamp<uint16_t>(request->max_unacknowledged_reports_limit, 1, kMaxReportsPerHalfSecond);
  if (request->max_unacknowledged_reports_limit != max_unacknowledged_reports) {
    fdf::warn("GetInputReportsReaderV2: requested limit {} clamped to {}",
              request->max_unacknowledged_reports_limit, max_unacknowledged_reports);
  }
  zx_status_t status =
      readers_.CreateReaderV2(dispatcher(), std::move(request->reader), max_unacknowledged_reports);
  if (status != ZX_OK) {
    fdf::error("Failed to create InputReportReaderV2: {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }
  OnNextReader();
  completer.Reply(max_unacknowledged_reports);
}

void Tcs3400::GetDescriptor(GetDescriptorCompleter::Sync& completer) {
  using SensorAxisVector = fidl::VectorView<fuchsia_input_report::wire::SensorAxis>;

  fidl::Arena<kFeatureAndDescriptorBufferSize> allocator;

  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kAmsLightSensor));

  auto sensor_axes = SensorAxisVector(allocator, 4);
  sensor_axes[0] = MakeLightSensorAxis(fuchsia_input_report::wire::SensorType::kLightIlluminance);
  sensor_axes[1] = MakeLightSensorAxis(fuchsia_input_report::wire::SensorType::kLightRed);
  sensor_axes[2] = MakeLightSensorAxis(fuchsia_input_report::wire::SensorType::kLightGreen);
  sensor_axes[3] = MakeLightSensorAxis(fuchsia_input_report::wire::SensorType::kLightBlue);

  fidl::VectorView<fuchsia_input_report::wire::SensorInputDescriptor> input_descriptor(allocator,
                                                                                       1);
  input_descriptor[0] = fuchsia_input_report::wire::SensorInputDescriptor::Builder(allocator)
                            .values(sensor_axes)
                            .Build();

  auto sensitivity_axes = SensorAxisVector(allocator, 1);
  sensitivity_axes[0] = {
      .axis = kSensitivityAxis,
      .type = fuchsia_input_report::wire::SensorType::kLightIlluminance,
  };

  auto threshold_high_axes = SensorAxisVector(allocator, 1);
  threshold_high_axes[0] =
      MakeLightSensorAxis(fuchsia_input_report::wire::SensorType::kLightIlluminance);

  auto threshold_low_axes = SensorAxisVector(allocator, 1);
  threshold_low_axes[0] =
      MakeLightSensorAxis(fuchsia_input_report::wire::SensorType::kLightIlluminance);

  fidl::VectorView<fuchsia_input_report::wire::SensorFeatureDescriptor> feature_descriptor(
      allocator, 1);
  feature_descriptor[0] = fuchsia_input_report::wire::SensorFeatureDescriptor::Builder(allocator)
                              .report_interval(kReportIntervalAxis)
                              .supports_reporting_state(true)
                              .sensitivity(sensitivity_axes)
                              .threshold_high(threshold_high_axes)
                              .threshold_low(threshold_low_axes)
                              .sampling_rate(kSamplingRateAxis)
                              .Build();

  const auto sensor_descriptor = fuchsia_input_report::wire::SensorDescriptor::Builder(allocator)
                                     .input(input_descriptor)
                                     .feature(feature_descriptor)
                                     .Build();

  const auto descriptor = fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator)
                              .device_information(device_info.Build())
                              .sensor(sensor_descriptor)
                              .Build();

  completer.Reply(descriptor);
}

void Tcs3400::SendOutputReport(SendOutputReportRequestView request,
                               SendOutputReportCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Tcs3400::GetFeatureReport(GetFeatureReportCompleter::Sync& completer) {
  fbl::AutoLock lock(&feature_lock_);
  fidl::Arena<kFeatureAndDescriptorBufferSize> allocator;
  completer.ReplySuccess(feature_rpt_.ToFidlFeatureReport(allocator));
}

void Tcs3400::SetFeatureReport(SetFeatureReportRequestView request,
                               SetFeatureReportCompleter::Sync& completer) {
  const auto& report = request->report;
  if (!report.has_sensor()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!report.sensor().has_report_interval() || report.sensor().report_interval() < 0) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!report.sensor().has_sensitivity() || report.sensor().sensitivity().size() != 1 ||
      !FeatureValueValid(report.sensor().sensitivity()[0], kSensitivityAxis)) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  const int64_t gain = report.sensor().sensitivity()[0];
  if (!(gain == 1 || gain == 4 || gain == 16 || gain == 64)) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!report.sensor().has_threshold_high() || report.sensor().threshold_high().size() != 1 ||
      !FeatureValueValid(report.sensor().threshold_high()[0], kLightSensorAxis)) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!report.sensor().has_threshold_low() || report.sensor().threshold_low().size() != 1 ||
      !FeatureValueValid(report.sensor().threshold_low()[0], kLightSensorAxis)) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!report.sensor().has_sampling_rate()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  const int64_t atime = report.sensor().sampling_rate() / kIntegrationTimeStepSize.to_usecs();
  if (atime < 1 || atime > 256) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  Tcs3400FeatureReport feature_report;
  {
    fbl::AutoLock lock(&feature_lock_);
    feature_rpt_.event_time = zx::clock::get_monotonic();
    feature_rpt_.report_interval_us = report.sensor().report_interval();
    feature_rpt_.reporting_state = report.sensor().reporting_state();
    feature_rpt_.sensitivity = report.sensor().sensitivity()[0];
    feature_rpt_.threshold_high = report.sensor().threshold_high()[0];
    feature_rpt_.threshold_low = report.sensor().threshold_low()[0];
    feature_rpt_.integration_time_us = atime * kIntegrationTimeStepSize.to_usecs();
    feature_report = feature_rpt_;
  }
  inspect_reports_->CreateEntry(
      [feature_report](inspect::Node& n) { RecordReport(n, feature_report); });

  Configure();
  completer.ReplySuccess();
}

void Tcs3400::GetInputReport(GetInputReportRequestView request,
                             GetInputReportCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(false);
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t Tcs3400::InitGain(uint8_t gain) {
  if (gain != 1 && gain != 4 && gain != 16 && gain != 64) {
    fdf::warn("Invalid gain ({}) using gain = 1", gain);
    gain = 1;
  }

  again_ = gain;
  fdf::debug("again ({})", again_);

  uint8_t reg;
  // clang-format off
  if (gain == 1)  reg = 0;
  if (gain == 4)  reg = 1;
  if (gain == 16) reg = 2;
  if (gain == 64) reg = 3;
  // clang-format on

  zx::result result = WriteReg(TCS_I2C_CONTROL, reg);
  if (result.is_error()) {
    fdf::error("Setting gain failed {}", result);
    return result.status_value();
  }

  return ZX_OK;
}

zx::result<> Tcs3400::InitMetadata(fdf::Namespace& incoming) {
  zx::result pdev_client =
      incoming.Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client.value()));

  zx::result metadata_result = pdev.GetFidlMetadata<fuchsia_hardware_lightsensor::Metadata>();
  if (metadata_result.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata_result);
    return metadata_result.take_error();
  }
  const fuchsia_hardware_lightsensor::Metadata& metadata = metadata_result.value();
  if (!metadata.integration_time().has_value()) {
    fdf::error("Metadata missing `integration_time` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.gain().has_value()) {
    fdf::error("Metadata missing `gain` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!metadata.polling_time().has_value()) {
    fdf::error("Metadata missing `polling_time` field");
    return zx::error(ZX_ERR_INTERNAL);
  }

  // ATIME = 256 - Integration Time / 2.78 ms.
  zx::duration integration_time(metadata.integration_time().value());
  int64_t atime = integration_time.get() / kIntegrationTimeStepSize.get();
  if (atime < kMinIntegrationTimeStep || atime > kMaxIntegrationTimeStep) {
    atime = kMaxIntegrationTimeStep - 1;
    fdf::warn("Invalid integration time ({}us) using atime = 1", integration_time.to_usecs());
  }
  atime_ = static_cast<uint8_t>(kMaxIntegrationTimeStep - atime);

  fdf::debug("atime ({})", atime_);
  if (zx::result result = WriteReg(TCS_I2C_ATIME, atime_); result.is_error()) {
    fdf::error("Setting integration time failed {}", result);
    return result.take_error();
  }

  zx_status_t status = InitGain(metadata.gain().value());
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // Set the default features and send a configuration packet.
  Tcs3400FeatureReport feature_report;
  {
    fbl::AutoLock lock(&feature_lock_);
    // The device will trigger an interrupt outside the thresholds.  These default threshold
    // values effectively disable interrupts since we can't be outside this range, interrupts
    // get effectively enabled when we configure a range that could trigger.
    feature_rpt_.threshold_low = 0x0000;
    feature_rpt_.threshold_high = 0xFFFF;
    feature_rpt_.sensitivity = again_;
    feature_rpt_.report_interval_us = zx::duration(metadata.polling_time().value()).to_usecs();
    feature_rpt_.reporting_state =
        fuchsia_input_report::wire::SensorReportingState::kReportAllEvents;
    feature_rpt_.integration_time_us = atime * kIntegrationTimeStepSize.to_usecs();
    feature_report = feature_rpt_;
  }
  inspect_reports_->CreateEntry(
      [feature_report](inspect::Node& n) { RecordReport(n, feature_report); });

  Configure();
  return zx::ok();
}

zx::result<uint8_t> Tcs3400::ReadReg(uint8_t reg) {
  const std::array<uint8_t, 1> write_data = {reg};
  constexpr uint8_t kNumberOfRetries = 2;
  constexpr zx::duration kRetryDelay = zx::msec(1);
  std::array<uint8_t, 1> read_data;
  auto result = i2c_.WriteReadSyncRetries(write_data, read_data, kNumberOfRetries, kRetryDelay);
  if (result.is_error()) {
    fdf::error("I2C write to register {:#02x} failed: {}", reg, result);
    return result.take_error();
  }
  return zx::ok(read_data[0]);
}

zx::result<> Tcs3400::WriteReg(uint8_t reg, uint8_t value) {
  std::array<uint8_t, 2> write_data = {reg, value};
  constexpr uint8_t kNumberOfRetries = 2;
  constexpr zx::duration kRetryDelay = zx::msec(1);
  auto result = i2c_.WriteSyncRetries(write_data, kNumberOfRetries, kRetryDelay);
  if (result.is_error()) {
    fdf::error("I2C write to register {:#02x} failed: {}", reg, result);
    return result.take_error();
  }
  return zx::ok();
}

zx::result<> Tcs3400::Start(fdf::DriverContext context) {
  component_inspector_ = context.CreateInspector(this);
  auto incoming = context.take_incoming();
  inspect_reports_.emplace(
      component_inspector_->inspector().GetRoot().CreateChild("feature_reports"),
      kMaxFeatureReports);

  zx::result gpio = incoming->Connect<fuchsia_hardware_gpio::Service::Device>("gpio-int");
  if (gpio.is_error()) {
    fdf::error("Failed to connect to gpio protocol: {}", gpio);
    return gpio.take_error();
  }
  gpio_.Bind(std::move(gpio.value()));

  zx::result i2c = i2c::I2cChannel::FromIncoming(*incoming, "i2c");
  if (i2c.is_error()) {
    fdf::error("Failed to create i2c channel: {}", i2c);
    return i2c.take_error();
  }
  i2c_ = std::move(i2c.value());

  fidl::Arena arena;
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                              .Build();
  fidl::WireResult configure_result = gpio_->ConfigureInterrupt(interrupt_config);
  if (!configure_result.ok()) {
    fdf::error("Failed to send ConfigureInterrupt request to gpio: {}",
               configure_result.status_string());
    return zx::error(configure_result.status());
  }
  if (configure_result->is_error()) {
    fdf::error("Failed to configure interrupt: {}",
               zx_status_get_string(configure_result->error_value()));
    return configure_result->take_error();
  }

  fidl::WireResult interrupt_result = gpio_->GetInterrupt({});
  if (!interrupt_result.ok()) {
    fdf::error("Failed to send GetInterrupt request to gpio: {}", interrupt_result.status_string());
    return zx::error(interrupt_result.status());
  }
  if (interrupt_result->is_error()) {
    fdf::error("Failed to get interrupt from gpio: {}",
               zx_status_get_string(interrupt_result->error_value()));
    return interrupt_result->take_error();
  }
  irq_ = std::move(interrupt_result->value()->interrupt);
  irq_handler_.set_object(irq_.get());
  irq_handler_.Begin(dispatcher());

  if (zx::result result = InitMetadata(*incoming); result.is_error()) {
    return result.take_error();
    ;
  }

  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("Failed bind devfs connector: {}", connector.status_string());
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args(
      {.connector = std::move(connector.value()), .class_name = "input-report"});

  zx::result child = AddOwnedChild(kChildNodeName, devfs_args);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void Tcs3400::DevfsConnect(fidl::ServerEnd<fuchsia_input_report::InputDevice> request) {
  bindings_.AddBinding(dispatcher(), std::move(request), this, fidl::kIgnoreBindingClosure);
}

}  // namespace tcs

FUCHSIA_DRIVER_EXPORT2(tcs::Tcs3400);
