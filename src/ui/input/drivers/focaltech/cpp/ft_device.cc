// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ft_device.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/inplace_vector.h>
#include <lib/zx/clock.h>
#include <lib/zx/process.h>
#include <lib/zx/profile.h>
#include <lib/zx/time.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <algorithm>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <hwreg/bitfields.h>

#include "lib/device-protocol/display-panel.h"
#include "src/devices/i2c/lib/i2c-channel/i2c-channel.h"

namespace ft {

namespace {

constexpr fuchsia_input_report::wire::Axis XYAxis(int64_t max) {
  return {
      .range = {.min = 0, .max = max},
      .unit =
          {
              .type = fuchsia_input_report::wire::UnitType::kOther,
              .exponent = 0,
          },
  };
}

constexpr size_t kFt3x27XMax = 600;
constexpr size_t kFt3x27YMax = 1024;

constexpr size_t kFt6336XMax = 480;
constexpr size_t kFt6336YMax = 800;

constexpr size_t kFt5726XMax = 800;
constexpr size_t kFt5726YMax = 1280;

constexpr size_t kFt5336XMax = 1080;
constexpr size_t kFt5336YMax = 1920;

}  // namespace

void FtDevice::FtInputReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<fuchsia_input_report::wire::ContactInputReport> contact_rpt(allocator,
                                                                               contacts.size());
  for (size_t i = 0; i < contacts.size(); i++) {
    contact_rpt[i] = fuchsia_input_report::wire::ContactInputReport::Builder(allocator)
                         .contact_id(contacts[i].finger_id)
                         .position_x(contacts[i].x)
                         .position_y(contacts[i].y)
                         .Build();
  }

  auto touch_report =
      fuchsia_input_report::wire::TouchInputReport::Builder(allocator).contacts(contact_rpt);
  input_report.event_time(event_time.get()).touch(touch_report.Build());
}

FtDevice::FtInputReport FtDevice::ParseReport(std::span<const uint8_t> buf) {
  FtInputReport report;
  const int contact_count = std::min(int{buf[0]}, int{report.contacts.capacity()});
  for (int i = 0; i < contact_count; i++) {
    const int touch_record_offset = 1 + (i * kTouchRecordSize);
    const TouchRecord* touch_record =
        reinterpret_cast<const TouchRecord*>(&buf[touch_record_offset]);
    if (touch_record->event_type() != FtTouchEventType::kContact) {
      continue;
    }
    report.contacts.push_back(FtInputReport::Contact{
        .finger_id = touch_record->touch_id(),
        .x = touch_record->x(),
        .y = touch_record->y(),
    });
  }
  return report;
}

void FtDevice::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                         const zx_packet_interrupt_t* interrupt) {
  if (status != ZX_OK) {
    if (status != ZX_ERR_CANCELED) {
      fdf::error("Interrupt error: {}", zx_status_get_string(status));
    }
    return;
  }
  TRACE_DURATION("input", "FtDevice Read");
  std::array<uint8_t, (kMaxPoints * kTouchRecordSize) + 1> read_buf;
  zx::result result = Read(FTS_REG_CURPOINT, read_buf);
  if (result.is_ok()) {
    auto timestamp = zx::time(interrupt->timestamp);
    auto report = ParseReport(read_buf);
    report.event_time = timestamp;
    readers_.SendReportToAllReaders(report);

    const zx::duration latency = zx::clock::get_monotonic() - timestamp;

    total_latency_ += latency;
    report_count_++;
    average_latency_usecs_.Set(total_latency_.to_usecs() / report_count_);

    if (latency > max_latency_) {
      max_latency_ = latency;
      max_latency_usecs_.Set(max_latency_.to_usecs());
    }

    if (read_buf[0] > 0) {
      total_report_count_.Add(1);
      last_event_timestamp_.Set(timestamp.get());
    }
  } else {
    fdf::error("Failed to read i2c: {}", result);
  }

  irq_.ack();
}

FtDevice::FtDevice() : fdf::DriverBase2(kDriverName) {}

zx::result<> FtDevice::Start(fdf::DriverContext context) {
  auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result i2c = i2c::I2cChannel::FromIncoming(*incoming_ptr, "i2c");
  if (i2c.is_error()) {
    fdf::error("Failed to connect to i2c: {}", i2c);
    return i2c.take_error();
  }
  i2c_ = std::move(i2c.value());

  zx::result int_gpio_client =
      incoming_ptr->Connect<fuchsia_hardware_gpio::Service::Device>("gpio-int");
  if (int_gpio_client.is_error()) {
    fdf::error("Failed to connect to interrupt gpio: {}", int_gpio_client);
    return zx::error(ZX_ERR_NO_RESOURCES);
  }
  int_gpio_.Bind(std::move(int_gpio_client.value()));

  zx::result reset_gpio_client =
      incoming_ptr->Connect<fuchsia_hardware_gpio::Service::Device>("gpio-reset");
  if (reset_gpio_client.is_error()) {
    fdf::error("Failed to connect to reset gpio: {}", reset_gpio_client);
    return zx::error(ZX_ERR_NO_RESOURCES);
  }
  reset_gpio_.Bind(std::move(reset_gpio_client.value()));

  {
    fidl::WireResult result = int_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
    if (!result.ok()) {
      fdf::error("Failed to send SetBufferMode request to int gpio: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to configure int gpio to input: {}",
                 zx_status_get_string(result->error_value()));
      return result->take_error();
    }
  }

  {
    fidl::Arena arena;
    auto config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                      .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                      .Build();
    fidl::WireResult result = int_gpio_->ConfigureInterrupt(config);
    if (!result.ok()) {
      fdf::error("Failed to send ConfigureInterrupt request to int gpio: {}",
                 result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to configure int gpio: {}", zx_status_get_string(result->error_value()));
      return result->take_error();
    }
  }

  fidl::WireResult interrupt = int_gpio_->GetInterrupt({});
  if (!interrupt.ok()) {
    fdf::error("Failed to send GetInterrupt request to int gpio: {}", interrupt.status_string());
    return zx::error(interrupt.status());
  }
  if (interrupt->is_error()) {
    fdf::error("Failed to get interrupt from int gpio: {}",
               zx_status_get_string(interrupt->error_value()));
    return interrupt->take_error();
  }
  irq_ = std::move(interrupt.value()->interrupt);
  irq_handler_.set_object(irq_.get());
  irq_handler_.Begin(dispatcher());

  zx::result pdev_client =
      incoming_ptr->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev(std::move(pdev_client.value()));

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_input_focaltech::Metadata>();
  if (metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata);
    return metadata.take_error();
  }
  const fuchsia_hardware_input_focaltech::Metadata& device_info = metadata.value();
  if (!device_info.device_id().has_value()) {
    fdf::error("Metadata missing `device_id` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (!device_info.needs_firmware().has_value()) {
    fdf::error("Metadata missing `needs_firmware` field");
    return zx::error(ZX_ERR_INTERNAL);
  }

  display::PanelType panel_type = display::PanelType::kUnknown;
  zx::result panel_type_result = compat::GetMetadata<display::PanelType>(
      incoming_ptr, DEVICE_METADATA_DISPLAY_PANEL_TYPE, "pdev");
  if (panel_type_result.is_ok()) {
    panel_type = *panel_type_result.value();
  } else if (panel_type_result.status_value() == ZX_ERR_NOT_FOUND) {
    fdf::warn("Display panel type information not found.");
  } else {
    fdf::error("Failed to get panel type: {}", panel_type_result);
    return panel_type_result.take_error();
  }
  fuchsia_hardware_input_focaltech::DeviceId device_id = device_info.device_id().value();
  switch (device_id) {
    case fuchsia_hardware_input_focaltech::DeviceId::kFt3X27:
      x_max_ = kFt3x27XMax;
      y_max_ = kFt3x27YMax;
      break;
    case fuchsia_hardware_input_focaltech::DeviceId::kFt6336:
      x_max_ = kFt6336XMax;
      y_max_ = kFt6336YMax;
      break;
    case fuchsia_hardware_input_focaltech::DeviceId::kFt5726:
      x_max_ = kFt5726XMax;
      y_max_ = kFt5726YMax;
      break;
    case fuchsia_hardware_input_focaltech::DeviceId::kFt5336:
      // Currently we assume the panel to be always Khadas TS050. If this changes,
      // we may need extra information from the metadata to determine which HID
      // report descriptor to use.
      x_max_ = kFt5336XMax;
      y_max_ = kFt5336YMax;
      break;
    default:
      fdf::error("Unkown device ID {}", static_cast<uint32_t>(device_id));
      return zx::error(ZX_ERR_INTERNAL);
  }

  // Reset the chip -- should be low for at least 1ms, and the chip should take at most 200ms to
  // initialize.
  {
    fidl::WireResult result =
        reset_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputLow);
    if (!result.ok()) {
      fdf::error("Failed to send SetBufferMode request to reset gpio: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to configure reset gpio to output: {}",
                 zx_status_get_string(result->error_value()));
      return result->take_error();
    }
  }
  zx::nanosleep(zx::deadline_after(zx::msec(5)));
  {
    fidl::WireResult result =
        reset_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    if (!result.ok()) {
      fdf::error("Failed to send Write request to reset gpio: {}", result.status_string());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to write to reset gpio: {}", zx_status_get_string(result->error_value()));
      return result->take_error();
    }
  }
  zx::nanosleep(zx::deadline_after(zx::msec(200)));

  zx_status_t status = UpdateFirmwareIfNeeded(device_info, panel_type);
  if (status != ZX_OK) {
    fdf::error("Failed to update firmware: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  node_ = inspector_.GetRoot().CreateChild("Chip_info");
  LogRegisterValue(FTS_REG_TYPE, "TYPE");
  LogRegisterValue(FTS_REG_FIRMID, "FIRMID");
  LogRegisterValue(FTS_REG_VENDOR_ID, "VENDOR_ID");
  LogRegisterValue(FTS_REG_PANEL_ID, "PANEL_ID");
  LogRegisterValue(FTS_REG_RELEASE_ID_HIGH, "RELEASE_ID_HIGH");
  LogRegisterValue(FTS_REG_RELEASE_ID_LOW, "RELEASE_ID_LOW");
  LogRegisterValue(FTS_REG_IC_VERSION, "IC_VERSION");

  node_.CreateBool("needs_firmware_update", device_info.needs_firmware().value(), &values_);
  node_.CreateUint("panel_type", static_cast<uint32_t>(panel_type), &values_);
  fdf::info("Panel type: {}", static_cast<uint32_t>(panel_type));

  // These names must match the strings in //src/diagnostics/config/sampler/input.json.
  metrics_root_ = inspector_.GetRoot().CreateChild("hid-input-report-touch");
  average_latency_usecs_ = metrics_root_.CreateUint("average_latency_usecs", 0);
  max_latency_usecs_ = metrics_root_.CreateUint("max_latency_usecs", 0);
  total_report_count_ = metrics_root_.CreateUint("total_report_count", 0);
  last_event_timestamp_ = metrics_root_.CreateUint("last_event_timestamp", 0);

  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs({
      .connector = std::move(connector.value()),
      .class_name = "input-report",
      .connector_supports = fuchsia_device_fs::ConnectionType::kController,
  });

  zx::result child = AddOwnedChild(kChildNodeName, devfs);
  if (child.is_error()) {
    fdf::error("Failed to create child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void FtDevice::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                     GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
}

void FtDevice::GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                                       GetInputReportsReaderV2Completer::Sync& completer) {
  const uint16_t max_unacknowledged_reports =
      std::clamp<uint16_t>(request->max_unacknowledged_reports_limit, 1, kHalfSecondReportCount);
  if (request->max_unacknowledged_reports_limit != max_unacknowledged_reports) {
    fdf::warn("GetInputReportsReaderV2: requested limit {} clamped to {}",
              request->max_unacknowledged_reports_limit, max_unacknowledged_reports);
  }
  zx_status_t status =
      readers_.CreateReaderV2(dispatcher(), std::move(request->reader), max_unacknowledged_reports);
  if (status != ZX_OK) {
    fdf::error("Failed to create a reader v2 {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }
  completer.Reply(max_unacknowledged_reports);
}

void FtDevice::GetDescriptor(GetDescriptorCompleter::Sync& completer) {
  fidl::Arena<kFeatureAndDescriptorBufferSize> allocator;

  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(static_cast<uint32_t>(
      fuchsia_input_report::wire::VendorGoogleProductId::kFocaltechTouchscreen));
  device_info.manufacturer_name(allocator, "FocalTech");
  device_info.product_name(allocator, "FocalTech Touchscreen");

  fidl::VectorView<fuchsia_input_report::wire::ContactInputDescriptor> contacts(allocator,
                                                                                kMaxPoints);
  for (auto& c : contacts) {
    c = fuchsia_input_report::wire::ContactInputDescriptor::Builder(allocator)
            .position_x(XYAxis(x_max_))
            .position_y(XYAxis(y_max_))
            .Build();
  }

  const auto input = fuchsia_input_report::wire::TouchInputDescriptor::Builder(allocator)
                         .touch_type(fuchsia_input_report::wire::TouchType::kTouchscreen)
                         .max_contacts(kMaxPoints)
                         .contacts(contacts)
                         .Build();

  const auto touch =
      fuchsia_input_report::wire::TouchDescriptor::Builder(allocator).input(input).Build();

  const auto descriptor = fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator)
                              .device_information(device_info.Build())
                              .touch(touch)
                              .Build();

  completer.Reply(descriptor);
}

// simple i2c read for reading one register location
//  intended mostly for debug purposes
zx::result<uint8_t> FtDevice::Read(uint8_t addr) {
  std::array<uint8_t, 1> rbuf;
  zx::result result = i2c_.WriteReadSync(std::array<uint8_t, 1>{addr}, rbuf);
  if (result.is_error()) {
    fdf::error("Failed to write and read: {}", result);
    return result.take_error();
  }
  return zx::ok(rbuf[0]);
}

zx::result<> FtDevice::Read(uint8_t addr, std::span<uint8_t> dst) {
  // TODO(bradenkell): Remove this workaround when transfers of more than 8 bytes are supported on
  // the MT8167.
  size_t offset = 0;

  while (offset < dst.size()) {
    const size_t remaining = dst.size() - offset;
    const size_t transfer_size = std::min(remaining, kMaxI2cTransferLength);
    const std::array<uint8_t, 1> write_data = {static_cast<uint8_t>(addr + offset)};

    zx::result result = i2c_.WriteReadSync(write_data, dst.subspan(offset, transfer_size));
    if (result.is_error()) {
      fdf::error("Failed to read i2c: {}", result);
      return result.take_error();
    }

    offset += transfer_size;
  }

  return zx::ok();
}

void FtDevice::LogRegisterValue(uint8_t addr, std::string_view name) {
  zx::result result = Read(addr);
  if (result.is_ok()) {
    uint8_t value = result.value();
    node_.CreateByteVector(name, {&value, sizeof(value)}, &values_);
    fdf::info("  {:16}: {:#02x}", name, value);
  } else {
    node_.CreateString(name, "error", &values_);
    fdf::error("  {:16}: error {}", name, result);
  }
}

void FtDevice::DevfsConnect(fidl::ServerEnd<fuchsia_input_report::InputDevice> server) {
  bindings_.AddBinding(dispatcher(), std::move(server), this, fidl::kIgnoreBindingClosure);
}

}  // namespace ft

FUCHSIA_DRIVER_EXPORT2(ft::FtDevice);
