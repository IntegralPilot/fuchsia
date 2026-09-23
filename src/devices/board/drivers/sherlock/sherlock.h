// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_SHERLOCK_SHERLOCK_H_
#define SRC_DEVICES_BOARD_DRIVERS_SHERLOCK_SHERLOCK_H_

#include <fidl/fuchsia.hardware.clockimpl/cpp/wire.h>
#include <fidl/fuchsia.hardware.pinimpl/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <lib/ddk/device.h>
#include <zircon/types.h>

#include <ddktl/device.h>
#include <fbl/macros.h>
#include <soc/aml-t931/t931-hw.h>

#include "sdk/lib/driver/outgoing/cpp/outgoing_directory.h"
#include "src/devices/board/drivers/sherlock/sherlock-btis.h"
namespace sherlock {

// MAC address metadata indices
enum {
  MACADDR_WIFI = 0,
  MACADDR_BLUETOOTH = 1,
};

// These should match the mmio table defined in sherlock-i2c.c
enum {
  SHERLOCK_I2C_A0_0,
  SHERLOCK_I2C_2,
  SHERLOCK_I2C_3,
};

// These should match the mmio table defined in sherlock-spi.c
enum { SHERLOCK_SPICC0, SHERLOCK_SPICC1 };

class Sherlock;
using SherlockType = ddk::Device<Sherlock, ddk::Initializable>;

// This is the main class for the platform bus driver.
class Sherlock : public SherlockType {
 public:
  explicit Sherlock(zx_device_t* parent,
                    fdf::ClientEnd<fuchsia_hardware_platform_bus::PlatformBus> pbus)
      : SherlockType(parent),
        pbus_(std::move(pbus)),
        outgoing_(fdf::Dispatcher::GetCurrent()->get()) {}

  static zx_status_t Create(void* ctx, zx_device_t* parent);

  // Device protocol implementation.
  void DdkInit(ddk::InitTxn txn);
  void DdkRelease();

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(Sherlock);

  zx_status_t CreateGpioPlatformDevice();

  void Serve(fdf::ServerEnd<fuchsia_hardware_platform_bus::PlatformBus> request) {
    device_connect_fragment_runtime_protocol(
        parent(), "default", fuchsia_hardware_platform_bus::Service::PlatformBus::ServiceName,
        fuchsia_hardware_platform_bus::Service::PlatformBus::Name, request.TakeChannel().release());
  }

  zx_status_t Start();
  zx::result<> AdcInit();
  zx_status_t GpioInit();
  zx_status_t RegistersInit();
  zx_status_t CanvasInit();
  zx_status_t I2cInit();
  zx_status_t SpiInit();
  zx_status_t UsbInit();
  zx_status_t EmmcInit();
  zx_status_t BCM43458LpoClockInit();  // required for BCM43458 wifi/bluetooth chip.
  zx_status_t SdioInit();
  zx_status_t BluetoothInit();
  zx_status_t ClkInit();
  zx_status_t CameraInit();
  zx_status_t MaliInit();
  zx_status_t TeeInit();
  zx_status_t VideoInit();
  zx_status_t VideoEncInit();
  zx_status_t HevcEncInit();
  zx_status_t ButtonsInit();
  zx_status_t AudioInit();
  zx_status_t ThermalInit();
  zx_status_t LightInit();
  zx_status_t OtRadioInit();
  zx_status_t BacklightInit();
  zx_status_t NnaInit();
  zx_status_t SecureMemInit();
  zx_status_t PwmInit();
  zx_status_t RamCtlInit();
  zx_status_t CpuInit();
  zx_status_t ThermistorInit();
  zx_status_t AddPostInitDevice();
  int Thread();

  zx_status_t EnableWifi32K(void);

  static fuchsia_hardware_pinimpl::InitStep GpioPull(uint32_t index,
                                                     fuchsia_hardware_pin::Pull pull) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        index,
        fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
            fuchsia_hardware_pin::Configuration{{.pull = pull}}),
    }});
  }

  static fuchsia_hardware_pinimpl::InitStep GpioOutput(uint32_t index, bool value) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        index,
        fuchsia_hardware_pinimpl::InitCall::WithBufferMode(
            value ? fuchsia_hardware_gpio::BufferMode::kOutputHigh
                  : fuchsia_hardware_gpio::BufferMode::kOutputLow),
    }});
  }

  static fuchsia_hardware_pinimpl::InitStep GpioFunction(uint32_t index, uint64_t function) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        index,
        fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
            fuchsia_hardware_pin::Configuration{{.function = function}}),
    }});
  }

  static fuchsia_hardware_pinimpl::InitStep GpioDriveStrength(uint32_t index, uint64_t ds_ua) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        index,
        fuchsia_hardware_pinimpl::InitCall::WithPinConfig(
            fuchsia_hardware_pin::Configuration{{.drive_strength_ua = ds_ua}}),
    }});
  }

  fuchsia_hardware_clockimpl::wire::InitStep ClockDisable(uint32_t id) {
    return fuchsia_hardware_clockimpl::wire::InitStep::Builder(init_arena_)
        .id(id)
        .call(fuchsia_hardware_clockimpl::wire::InitCall::WithDisable({}))
        .Build();
  }

  fuchsia_hardware_clockimpl::wire::InitStep ClockEnable(uint32_t id) {
    return fuchsia_hardware_clockimpl::wire::InitStep::Builder(init_arena_)
        .id(id)
        .call(fuchsia_hardware_clockimpl::wire::InitCall::WithEnable({}))
        .Build();
  }

  fuchsia_hardware_clockimpl::wire::InitStep ClockSetRate(uint32_t id, uint64_t rate_hz) {
    return fuchsia_hardware_clockimpl::wire::InitStep::Builder(init_arena_)
        .id(id)
        .call(fuchsia_hardware_clockimpl::wire::InitCall::WithRateHz(init_arena_, rate_hz))
        .Build();
  }

  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus> pbus_;
  fidl::Arena<> init_arena_;
  std::vector<fuchsia_hardware_pinimpl::InitStep> gpio_init_steps_;
  std::vector<fuchsia_hardware_clockimpl::wire::InitStep> clock_init_steps_;

  fdf::OutgoingDirectory outgoing_;
};

}  // namespace sherlock

#endif  // SRC_DEVICES_BOARD_DRIVERS_SHERLOCK_SHERLOCK_H_
