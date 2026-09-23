// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adc-buttons-device.h"

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/clock.h>
#include <zircon/status.h>

#include <algorithm>
#include <cstdint>

namespace adc_buttons_device {

void AdcButtonsDevice::AdcButtonInputReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<fuchsia_input_report::wire::ConsumerControlButton> pressed_buttons_rpt(
      allocator, buttons.size());
  std::copy(buttons.begin(), buttons.end(), pressed_buttons_rpt.begin());

  auto consumer_control_report =
      fuchsia_input_report::wire::ConsumerControlInputReport::Builder(allocator).pressed_buttons(
          pressed_buttons_rpt);
  input_report.event_time(event_time.get()).consumer_control(consumer_control_report.Build());
}

void AdcButtonsDevice::PollingTask(async_dispatcher_t* dispatcher, async::TaskBase* task,
                                   zx_status_t status) {
  if (status != ZX_OK) {
    return;
  }

  polling_task_.PostDelayed(dispatcher_, polling_interval_);

  zx::result<AdcButtonInputReport> report = GetInputReport();
  if (report.is_error()) {
    fdf::error("Failed to get report {}", report.status_string());
    return;
  }
  if (rpt_.has_value() && rpt_->buttons == report->buttons) {
    return;
  }

  rpt_ = std::move(*report);
  readers_.SendReportToAllReaders(*rpt_);
}

zx::result<AdcButtonsDevice::AdcButtonInputReport> AdcButtonsDevice::GetInputReport() {
  std::set<fuchsia_input_report::ConsumerControlButton> buttons;
  for (const auto& client : clients_) {
    auto result = client.adc_->GetSample();
    if (!result.ok()) {
      fdf::error("{}: GetSample failed {}", __func__, result.FormatDescription());
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (result->is_error()) {
      fdf::error("{}: GetSample failed {}", __func__, result->error_value());
      return zx::error(result->error_value());
    }

    for (const auto& cfg : client.buttons_) {
      const auto& saradc = cfg.button_config()->adc();
      if (result.value()->value >= saradc->press_threshold() &&
          result.value()->value < saradc->release_threshold()) {
        buttons.insert(cfg.types()->begin(), cfg.types()->end());
      }
    }
  }
  ZX_DEBUG_ASSERT_MSG(
      buttons.size() <= fuchsia_input_report::kConsumerControlMaxNumButtons,
      "More buttons than expected (max = %d). Please increase kConsumerControlMaxNumButtons ",
      fuchsia_input_report::kConsumerControlMaxNumButtons);

  return zx::ok(AdcButtonInputReport{
      .event_time = zx::clock::get_monotonic(),
      .buttons = std::move(buttons),
  });
}

void AdcButtonsDevice::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                             GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
}

void AdcButtonsDevice::GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                                               GetInputReportsReaderV2Completer::Sync& completer) {
  zx::result<AdcButtonInputReport> initial_report = GetInputReport();
  if (initial_report.is_error()) {
    fdf::error("Failed to get initial report {}", initial_report.status_string());
  }
  // Max unacknowledged report count allowed for 1/2 second based on the configured
  // polling_interval_ (e.g. 20 ms polling interval yields 25 reports per 1/2 second).
  const uint16_t kMaxReportsPerHalfSecond =
      static_cast<uint16_t>(std::clamp<int64_t>(zx::msec(500) / polling_interval_, 1, UINT16_MAX));
  const uint16_t max_unacknowledged_reports =
      std::clamp<uint16_t>(request->max_unacknowledged_reports_limit, 1, kMaxReportsPerHalfSecond);
  if (request->max_unacknowledged_reports_limit != max_unacknowledged_reports) {
    fdf::warn("GetInputReportsReaderV2: requested limit {} clamped to {}",
              request->max_unacknowledged_reports_limit, max_unacknowledged_reports);
  }
  zx_status_t status = readers_.CreateReaderV2(
      dispatcher_, std::move(request->reader), max_unacknowledged_reports,
      initial_report.is_ok() ? std::make_optional(initial_report.value()) : std::nullopt);
  if (status != ZX_OK) {
    fdf::error("CreateReaderV2 failed: {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }
  completer.Reply(max_unacknowledged_reports);
}

void AdcButtonsDevice::GetDescriptor(GetDescriptorCompleter::Sync& completer) {
  fidl::Arena<kFeatureAndDescriptorBufferSize> allocator;

  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kAdcButtons));
  device_info.polling_rate(polling_interval_.get());

  fidl::VectorView<fuchsia_input_report::wire::ConsumerControlButton> buttons(allocator,
                                                                              buttons_.size());
  std::copy(buttons_.begin(), buttons_.end(), buttons.begin());

  const auto input = fuchsia_input_report::wire::ConsumerControlInputDescriptor::Builder(allocator)
                         .buttons(buttons)
                         .Build();

  const auto consumer_control =
      fuchsia_input_report::wire::ConsumerControlDescriptor::Builder(allocator)
          .input(input)
          .Build();

  const auto descriptor = fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator)
                              .device_information(device_info.Build())
                              .consumer_control(consumer_control)
                              .Build();

  completer.Reply(descriptor);
}

void AdcButtonsDevice::Shutdown() { polling_task_.Cancel(); }

}  // namespace adc_buttons_device
