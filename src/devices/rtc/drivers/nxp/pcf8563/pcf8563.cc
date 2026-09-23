// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pcf8563.h"

#include <fidl/fuchsia.hardware.i2c/cpp/fidl.h>
#include <fidl/fuchsia.hardware.rtc/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <utility>
#include <vector>

#include "pcf8563-server.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

uint8_t to_bcd(uint8_t binary) {
  return static_cast<uint8_t>((((binary / 10) << 4) | (binary % 10)));
}

uint8_t from_bcd(uint8_t bcd) { return ((bcd >> 4) * 10) + (bcd & 0xf); }

namespace pcf8563 {

namespace {

namespace fi2c = fuchsia_hardware_i2c;
namespace frtc = fuchsia_hardware_rtc;

}  // namespace

zx::result<> RtcDriver::Start(fdf::DriverContext context) {
  zx::result i2c = context.incoming().Connect<fi2c::Service::Device>();
  if (i2c.is_error()) {
    fdf::error("Connect(): {}", i2c);
    return i2c.take_error();
  }
  i2c_.Bind(std::move(i2c.value()));

  server_ = std::make_unique<RtcServer>(this);

  zx::result serve = outgoing()->AddService<frtc::Service>(server_->GetInstanceHandler());
  if (serve.is_error()) {
    fdf::error("AddService(): {}", serve);
    return serve.take_error();
  }

  // Haoyu HYM8563 quirk #1.
  //
  // When the (NXP-compatible) HYM8563 chip resets, it sets the RTC to some random time. This is
  // causing issues with time detection within timekeeper. To rectify, we'll set the time to
  // 1900-01-01T00:00:00 if the chip appears to not have been externally powered during system
  // boot. Otherwise, we'll assume the battery-backed value stored in the RTC is accurate.
  //
  // To detect the external battery state, we'll inspect the TESTC bit, which is reset to 1 on
  // power-reset. The TESTC bit enables an unused feature having to do with bench-testing of the
  // chip, and can/should be disabled by system software.
  zx::result csr = I2cReadRaw(kI2cCsrRegister, 1);
  if (csr.is_error()) {
    return csr.take_error();
  }
  if (csr.value()[0] & 0x08) {  // TESTC (power-on-reset) bit.
    // 1900-01-01T00:00:00
    zx::result reset = I2cWriteRaw({kI2cDateRegister, 0, 0, 0, 1, 0, 1, 0});
    if (reset.is_error()) {
      return reset.take_error();
    }

    // Clear the TESTC bit (all other bits should be cleared). See pcf8563 datasheet 8.11 table27.
    zx::result testc_clear = I2cWriteRaw({kI2cCsrRegister, 0});
    if (testc_clear.is_error()) {
      return testc_clear.take_error();
    }
  }

  // Haoyu HYM8563 quirk #2.
  //
  // In some circumstances, we've seen Get() FIDL calls to the server return nonsensical values.
  // This would imply the TESTC bit was clear at the time the driver started with a bogus date
  // currently written to the chip - a case which shouldn't happen in practice. It's unknown how the
  // chip gets into this state.
  //
  // To recover, the driver resets the time to 1900-01-01T00:00:00 on startup if it detects a
  // nonsensical value is stored.
  zx::result time0 = Read();
  if (time0.is_error()) {
    fdf::error("Read(): {}", time0);
    return time0.take_error();
  }

  if (IsInvalid(time0.value())) {
    fdf::warn("nonsensical startup datetime {}-{}-{}T{}:{}:{}", time0->year(), time0->month(),
              time0->day(), time0->hours(), time0->minutes(), time0->seconds());
    fdf::warn("resetting rtc to 1900-01-01T00:00:00");

    zx::result write = I2cWriteRaw({kI2cDateRegister, 0, 0, 0, 1, 0, 1, 0});
    if (write.is_error()) {
      fdf::error("Write(): {}", write);
      return write.take_error();
    }
  }

  zx::result devfs_node = CreateDevfsNode();
  if (devfs_node.is_error()) {
    fdf::error("CreateDevfsNode(): {}", devfs_node);
    return devfs_node.take_error();
  }

  return zx::ok();
}

zx::result<frtc::Time> RtcDriver::Read() {
  zx::result result = I2cReadRaw(kI2cDateRegister, 7);
  if (result.is_error()) {
    return result.take_error();
  }

  std::vector<uint8_t>& rx_data = result.value();

  frtc::Time time{{
      .seconds = from_bcd(rx_data[0] & 0x7f),
      .minutes = from_bcd(rx_data[1] & 0x7f),
      .hours = from_bcd(rx_data[2] & 0x3f),
      .day = from_bcd(rx_data[3] & 0x3f),
      // .weekday unused.
      .month = from_bcd(rx_data[5] & 0x1f),
      .year = static_cast<uint16_t>(((rx_data[5] & 0x80) ? 2000 : 1900) + from_bcd(rx_data[6])),
  }};

  return zx::ok(time);
}

bool RtcDriver::IsInvalid(const frtc::Time& time) const {
  // The PCF8563 uses 1 BCD-encoded byte for the year (0-99). The century is encoded as the high-bit
  // of the month field whose value represents century_bit ? 2000 : 1900. As a consequence, this
  // chip can only support dates whose year is in the range 1900 through 2099.
  //
  // Additionally, the PCF8563 may also contain nonsensical values for the month, day, hour, minute,
  // or second fields (e.g. a month of 0).

  // The PCF8563 (incorrectly) considers any year divisible by 4 to be a leap year, allowing for a
  // 29th day of February. See 8.4.4, note-1 of table-12 in the datasheet.
  bool is_leap = time.year() % 4 == 0;

  // The max day value depends on both the month and year (e.g. September 31st isn't valid).
  uint8_t max_day;
  switch (time.month()) {
    case 1:
    case 3:
    case 5:
    case 7:
    case 8:
    case 10:
    case 12:
      max_day = 31;
      break;
    case 4:
    case 6:
    case 9:
    case 11:
      max_day = 30;
      break;
    case 2:
      max_day = is_leap ? 29 : 28;
      break;
  }

  // clang-format off
  return (time.year() < 1900 || time.year() > 2099
          || time.month() < 1 || time.month() > 12
          || time.day() < 1 || time.day() > max_day
          || time.hours() > 23
          || time.minutes() > 59
          || time.seconds() > 59);
  // clang-format on
}

zx::result<> RtcDriver::Write(frtc::Time time) {
  int year = time.year();
  uint8_t century_bit = (year < 2000) ? 0 : 1;
  year -= century_bit ? 2000 : 1900;  // Normalize to a value between 0 and 99.

  std::vector<uint8_t> tx_data = {
      0x02,
      to_bcd(time.seconds()),
      to_bcd(time.minutes()),
      to_bcd(time.hours()),
      to_bcd(time.day()),
      0,  // day of week
      static_cast<uint8_t>(century_bit << 7 | to_bcd(time.month())),
      to_bcd(static_cast<uint8_t>(year)),
  };

  return I2cWriteRaw(std::move(tx_data));
}

zx::result<std::vector<uint8_t>> RtcDriver::I2cReadRaw(uint8_t reg, uint8_t rx_size) {
  std::vector<fi2c::Transaction> txns{
      {{.data_transfer = fi2c::DataTransfer::WithWriteData({reg})}},
      {{.data_transfer = fi2c::DataTransfer::WithReadSize(rx_size)}},
  };

  fidl::Result result = i2c_->Transfer({{.transactions = std::move(txns)}});
  if (result.is_error()) {
    fdf::error("i2c_.Transfer(): {}", result.error_value().FormatDescription());
    if (result.error_value().is_framework_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::error(result.error_value().domain_error());
  }

  // Transfer() returns a vector of byte-vectors, one per read-transfer.
  return zx::ok(std::move(result->read_data()[0]));
}

zx::result<> RtcDriver::I2cWriteRaw(std::vector<uint8_t>&& tx_data) {
  std::vector<fi2c::Transaction> txns{
      {{.data_transfer = fi2c::DataTransfer::WithWriteData(std::move(tx_data))}},
  };

  fidl::Result result = i2c_->Transfer({{.transactions = std::move(txns)}});
  if (result.is_error()) {
    fdf::error("i2c_.Transfer(): {}", result.error_value().FormatDescription());
    if (result.error_value().is_framework_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::error(result.error_value().domain_error());
  }

  return zx::ok();
}

void RtcDriver::DevfsConnect(fidl::ServerEnd<frtc::Device> req) {
  server_->bindings().AddBinding(dispatcher(), std::move(req), server_.get(),
                                 fidl::kIgnoreBindingClosure);
}

zx::result<> RtcDriver::CreateDevfsNode() {
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("devfs_connector_.Bind(): {}", connector);
    return connector.take_error();
  }

  fdf::DevfsAddArgs devfs;
  devfs.connector(std::move(connector.value()));
  devfs.class_name("rtc");

  auto controller = AddChild("rtc", devfs, cpp20::span<const fdf::NodeProperty2>{}, {});
  if (controller.is_error()) {
    fdf::error("AddChild(): {}", controller);
    return controller.take_error();
  }

  return zx::ok();
}

}  // namespace pcf8563

FUCHSIA_DRIVER_EXPORT2(pcf8563::RtcDriver);
