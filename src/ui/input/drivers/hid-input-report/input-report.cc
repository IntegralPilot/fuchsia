// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "input-report.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/epitaph.h>
#include <lib/fit/defer.h>
#include <lib/hid-parser/parser.h>
#include <lib/hid-parser/report.h>
#include <lib/hid-parser/usages.h>
#include <lib/trace/internal/event_common.h>
#include <lib/zx/clock.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <algorithm>

#include <fbl/alloc_checker.h>

#include "src/ui/input/lib/hid-input-report/device.h"

namespace hid_input_report_dev {

namespace fhidbus = fuchsia_hardware_hidbus;
namespace finput = fuchsia_hardware_input;

void InputReport::RemoveReaderFromList(InputReportsReaderV2* reader) {
  std::erase_if(readers_v2_list_, [reader](const auto& item) { return item.get() == reader; });
}

void InputReport::HandleReports(
    fidl::WireUnownedResult<finput::DeviceReportsReader::ReadReports>& result) {
  if (!result.ok()) {
    return;
  }
  if (result->is_error()) {
    return;
  }

  zx::time time = zx::clock::get_monotonic();
  for (const fuchsia_hardware_hidbus::wire::Report& report : result->value()->reports) {
    if (!report.has_buf()) {
      fdf::error("Report does not have data!");
      continue;
    }
    if (report.has_wake_lease()) {
      const zx::duration kLeaseTimeout = zx::msec(500);
      wake_lease_.DepositWakeLease(std::move(report.wake_lease()),
                                   zx::deadline_after(kLeaseTimeout));
    }
    HandleReport(cpp20::span<uint8_t>(report.buf().data(), report.buf().size()),
                 report.has_timestamp() ? zx::time(report.timestamp()) : time);
  }

  dev_reader_->ReadReports().Then(fit::bind_member<&InputReport::HandleReports>(this));
}

void InputReport::HandleReport(cpp20::span<const uint8_t> report, zx::time report_time) {
  for (auto& device : devices_) {
    // A Device may not have input reports at all: for example, a
    // TouchConfiguration Device takes only feature reports but no input
    // report. We should ignore all such devices when handling input reports.
    if (!device->InputReportId().has_value()) {
      continue;
    }

    // TODO(https://fxbug.dev/42085733): For Devices accepting input reports, there are
    // two possible cases: [1] the Device may have its dedicated report ID,
    // which can be any integer value in [1, 255]; or [2] the Device doesn't
    // have its dedicated report ID which is allowed if there is only one
    // report. Currently for [2], this library sets the report_id field to 0,
    // which is a value reserved by the USB-HID standard.
    //
    // Instead of using a placeholder value (0) that is reserved by the specs,
    // we should use a different way to indicate that the Device takes input
    // reports but without report IDs.
    const uint8_t device_input_report_id = *device->InputReportId();
    static constexpr uint8_t kInputReportIdForDevicesWithoutReportIds = 0;
    if (device_input_report_id != kInputReportIdForDevicesWithoutReportIds) {
      // If a device has a report ID, it must match the ID from the incoming
      // report.
      const uint8_t incoming_input_report_id = report[0];
      if (device_input_report_id != incoming_input_report_id) {
        continue;
      }
    }

    for (auto& reader : readers_v2_list_) {
      reader->ReceiveReport(report, report_time, device.get());
    }
  }

  const zx::duration latency = zx::clock::get_monotonic() - report_time;

  total_latency_ += latency;
  total_report_count_.Set(++report_count_);
  average_latency_usecs_.Set(total_latency_.to_usecs() / report_count_);

  if (latency > max_latency_) {
    max_latency_ = latency;
    max_latency_usecs_.Set(max_latency_.to_usecs());
  }

  latency_histogram_usecs_.Insert(latency.to_usecs());
  last_event_timestamp_.Set(report_time.get());
}

bool InputReport::ParseHidInputReportDescriptor(const hid::ReportDescriptor* report_desc) {
  std::unique_ptr<hid_input_report::Device> device;
  hid_input_report::ParseResult result = hid_input_report::CreateDevice(report_desc, &device);
  if (result != hid_input_report::ParseResult::kOk) {
    return false;
  }
  if (device->GetDeviceType() == hid_input_report::DeviceType::kSensor) {
    sensor_count_++;
  }
  devices_.push_back(std::move(device));
  return true;
}

void InputReport::SendInitialConsumerControlReport(InputReportsReaderV2* reader) {
  for (auto& device : devices_) {
    if (device->GetDeviceType() == hid_input_report::DeviceType::kConsumerControl) {
      if (!device->InputReportId().has_value()) {
        continue;
      }

      fidl::WireResult result =
          input_device_->GetReport(fhidbus::ReportType::kInput, *device->InputReportId());
      if (!result.ok() || result->is_error()) {
        continue;
      }
      reader->ReceiveReport(
          cpp20::span(result.value()->report.data(), result.value()->report.size()),
          zx::clock::get_monotonic(), device.get());
    }
  }
}

std::string InputReport::GetDeviceTypesString() const {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
  const char* kDeviceTypeNames[] = {
      [static_cast<uint32_t>(hid_input_report::DeviceType::kMouse)] = "mouse",
      [static_cast<uint32_t>(hid_input_report::DeviceType::kSensor)] = "sensor",
      [static_cast<uint32_t>(hid_input_report::DeviceType::kTouch)] = "touch",
      [static_cast<uint32_t>(hid_input_report::DeviceType::kKeyboard)] = "keyboard",
      [static_cast<uint32_t>(hid_input_report::DeviceType::kConsumerControl)] = "consumer-control",
  };
#pragma GCC diagnostic pop

  std::string device_types;
  for (size_t i = 0; i < devices_.size(); i++) {
    if (i > 0) {
      device_types += ',';
    }

    const auto type = static_cast<uint32_t>(devices_[i]->GetDeviceType());
    if (type >= sizeof(kDeviceTypeNames) || !kDeviceTypeNames[type]) {
      device_types += "unknown";
    } else {
      device_types += kDeviceTypeNames[type];
    }
  }

  return device_types;
}

void InputReport::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                        GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
}

void InputReport::GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                                          GetInputReportsReaderV2Completer::Sync& completer) {
  uint16_t max_unacknowledged =
      std::clamp<uint16_t>(request->max_unacknowledged_reports_limit, 1, kHalfSecondReportCount);
  if (request->max_unacknowledged_reports_limit != max_unacknowledged) {
    fdf::warn("GetInputReportsReaderV2: requested limit {} clamped to {}",
              request->max_unacknowledged_reports_limit, max_unacknowledged);
  }

  std::unique_ptr<InputReportsReaderV2> reader = std::make_unique<InputReportsReaderV2>(
      this, next_reader_id_++, std::move(request->reader), max_unacknowledged);

  SendInitialConsumerControlReport(reader.get());
  readers_v2_list_.push_back(std::move(reader));

  sync_completion_signal(&next_reader_wait_);
  completer.Reply(max_unacknowledged);
}

void InputReport::GetDescriptor(GetDescriptorCompleter::Sync& completer) {
  fidl::Arena<kFidlDescriptorBufferSize> descriptor_allocator;
  fuchsia_input_report::wire::DeviceDescriptor descriptor(descriptor_allocator);

  fidl::Arena<kFidlDescriptorBufferSize> descriptor_info_allocator;
  fuchsia_input_report::wire::DeviceInformation fidl_info(descriptor_info_allocator);
  {
    fidl::WireResult result = input_device_->Query();
    if (!result.ok()) {
      fdf::error("GetDescriptor: FIDL failed to Query: {}", result.FormatDescription());
      completer.Close(result.status());
      return;
    }
    if (result->is_error()) {
      fdf::error("GetDescriptor: Failed to Query: {}", result->error_value());
      completer.Close(result->error_value());
      return;
    }
    if (result.value()->info.has_vendor_id()) {
      fidl_info.set_vendor_id(result.value()->info.vendor_id());
    }
    if (result.value()->info.has_product_id()) {
      fidl_info.set_product_id(result.value()->info.product_id());
    }
    if (result.value()->info.has_version()) {
      fidl_info.set_version(result.value()->info.version());
    }
    if (result.value()->info.has_polling_rate()) {
      fidl_info.set_polling_rate(descriptor_allocator, result.value()->info.polling_rate());
    }
    if (result.value()->info.has_manufacturer_name()) {
      std::string_view manufacturer_name = result.value()->info.manufacturer_name().get();
      fidl_info.set_manufacturer_name(descriptor_allocator,
                                      fidl::StringView(descriptor_allocator, manufacturer_name));
    }
    if (result.value()->info.has_product_name()) {
      std::string_view product_name = result.value()->info.product_name().get();
      fidl_info.set_product_name(descriptor_allocator,
                                 fidl::StringView(descriptor_allocator, product_name));
    }
    if (result.value()->info.has_serial_number()) {
      std::string_view serial_number = result.value()->info.serial_number().get();
      fidl_info.set_serial_number(descriptor_allocator,
                                  fidl::StringView(descriptor_allocator, serial_number));
    }
  }
  descriptor.set_device_information(descriptor_allocator, fidl_info);

  if (sensor_count_) {
    fidl::VectorView<fuchsia_input_report::wire::SensorInputDescriptor> input(descriptor_allocator,
                                                                              sensor_count_);
    fuchsia_input_report::wire::SensorDescriptor sensor(descriptor_allocator);
    sensor.set_input(descriptor_allocator, std::move(input));
    descriptor.set_sensor(descriptor_allocator, std::move(sensor));
  }

  for (auto& device : devices_) {
    device->CreateDescriptor(descriptor_allocator, descriptor);
  }

  completer.Reply(descriptor);
  {
    fidl::Status result = completer.result_of_reply();
    if (result.status() != ZX_OK) {
      fdf::error("GetDescriptor: Failed to send descriptor: {}", result.FormatDescription());
    }
  }
}

void InputReport::SendOutputReport(SendOutputReportRequestView request,
                                   SendOutputReportCompleter::Sync& completer) {
  uint8_t hid_report[fhidbus::wire::kMaxReportLen];
  size_t size;
  hid_input_report::ParseResult result = hid_input_report::ParseResult::kNotImplemented;
  for (auto& device : devices_) {
    result = device->SetOutputReport(&request->report, hid_report, sizeof(hid_report), &size);
    if (result == hid_input_report::ParseResult::kOk) {
      break;
    }
    // Returning an error other than kParseNotImplemented means the device was supposed
    // to set the Output report but hit an error. When this happens we return the error.
    if (result != hid_input_report::ParseResult::kNotImplemented) {
      break;
    }
  }
  if (result != hid_input_report::ParseResult::kOk) {
    completer.ReplyError(ZX_ERR_INTERNAL);
    return;
  }

  fidl::WireResult status =
      input_device_->SetReport(fhidbus::ReportType::kOutput, hid_report[0],
                               fidl::VectorView<uint8_t>::FromExternal(hid_report, size));
  if (!status.ok()) {
    completer.ReplyError(status.status());
    return;
  }
  completer.ReplySuccess();
}

void InputReport::GetFeatureReport(GetFeatureReportCompleter::Sync& completer) {
  fidl::Arena<kFidlDescriptorBufferSize> allocator;
  fuchsia_input_report::wire::FeatureReport report(allocator);

  for (auto& device : devices_) {
    if (!device->FeatureReportId().has_value()) {
      continue;
    }

    fidl::WireResult feature_report_result =
        input_device_->GetReport(fhidbus::ReportType::kFeature, *device->FeatureReportId());
    if (!feature_report_result.ok()) {
      fdf::error("FIDL GetReport failed {}", feature_report_result.FormatDescription());
      completer.ReplyError(feature_report_result.status());
      return;
    }
    if (feature_report_result->is_error()) {
      fdf::error("GetReport failed {}", zx_status_get_string(feature_report_result->error_value()));
      completer.ReplyError(feature_report_result->error_value());
      return;
    }
    const fidl::VectorView<uint8_t>& feature_report = feature_report_result.value()->report;

    auto result =
        device->ParseFeatureReport(feature_report.data(), feature_report.size(), allocator, report);
    if (result != hid_input_report::ParseResult::kOk &&
        result != hid_input_report::ParseResult::kNotImplemented) {
      fdf::error("ParseFeatureReport failed with {}", static_cast<unsigned int>(result));
      completer.ReplyError(ZX_ERR_INTERNAL);
      return;
    }
  }

  completer.ReplySuccess(std::move(report));
  fidl::Status result = completer.result_of_reply();
  if (result.status() != ZX_OK) {
    fdf::error("Failed to get feature report: {}", result.FormatDescription());
  }
}

void InputReport::SetFeatureReport(SetFeatureReportRequestView request,
                                   SetFeatureReportCompleter::Sync& completer) {
  bool found = false;
  for (auto& device : devices_) {
    if (!device->FeatureReportId().has_value()) {
      continue;
    }

    uint8_t hid_report[fhidbus::wire::kMaxReportLen];
    size_t size;
    auto result = device->SetFeatureReport(&request->report, hid_report, sizeof(hid_report), &size);
    if (result == hid_input_report::ParseResult::kNotImplemented ||
        result == hid_input_report::ParseResult::kItemNotFound) {
      continue;
    }
    if (result != hid_input_report::ParseResult::kOk) {
      fdf::error("SetFeatureReport failed with {}", static_cast<unsigned int>(result));
      completer.ReplyError(ZX_ERR_INTERNAL);
      return;
    }
    fidl::WireResult set_report_result =
        input_device_->SetReport(fhidbus::ReportType::kFeature, *device->FeatureReportId(),
                                 fidl::VectorView<uint8_t>::FromExternal(hid_report, size));
    if (set_report_result.ok()) {
      fdf::error("SetReport failed with {}", set_report_result.FormatDescription());
      completer.ReplyError(set_report_result.status());
      return;
    }
    found = true;
  }

  if (!found) {
    completer.ReplyError(ZX_ERR_INTERNAL);
    return;
  }
  completer.ReplySuccess();
  fidl::Status result = completer.result_of_reply();
  if (result.status() != ZX_OK) {
    fdf::error("Failed to set feature report: {}", result.FormatDescription());
  }
}

void InputReport::GetInputReport(GetInputReportRequestView request,
                                 GetInputReportCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(false);
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t InputReport::Start() {
  auto result = input_device_->GetReportDesc();
  if (!result.ok()) {
    return result.status();
  }

  hid::DeviceDescriptor* dev_desc = nullptr;
  hid::ParseResult parse_res =
      hid::ParseReportDescriptor(result->desc.data(), result->desc.size(), &dev_desc);
  if (parse_res != hid::ParseResult::kParseOk) {
    fdf::error("hid-parser: parsing report descriptor failed with error {}",
               static_cast<int>(parse_res));
    return ZX_ERR_INTERNAL;
  }
  auto free_desc = fit::defer([dev_desc]() { hid::FreeDeviceDescriptor(dev_desc); });

  auto count = dev_desc->rep_count;
  if (count == 0) {
    fdf::error("No report descriptors found");
    return ZX_ERR_INTERNAL;
  }

  // Parse each input report.
  for (size_t rep = 0; rep < count; rep++) {
    const hid::ReportDescriptor* desc = &dev_desc->report[rep];
    if (!ParseHidInputReportDescriptor(desc)) {
      continue;
    }
  }

  // If we never parsed a single device correctly then fail.
  if (devices_.size() == 0) {
    fdf::error("Can't process HID report descriptor for, all parsing attempts failed.");
    return ZX_ERR_INTERNAL;
  }

  // Register to listen to HID reports.
  auto [client, server] = fidl::Endpoints<finput::DeviceReportsReader>::Create();
  fidl::WireResult get_device_reports_reader_result =
      input_device_->GetDeviceReportsReader(std::move(server));
  if (!get_device_reports_reader_result.ok()) {
    fdf::error("Failed to get device reports reader: {}",
               get_device_reports_reader_result.FormatDescription());
    return get_device_reports_reader_result.status();
  }
  dev_reader_.Bind(std::move(client), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  dev_reader_->ReadReports().Then(fit::bind_member<&InputReport::HandleReports>(this));

  const std::string device_types = GetDeviceTypesString();

  root_ = inspector_.GetRoot().CreateChild("hid-input-report-" + device_types);
  latency_histogram_usecs_ = root_.CreateExponentialUintHistogram(
      "latency_histogram_usecs", kLatencyFloor.to_usecs(), kLatencyInitialStep.to_usecs(),
      kLatencyStepMultiplier, kLatencyBucketCount);
  average_latency_usecs_ = root_.CreateUint("average_latency_usecs", 0);
  max_latency_usecs_ = root_.CreateUint("max_latency_usecs", 0);
  device_types_ = root_.CreateString("device_types", device_types);
  total_report_count_ = root_.CreateUint("total_report_count", 0);
  last_event_timestamp_ = root_.CreateUint("last_event_timestamp", 0);

  return ZX_OK;
}

}  // namespace hid_input_report_dev
