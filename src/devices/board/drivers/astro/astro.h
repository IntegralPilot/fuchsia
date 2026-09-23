// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_ASTRO_ASTRO_H_
#define SRC_DEVICES_BOARD_DRIVERS_ASTRO_ASTRO_H_

#include <fidl/fuchsia.hardware.clockimpl/cpp/wire.h>
#include <fidl/fuchsia.hardware.pinimpl/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/markers.h>
#include <lib/ddk/device.h>
#include <threads.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <ddktl/device.h>
#include <fbl/macros.h>
#include <soc/aml-s905d2/s905d2-gpio.h>

#include "sdk/lib/driver/outgoing/cpp/outgoing_directory.h"
#include "src/devices/board/drivers/astro/astro-btis.h"

namespace astro {

// MAC address metadata indices
enum {
  MACADDR_WIFI = 0,
  MACADDR_BLUETOOTH = 1,
};

// These should match the mmio table defined in astro-i2c.c
enum {
  ASTRO_I2C_A0_0,
  ASTRO_I2C_2,
  ASTRO_I2C_3,
};

// Astro GPIO Pins used for board rev detection
constexpr uint32_t GPIO_HW_ID0 = (S905D2_GPIOZ(7));
constexpr uint32_t GPIO_HW_ID1 = (S905D2_GPIOZ(8));
constexpr uint32_t GPIO_HW_ID2 = (S905D2_GPIOZ(3));

/* Astro I2C Devices */
constexpr uint8_t I2C_BACKLIGHT_ADDR = (0x2C);
constexpr uint8_t I2C_FOCALTECH_TOUCH_ADDR = (0x38);
constexpr uint8_t I2C_AMBIENTLIGHT_ADDR = (0x39);
constexpr uint8_t I2C_AUDIO_CODEC_ADDR = (0x48);
constexpr uint8_t I2C_GOODIX_TOUCH_ADDR = (0x5d);

class Astro;
using AstroType = ddk::Device<Astro>;

// This is the main class for the Astro platform bus driver.
class Astro : public AstroType {
 public:
  explicit Astro(zx_device_t* parent,
                 fdf::ClientEnd<fuchsia_hardware_platform_bus::PlatformBus> pbus)
      : AstroType(parent),
        pbus_(std::move(pbus)),
        outgoing_(fdf::Dispatcher::GetCurrent()->get()) {}

  static zx_status_t Create(void* ctx, zx_device_t* parent);

  // Device protocol implementation.
  void DdkRelease();

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(Astro);

  zx_status_t CreateGpioPlatformDevice();

  void Serve(fdf::ServerEnd<fuchsia_hardware_platform_bus::PlatformBus> request) {
    device_connect_fragment_runtime_protocol(
        parent(), "default", fuchsia_hardware_platform_bus::Service::PlatformBus::ServiceName,
        fuchsia_hardware_platform_bus::Service::PlatformBus::Name, request.TakeChannel().release());
  }

  zx::result<> AdcInit();
  zx_status_t AudioInit();
  zx_status_t BluetoothInit();
  zx_status_t ButtonsInit();
  zx_status_t CanvasInit();
  zx_status_t ClkInit();
  zx_status_t CpuInit();
  zx_status_t GpioInit();
  zx_status_t I2cInit();
  zx_status_t LightInit();
  zx_status_t MaliInit();
  zx_status_t PowerInit();
  zx_status_t PwmInit();
  zx_status_t RamCtlInit();
  zx_status_t RawNandInit();
  zx_status_t RegistersInit();
  zx_status_t SdioInit();
  zx_status_t SecureMemInit();
  zx_status_t Start();
  zx_status_t TeeInit();
  zx_status_t ThermalInit();
  zx_status_t ThermistorInit();
  zx_status_t UsbInit();
  zx_status_t VideoInit();
  zx_status_t AddPostInitDevice();
  int Thread();

  zx_status_t EnableWifi32K(void);
  zx_status_t SdEmmcConfigurePortB(void);

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

  thrd_t thread_;

  fdf::OutgoingDirectory outgoing_;
};

}  // namespace astro

#endif  // SRC_DEVICES_BOARD_DRIVERS_ASTRO_ASTRO_H_
