// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, AstroTest) {
  static const char* kNodeMonikers[] = {
      "astro",
      "astro.post-init",
      "gpio-controller-ff634400.aml-gpio.gpio",
      "gpio-controller-ff634400.aml-gpio.gpio-init",
      "gpio-buttons.buttons",
      "i2c-5000.aml-i2c",
      "i2c-1c000.aml-i2c",
      "i2c-1d000.aml-i2c",
      "gpu-ffe40000.aml-gpu",
      "usb-phy-ffe09000.aml_usb_phy",
      "bt-uart-ffd24000.aml-uart",
      "mmc-ffe05000.aml-sd-emmc.sdmmc",
      "mmc-ffe05000.aml-sd-emmc.sdmmc.sdmmc-sdio",
      "mmc-ffe05000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-1",
      "mmc-ffe05000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-2",
      "pwm-ffd1b000.aml-pwm-device",
      "pwm-ffd1b000.aml-pwm-device.pwm-4.pwm-init.aml-pwm-init",
      "adc-9000.aml-saradc",
      "adc-9000.aml-saradc.adc-0",
      "adc-9000.aml-saradc.adc-1",
      "adc-9000.aml-saradc.ASTRO_ADC_BUTTON",
      "adc-9000.aml-saradc.adc-3",
      "canvas-ff638000.aml-canvas",
      "register-controller-1000",
      "nand-ffe07800.aml-raw_nand",
      "nand-ffe07800.aml-raw_nand.nand.bl2.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.tpl.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.fts.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.factory.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.zircon-b.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.zircon-a.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.zircon-r.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.sys-config.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.migration.skip-block",
      "nand-ffe07800.aml-raw_nand.nand.fvm.ftl",
      "tee-5300000.optee",
      "thermistor",
      "thermistor.thermistor-device",
      "thermistor.thermistor-device.therm-soc",
      "thermistor.thermistor-device.therm-wifi",
      "thermistor.thermistor-device.therm-dsp",
      "thermistor.thermistor-device.therm-ambient",
      "ram-controller-ff638000.ram",
      "temperature-sensor-ff634800",
      "temperature-sensor-ff634c00",
      "temperature-sensor-ff634800.thermal",
      "temperature-sensor-ff634c00.thermal",
      "secure-monitor",
      "secure-monitor.aml-securemem",
      "clock-controller-ff63c000",
      "clock-controller-ff63c000.clocks.clock-init",
      "aml-light",
      "i2c-1c000.aml-i2c.i2c.i2c-2-72.audio-codec-48",
      "i2c-5000.aml-i2c.i2c.i2c-0-57.tcs3400-light-39.tcs-3400",
      "power-controller",
      "power-controller.power-impl.power-core",
      "power-controller.power-impl.power-core.power-0",
      "cpu-controller-0",
      "power-controller.power-impl.power-core.power-0.cpu-controller-0",

#ifdef include_packaged_drivers
      "astro-i2s-audio-out.aml_tdm.astro-audio-i2s-out",
      "astro-audio-pdm-in.aml_pdm.astro-audio-pdm-in",
      "bt-uart-ffd24000.aml-uart.serial.bt-transport-uart",
      "mmc-ffe05000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-1.wifi-1.brcmfmac-wlanphy",
      "aml-light.gpio-light",
      "video-decoder-ffd00000.amlogic_video",
#endif
  };
  VerifyNodes(kNodeMonikers);

  static const char* kTouchscreenNodeMonikers[] = {
      "i2c-1d000.aml-i2c.i2c.i2c-1-56.focaltech-touch-38.focaltouch-HidDevice",
      "i2c-1d000.aml-i2c.i2c.i2c-1-93.goodix-touch-5d.gt92xx-HidDevice",
  };
  VerifyOneOf(kTouchscreenNodeMonikers);

  static const char* kDisplayNodeMonikers[] = {
      "boe-display-ff900000.amlogic-display.display-coordinator",
      "innolux-display-ff900000.amlogic-display.display-coordinator",
  };
  VerifyOneOf(kDisplayNodeMonikers);

  static const char* kBacklightNodeMonikers[] = {
      "i2c-1c000.aml-i2c.i2c.i2c-2-44.backlight-boe-2c",
      "i2c-1c000.aml-i2c.i2c.i2c-2-44.backlight-innolux-2c",
  };
  VerifyOneOf(kBacklightNodeMonikers);
}

}  // namespace
