// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_MCU_DRIVERS_VIM3_MCU_VIM3_MCU_H_
#define SRC_DEVICES_MCU_DRIVERS_VIM3_MCU_VIM3_MCU_H_

#include <fidl/fuchsia.hardware.fan/cpp/fidl.h>
#include <lib/ddk/device.h>
#include <lib/inspect/cpp/inspect.h>
#include <threads.h>

#include <optional>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>
#include <fbl/mutex.h>

#include "src/devices/i2c/lib/i2c-channel-legacy/i2c-channel.h"

// STM8S003 MCU specific reg definitions
// dl.khadas.com/Hardware/VIM3/MCU/VIM3_MCU_REG_EN.pdf for reg details
#if 0
#define STM_MCU_CHIP_ADDR 0x18
// RO
#define STM_MCU_REG_PASSWD_VEN 0x00
#define STM_MCU_REG_MAC 0x06
#define STM_MCU_REG_USID 0x0c
#define STM_MCU_REG_VERSION 0x12
#define STM_MCU_REG_SHUTDOWN_NORMAL_STATUS_REG 0x86

// RW
#define STM_MCU_REG_BOOT_MODE 0x20

#define STM_MCU_REG_BOOT_EN_RTC 0x22
#define STM_MCU_REG_BOOT_EN_EXP 0x23
#define STM_MCU_REG_BOOT_EN_IR 0x24
#define STM_MCU_REG_BOOT_EN_DCIN 0x25
#define STM_MCU_REG_BOOT_EN_KEY 0x26
#define STM_MCU_REG_LED_SYSTEM_ON_MODE 0x28
#define STM_MCU_REG_LED_SYSTEM_OFF_MODE 0x29
#define STM_MCU_REG_MAC_SWITCH 0x2d

// WO
#define STM_MCU_REG_PWR_OFF_CMD_REG 0x80
#define STM_MCU_REG_PASSWD_START_REG 0x81
#define STM_MCU_REG_CHECK_VEN_PASSWD_REG 0x82
#define STM_MCU_REG_CHECK_USER_PASSWD_REG 0x83
#define STM_MCU_REG_WOL_INIT_START_REG 0x87
#endif

#define STM_MCU_REG_BOOT_EN_WOL 0x21
#define STM_MCU_REG_CMD_FAN_STATUS_CTRL_REG 0x88

#define STM_MCU_REG_EN_WOL_RESET_ENABLE 0x03

#define STM_MCU_USB_PCIE_SWITCH_REG 0x33

// Vim3MCU is an external MCU made by STM used in vim3 for
// fan control and WoL
//
// For now this only sets the fan level on boot

namespace stm {
enum FanLevel {
  FL0,
  FL1,
  FL2,
  FL3,

  MAX,
};
class StmMcu;
using DeviceType = ddk::Device<StmMcu, ddk::Unbindable>;

class StmMcu : public DeviceType, public fidl::WireServer<fuchsia_hardware_fan::Device> {
 public:
  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(StmMcu);
  StmMcu(zx_device_t* parent, ddk::I2cChannel i2c) : DeviceType(parent), i2c_(std::move(i2c)) {}
  ~StmMcu() {}
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  void Init();  // Visible for testing.

  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease();

  // fuchsia_hardware_fan.Device protocol implementation.
  void GetFanLevel(GetFanLevelCompleter::Sync& completer) override {
    if (fan_level_ >= FanLevel::MAX) {
      completer.Reply(ZX_ERR_INTERNAL, fan_level_);
      return;
    }
    completer.Reply(ZX_OK, fan_level_);
  }
  void SetFanLevel(SetFanLevelRequestView request, SetFanLevelCompleter::Sync& completer) override {
    if (request->fan_level >= FanLevel::MAX) {
      completer.Reply(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    completer.Reply(SetFanLevel(static_cast<FanLevel>(request->fan_level)));
  }
  void GetClientType(GetClientTypeCompleter::Sync& completer) override { completer.Reply("fan"); }

  zx::vmo inspect_vmo() const { return inspect_.DuplicateVmo(); }

 private:
  zx_status_t SetFanLevel(FanLevel level);

  ddk::I2cChannel i2c_;
  void ShutDown();
  fbl::Mutex i2c_lock_;
  inspect::Inspector inspect_;

  fidl::ServerBindingGroup<fuchsia_hardware_fan::Device> bindings_;

  FanLevel fan_level_ = FanLevel::MAX;
};

}  // namespace stm
#endif  // SRC_DEVICES_MCU_DRIVERS_VIM3_MCU_VIM3_MCU_H_
