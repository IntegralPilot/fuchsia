// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_PCF8563_H_
#define SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_PCF8563_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2c/cpp/fidl.h>
#include <fidl/fuchsia.hardware.rtc/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/zx/result.h>

#include <memory>

#include "pcf8563-server.h"

uint8_t to_bcd(uint8_t);
uint8_t from_bcd(uint8_t);

namespace pcf8563 {

constexpr uint8_t kI2cCsrRegister = 0x00;
constexpr uint8_t kI2cDateRegister = 0x02;

class RtcServer;
class RtcDriver : public fdf::DriverBase2 {
 public:
  RtcDriver()
      : fdf::DriverBase2("pcf8563-rtc"),
        devfs_connector_(fit::bind_member<&RtcDriver::DevfsConnect>(this)) {}

  ~RtcDriver() override = default;

  zx::result<> Start(fdf::DriverContext context) override;

  zx::result<fuchsia_hardware_rtc::Time> Read();
  zx::result<> Write(fuchsia_hardware_rtc::Time time);
  zx::result<std::vector<uint8_t>> I2cReadRaw(uint8_t reg, uint8_t rx_size);
  zx::result<> I2cWriteRaw(std::vector<uint8_t>&& tx_data);

  // The PCF8563 performs no validation and will gladly accept (and return) completely nonsensical
  // values. Basic field validation is performed:
  //   1. Year is between 1900 and 2099 (chip constraint).
  //   2. Month is between 1 and 12.
  //   3. Day is a value commensurate with the month (and leap year).
  //   4. Hour is between 0 and 23.
  //   5. Minute is between 0 and 59.
  //   6. Second is between 0 and 59.
  bool IsInvalid(const fuchsia_hardware_rtc::Time& time) const;

 private:
  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_rtc::Device> req);
  zx::result<> CreateDevfsNode();

  fidl::SyncClient<fuchsia_hardware_i2c::Device> i2c_;
  driver_devfs::Connector<fuchsia_hardware_rtc::Device> devfs_connector_;

  std::unique_ptr<RtcServer> server_;
};

}  // namespace pcf8563

#endif  // SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_PCF8563_H_
