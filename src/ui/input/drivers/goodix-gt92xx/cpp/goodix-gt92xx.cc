// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "goodix-gt92xx.h"

#include <endian.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/platform-defs.h>
#include <lib/ddk/trace/event.h>
#include <lib/fit/defer.h>
#include <lib/zx/clock.h>
#include <lib/zx/profile.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <utility>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <fbl/vector.h>

namespace goodix {

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

constexpr size_t kXMax = 600;
constexpr size_t kYMax = 1024;

}  // namespace

void Gt92xxDevice::GtInputReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<fuchsia_input_report::wire::ContactInputReport> contact_rpt(allocator,
                                                                               contact_count);
  for (size_t i = 0; i < contact_count; i++) {
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

// clang-format off
// Configuration data
// first two bytes contain starting register address (part of i2c transaction)
fbl::Vector<uint8_t> Gt92xxDevice::GetConfData() {
  return {GT_REG_CONFIG_DATA >> 8,
              GT_REG_CONFIG_DATA & 0xff,
              0x5f, 0x00, 0x04, 0x58, 0x02, 0x05, 0xbd, 0xc0,
              0x00, 0x08, 0x1e, 0x05, 0x50, 0x32, 0x00, 0x0b,
              0x00, 0x00, 0x00, 0x00, 0x40, 0x12, 0x00, 0x17,
              0x17, 0x19, 0x12, 0x8d, 0x2d, 0x0f, 0x3f, 0x41,
              0xb2, 0x04, 0x00, 0x00, 0x00, 0xbc, 0x03, 0x1d,
              0x1e, 0x80, 0x01, 0x00, 0x14, 0x46, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x37, 0x55, 0x8f, 0xc5, 0x02,
              0x07, 0x11, 0x00, 0x04, 0x8a, 0x39, 0x00, 0x81,
              0x3e, 0x00, 0x78, 0x44, 0x00, 0x71, 0x4a, 0x00,
              0x6a, 0x51, 0x00, 0x6a, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
              0x1c, 0x1a, 0x18, 0x16, 0x14, 0x12, 0x10, 0x0e,
              0x0c, 0x0a, 0x08, 0x06, 0x04, 0x02, 0x00, 0x00,
              0xff, 0xff, 0x1f, 0xe7, 0xff, 0xff, 0xff, 0x0f,
              0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x2a, 0x29,
              0x28, 0x27, 0x26, 0x25, 0x24, 0x23, 0x22, 0x21,
              0x20, 0x1f, 0x1e, 0x0c, 0x0b, 0x0a, 0x09, 0x08,
              0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x6e, 0x01 };
}
// clang-format on

int Gt92xxDevice::Thread() {
  // Format of data as it is read from the device
  struct FingerReport {
    uint8_t id;
    uint16_t x;
    uint16_t y;
    uint16_t size;
    uint8_t reserved;
  } __PACKED;

  zx_status_t status;
  zx::time_boot timestamp;
  zxlogf(INFO, "gt92xx: entering irq thread");
  while (true) {
    status = irq_.wait(&timestamp);
    if (!running_.load()) {
      return ZX_OK;
    }
    if (status != ZX_OK) {
      zxlogf(ERROR, "gt92xx: Interrupt error %d", status);
    }
    TRACE_DURATION("input", "Gt92xxDevice Read");
    uint8_t touch_stat = 0;
    uint8_t retry_cnt = 0;
    // Datasheet implies that it is not guaranteed that report will be
    // ready when interrupt is generated, so allow a couple retries to check
    // touch status.
    while (!(touch_stat & GT_REG_TOUCH_STATUS_READY) && (retry_cnt < 3)) {
      zx::result<uint8_t> read_status = Read(GT_REG_TOUCH_STATUS);
      if (read_status.is_ok()) {
        touch_stat = read_status.value();
      }
      if (!(touch_stat & GT_REG_TOUCH_STATUS_READY)) {
        retry_cnt++;
        zx_nanosleep(zx_deadline_after(ZX_MSEC(1)));
      }
    }

    if (touch_stat & GT_REG_TOUCH_STATUS_READY) {
      uint8_t num_reports = touch_stat & 0x0f;
      FingerReport reports[kMaxPoints];
      // Read touch reports
      zx_status_t status = Read(GT_REG_REPORTS, reinterpret_cast<uint8_t*>(&reports),
                                static_cast<uint8_t>(sizeof(FingerReport) * kMaxPoints));
      // Clear touch status after reading reports
      Write(GT_REG_TOUCH_STATUS, 0);
      if (status == ZX_OK) {
        GtInputReport report;
        report.event_time = timestamp;
        report.contact_count =
            std::min(num_reports, static_cast<uint8_t>(report.contacts.max_size()));
        // We are reusing same HID report as ft3x77 to simplify astro integration
        // so we need to copy from device format to HID structure format
        for (uint32_t i = 0; i < report.contact_count; i++) {
          report.contacts[i].finger_id = reports[i].id;
          report.contacts[i].y = reports[i].x;
          report.contacts[i].x = reports[i].y;
        }
        readers_.SendReportToAllReaders(report);

        const zx::duration latency = zx::clock::get_boot() - timestamp;

        total_latency_ += latency;
        report_count_++;
        average_latency_usecs_.Set(total_latency_.to_usecs() / report_count_);

        if (latency > max_latency_) {
          max_latency_ = latency;
          max_latency_usecs_.Set(max_latency_.to_usecs());
        }

        if (num_reports > 0) {
          total_report_count_.Add(1);
          last_event_timestamp_.Set(timestamp.get());
        }
      }
    } else {
      zxlogf(ERROR, "gt92xx: Errant interrupt, no report ready - %x", touch_stat);
    }
  }
  zxlogf(INFO, "gt92xx: exiting");
  return 0;
}

zx_status_t Gt92xxDevice::Create(zx_device_t* device) {
  zxlogf(INFO, "gt92xx: driver started...");
  ddk::I2cChannel i2c(device, "i2c");
  if (!i2c.is_valid()) {
    zxlogf(ERROR, "failed to acquire i2c");
    return ZX_ERR_NO_RESOURCES;
  }

  const char* kInterruptGpioFragmentName = "gpio-int";
  zx::result int_gpio = DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
      device, kInterruptGpioFragmentName);
  if (int_gpio.is_error()) {
    zxlogf(ERROR, "Failed to get gpio protocol from fragment %s: %s", kInterruptGpioFragmentName,
           int_gpio.status_string());
    return ZX_ERR_NO_RESOURCES;
  }

  const char* kResetGpioFragmentName = "gpio-reset";
  zx::result reset_gpio = DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
      device, kResetGpioFragmentName);
  if (reset_gpio.is_error()) {
    zxlogf(ERROR, "Failed to get gpio protocol from fragment %s: %s", kResetGpioFragmentName,
           reset_gpio.status_string());
    return ZX_ERR_NO_RESOURCES;
  }

  auto goodix_dev = std::make_unique<Gt92xxDevice>(
      device, fdf_dispatcher_get_async_dispatcher(fdf_dispatcher_get_current_dispatcher()),
      std::move(i2c), std::move(int_gpio.value()), std::move(reset_gpio.value()));

  zx_status_t status = goodix_dev->Init();
  if (status != ZX_OK) {
    zxlogf(ERROR, "Could not initialize gt92xx hardware %d", status);
    return status;
  }

  auto thunk = [](void* arg) -> int { return reinterpret_cast<Gt92xxDevice*>(arg)->Thread(); };

  auto cleanup = fit::defer([&]() { goodix_dev->ShutDown(); });

  goodix_dev->running_.store(true);
  int ret = thrd_create_with_name(&goodix_dev->thread_, thunk, goodix_dev.get(), "gt92xx-thread");
  ZX_DEBUG_ASSERT(ret == thrd_success);

  // Set scheduler role for device thread.
  {
    const char* role_name = "fuchsia.ui.input.drivers.goodix-gt92xx.device";
    status =
        device_set_profile_by_role(goodix_dev->parent(), thrd_get_zx_handle(goodix_dev->thread_),
                                   role_name, strlen(role_name));
    if (status != ZX_OK) {
      zxlogf(WARNING, "Gt92xxDevice::Create: Failed to apply role: %s",
             zx_status_get_string(status));
    }
  }

  status = goodix_dev->DdkAdd(ddk::DeviceAddArgs("gt92xx-HidDevice")
                                  .set_inspect_vmo(goodix_dev->inspector_.DuplicateVmo()));
  if (status != ZX_OK) {
    zxlogf(ERROR, "gt92xx: Could not create hid device: %d", status);
    return status;
  } else {
    zxlogf(INFO, "gt92xx: Added hid device");
  }

  cleanup.cancel();

  // device intentionally leaked as it is now held by DevMgr
  [[maybe_unused]] auto ptr = goodix_dev.release();

  return ZX_OK;
}

zx_status_t Gt92xxDevice::Init() {
  // Hardware reset
  HWReset();

  zx_status_t status = UpdateFirmwareIfNeeded();
  if (status != ZX_OK) {
    return status;
  }

  // Get the config data
  fbl::Vector<uint8_t> Conf(GetConfData());

  // Configuration data should span specific set of registers
  // last register has flag to latch in new configuration, second
  // to last register holds checksum of register values.
  // Note: first two bytes of conf_data hold the 16-bit register address where
  // the write will start.
  ZX_DEBUG_ASSERT((Conf.size() - sizeof(uint16_t)) ==
                  (GT_REG_CONFIG_REFRESH - GT_REG_CONFIG_DATA + 1));

  zx::result<uint8_t> version = Read(GT_REG_CONFIG_DATA);
  if (version.is_ok() && version.value() > Conf.data()[sizeof(uint16_t)]) {
    // Force the controller to take the config.
    Conf[sizeof(uint16_t)] = 0x00;
  }

  // Write conf data to registers
  status = Write(Conf.data(), Conf.size());
  if (status != ZX_OK) {
    return status;
  }
  // Device requires 10ms delay to refresh configuration
  zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));
  // Clear touch state in case there were spurious touches registered
  // during startup
  Write(GT_REG_TOUCH_STATUS, 0);

  // Note: Our configuration inverts polarity of interrupt
  // (datasheet implies it is active high)
  {
    fidl::Arena arena;
    auto config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                      .mode(fuchsia_hardware_gpio::InterruptMode::kEdgeLow)
                      .Build();
    fidl::WireResult result = int_gpio_->ConfigureInterrupt(config);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send ConfigureInterrupt request to interrupt gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure interrupt gpio: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  fidl::WireResult interrupt_result = int_gpio_->GetInterrupt({});
  if (!interrupt_result.ok()) {
    zxlogf(ERROR, "Failed to send GetInterrupt request to interrupt gpio: %s",
           interrupt_result.status_string());
    return interrupt_result.status();
  }
  if (interrupt_result->is_error()) {
    zxlogf(ERROR, "Failed to get interrupt from interrupt gpio: %s",
           zx_status_get_string(interrupt_result->error_value()));
    return interrupt_result->error_value();
  }
  irq_ = std::move(interrupt_result.value()->interrupt);

  LogFirmwareStatus();

  metrics_root_ = inspector_.GetRoot().CreateChild("hid-input-report-touch");
  average_latency_usecs_ = metrics_root_.CreateUint("average_latency_usecs", 0);
  max_latency_usecs_ = metrics_root_.CreateUint("max_latency_usecs", 0);
  total_report_count_ = metrics_root_.CreateUint("total_report_count", 0);
  last_event_timestamp_ = metrics_root_.CreateUint("last_event_timestamp", 0);
  return ZX_OK;
}

zx_status_t Gt92xxDevice::HWReset() {
  // Hardware reset will also set the address of the controller to either
  // 0x14 0r 0x5d.  See the datasheet for explanation of sequence.
  {
    // Make reset pin an output and drive low
    fidl::WireResult result =
        reset_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputLow);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request to reset gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure reset gpio to output: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  {
    // Make interrupt pin an output and drive low
    fidl::WireResult result =
        int_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputLow);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request to interrupt gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure interrupt gpio to output: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  // Delay for 100us
  zx_nanosleep(zx_deadline_after(ZX_USEC(100)));
  {
    // Release the reset
    fidl::WireResult result =
        reset_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send Write request to reset gpio: %s", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to write to reset gpio: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  zx_nanosleep(zx_deadline_after(ZX_MSEC(5 + 50)));  // Interrupt still output low
  {
    fidl::WireResult result = int_gpio_->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request to interrupt gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure interrupt gpio to input: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  zx_nanosleep(zx_deadline_after(ZX_MSEC(50)));  // Wait for reset before sending config
  return ZX_OK;
}

void Gt92xxDevice::DdkRelease() { delete this; }

void Gt92xxDevice::DdkUnbind(ddk::UnbindTxn txn) {
  ShutDown();
  txn.Reply();
}

zx_status_t Gt92xxDevice::ShutDown() {
  running_.store(false);
  irq_.destroy();
  thrd_join(thread_, NULL);
  return ZX_OK;
}

void Gt92xxDevice::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                         GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader (v1) is no longer supported; use GetInputReportsReaderV2");
}

void Gt92xxDevice::GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                                           GetInputReportsReaderV2Completer::Sync& completer) {
  const uint16_t max_unacknowledged_reports =
      std::clamp<uint16_t>(request->max_unacknowledged_reports_limit, 1, kHalfSecondReportCount);
  if (request->max_unacknowledged_reports_limit != max_unacknowledged_reports) {
    zxlogf(WARNING, "GetInputReportsReaderV2: requested limit %u clamped to %u",
           request->max_unacknowledged_reports_limit, max_unacknowledged_reports);
  }
  zx_status_t status =
      readers_.CreateReaderV2(dispatcher_, std::move(request->reader), max_unacknowledged_reports);
  if (status != ZX_OK) {
    zxlogf(ERROR, "CreateReaderV2 failed %s", zx_status_get_string(status));
    completer.Close(status);
    return;
  }
  completer.Reply(max_unacknowledged_reports);
}

void Gt92xxDevice::GetDescriptor(GetDescriptorCompleter::Sync& completer) {
  fidl::Arena<kFeatureAndDescriptorBufferSize> allocator;

  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kGoodixTouchscreen));
  device_info.manufacturer_name(allocator, "Goodix");
  device_info.product_name(allocator, "GT92xx Touchscreen");

  fidl::VectorView<fuchsia_input_report::wire::ContactInputDescriptor> contacts(allocator,
                                                                                kMaxPoints);
  for (auto& c : contacts) {
    c = fuchsia_input_report::wire::ContactInputDescriptor::Builder(allocator)
            .position_x(XYAxis(kXMax))
            .position_y(XYAxis(kYMax))
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

zx::result<uint8_t> Gt92xxDevice::Read(uint16_t addr) {
  uint8_t rbuf;
  zx_status_t status = Read(addr, &rbuf, 1);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(rbuf);
}

zx_status_t Gt92xxDevice::Read(uint16_t addr, uint8_t* buf, size_t len) {
  zx_status_t status;
  for (int i = 0; i < kI2cRetries; i++) {
    uint8_t tbuf[2];
    tbuf[0] = static_cast<uint8_t>(addr >> 8);
    tbuf[1] = static_cast<uint8_t>(addr & 0xff);
    if ((status = i2c_.WriteReadSync(tbuf, 2, buf, len)) == ZX_OK) {
      return ZX_OK;
    }
  }
  zxlogf(ERROR, "Failed to read %s 0x%04x: %d", len <= 1 ? "register" : "from", addr, status);
  return status;
}

zx_status_t Gt92xxDevice::Write(uint16_t addr, uint8_t val) {
  zx_status_t status;
  for (int i = 0; i < kI2cRetries; i++) {
    uint8_t tbuf[3];
    tbuf[0] = static_cast<uint8_t>(addr >> 8);
    tbuf[1] = static_cast<uint8_t>(addr & 0xff);
    tbuf[2] = val;
    if ((status = i2c_.WriteSync(tbuf, 3)) == ZX_OK) {
      return ZX_OK;
    }
  }
  zxlogf(ERROR, "Failed to write register 0x%04x: %d", addr, status);
  return status;
}

zx_status_t Gt92xxDevice::Write(uint8_t* buf, size_t len) {
  zx_status_t status;
  for (int i = 0; i < kI2cRetries; i++) {
    if ((status = i2c_.WriteSync(buf, len)) == ZX_OK) {
      return ZX_OK;
    }
  }
  zxlogf(ERROR, "Failed to write to 0x%04x: %d", (buf[0] << 8) | buf[1], status);
  return status;
}

}  // namespace goodix

zx_status_t gt92xx_bind(void* ctx, zx_device_t* device) {
  return goodix::Gt92xxDevice::Create(device);
}

static constexpr zx_driver_ops_t gt92xx_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = gt92xx_bind;
  return ops;
}();

ZIRCON_DRIVER(gt92xx, gt92xx_driver_ops, "zircon", "0.1");
