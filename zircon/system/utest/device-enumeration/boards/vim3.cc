// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, Vim3DeviceTreeTest) {
  static const char* kNodeMonikers[] = {
      "adc-9000",
      "adc-buttons",
      "arm-mali-0",
      "audio-controller-ff642000.aml-g12-audio-composite",

      // bt-transport-uart is not included in bootfs on vim3.
      "bt-uart-ffd24000.aml-uart",
      // TODO(b/291154545): Add bluetooth paths when firmware is publicly available.
      // "bt-uart-ffd24000.aml-uart.bt-transport-uart.bt-hci-broadcom",

      "clock-controller-ff63c000.clocks",
      "clock-controller-ff63c000.clocks.clock-init",
      "display-ff900000",
      "ethernet-phy-ff634000.aml-ethernet.dwmac-ff3f0000.dwmac.Designware-MAC.network-device",
      "ethernet-phy-ff634000.aml-ethernet.dwmac-ff3f0000.dwmac.eth_phy.phy_null_device",
#if !USE_VIM3_DML
      "fuchsia-sysmem",
#endif
      "gpio-buttons.buttons",
      "gpio-controller-ff634400.aml-gpio.gpio-init",
      "gpio-controller-ff634400.aml-gpio.gpio",
      "gpio-controller-ff634400.aml-gpio.gpio.gpio-93.fusb302-22.fusb302",
      "hrtimer-0.aml-hrtimer",
      "i2c-5000",
      "i2c-5000.aml-i2c.i2c.i2c-0-24",
      "i2c-5000.aml-i2c.i2c.i2c-0-24.khadas-mcu-18.vim3-mcu",
      "i2c-5000.aml-i2c.i2c.i2c-0-32.gpio-controller-20.ti-tca6408a.gpio",
      "interrupt-controller-ffc01000",
      "nna-ff100000.aml-nna-child",

      // SDIO
      "mmc-ffe03000.aml-sd-emmc.sdmmc",

      // SD card
      "mmc-ffe05000.aml-sd-emmc.sdmmc",

      // EMMC
      "mmc-ffe07000.aml-sd-emmc",
      "mmc-ffe07000.aml-sd-emmc.sdmmc",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.boot1",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.boot2",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.rpmb",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.user",

      "usb-phy-ffe09000.aml_usb_phy",
      "usb-phy-ffe09000.aml_usb_phy.dwc2-phy",
      "usb-ff400000.dwc2",
      "usb-ff400000.dwc2.usb-peripheral.function-000.usb-cdc-netdev.network-device",
      "usb-phy-ffe09000.aml_usb_phy.xhci",
      "power-controller.power-impl.power-core.power-0",
      "power-controller.power-impl.power-core.power-0.cpu-controller-0",
      "power-controller.power-impl.power-core.power-1",
      "board",
      "dt-root",
      "suspend.generic-suspend-device",
      "pwm-ffd1b000.aml-pwm-device",
      "pwm-ffd1b000.aml-pwm-device.pwm-0.pwm_a-regulator.vreg-pwm-big",
      "pwm-ffd1b000.aml-pwm-device.pwm-4.pwm-init.aml-pwm-init",
      "pwm-ffd1b000.aml-pwm-device.pwm-9.pwm_a0_d-regulator.vreg-pwm-little",
      "register-controller-1000",
      "temperature-sensor-ff634800.aml-trip-device",
      "temperature-sensor-ff634c00.aml-trip-device",

      "usb-ff500000.xhci",
      // USB 2.0 Hub
      // Ignored because we've had a spate of vim3 devices that seem to have
      // broken or flaky root hubs, and we don't make use of the XHCI bus in
      // any way so we'd rather ignore such failures than cause flakiness or
      // have to remove more devices from the fleet.
      // See b/296738636 for more information.
      // "usb-ff500000.xhci.usb-bus",
      "video-decoder-ffd00000",

#ifdef include_packaged_drivers
      // RTC
      "i2c-5000.aml-i2c.i2c.i2c-0-81.rtc-51.rtc",

      // WLAN
      "mmc-ffe03000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-1.wifi.brcmfmac-wlanphy",

      // GPU
      "gpu-ffe40000.aml-gpu",

      // aml-canvas
      "canvas-ff638000.aml-canvas",

      // display
      "display-ff900000.amlogic-display.display-coordinator",

      "bt-uart-ffd24000.aml-uart.serial",
#endif

  };

  VerifyNodes(kNodeMonikers);
}

}  // namespace
