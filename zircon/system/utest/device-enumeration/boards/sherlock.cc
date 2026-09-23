// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

namespace {

TEST_F(DeviceEnumerationTest, SherlockTest) {
  static const char* kCommonNodeMonikers[] = {
      "bt-uart-ffd24000.aml-uart",
      "gpio-controller-c-ff634400.aml-gpio.gpio.gpio-50.spi-13000.aml-spi-0.spi.spi-0-0",
      "gpio-controller-ff634400.aml-gpio.gpio",
      "gpio-controller-ff634400.aml-gpio.gpio-init",
      "i2c-5000.aml-i2c",
      "i2c-1c000.aml-i2c",
      "i2c-1d000.aml-i2c",
      "usb-phy-ffe09000.aml_usb_phy",
      "mmc-ffe03000.aml-sd-emmc.sdmmc",
      "mmc-ffe03000.aml-sd-emmc.sdmmc.sdmmc-sdio",
      "mmc-ffe03000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-1",
      "mmc-ffe03000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-2",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.boot1",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.boot2",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.rpmb",
      "mmc-ffe07000.aml-sd-emmc.sdmmc.sdmmc-mmc.user",
      "canvas-ff638000.aml-canvas",
      "gpio-buttons.buttons",
      "ram-controller-ff638000.ram",
      "register-controller-1000",
      "tee-5300000.optee",
      "secure-monitor.aml-securemem",
      "gpu-ffe40000.aml-gpu",
      "clock-controller-ff63c000",
      "clock-controller-ff63c000.clocks.clock-init",
      "pwm-ffd1b000.aml-pwm-device",
      "pwm-ffd1b000.aml-pwm-device.pwm-4.pwm-init",
      "i2c-1c000.aml-i2c.i2c.i2c-2-62",
      "i2c-5000.aml-i2c.i2c.i2c-0-111.audio-codec-6f.TAS5720",
      "i2c-5000.aml-i2c.i2c.i2c-0-108.audio-codec-6c.TAS5720",
      "i2c-5000.aml-i2c.i2c.i2c-0-109.audio-codec-6d.TAS5720",
      "i2c-5000.aml-i2c.i2c.i2c-0-57.tcs3400-light-39.tcs-3400",
      "sherlock.post-init",
      "usb-ff400000.dwc2",
      "aml-light",
      "temperature-sensor-ff634800",
      "temperature-sensor-ff634c00",
      "temperature-sensor-ff634800.thermal",
      "temperature-sensor-ff634c00.thermal",
      "cpu-controller-0",
      "temperature-sensor-ff634800.thermal.cpu-controller-0",
      "temperature-sensor-ff634800.thermal.cpu-controller-0.big-cluster",
      "temperature-sensor-ff634800.thermal.cpu-controller-0.little-cluster",
      "thermistor.thermistor-device.therm-base",
      "thermistor.thermistor-device.therm-audio",
      "thermistor.thermistor-device.therm-ambient",
      "adc-9000",
      "adc-9000.aml-saradc.adc-1",
      "adc-9000.aml-saradc.adc-2",
      "adc-9000.aml-saradc.adc-3",
  };
  VerifyNodes(kCommonNodeMonikers);

#ifdef include_packaged_drivers
  static const char* kPackagedCommonNodeMonikers[] = {
      "bt-uart-ffd24000.aml-uart.serial.bt-transport-uart",
      "mipi-csi-ff650000.aml-mipi",
      "mipi-csi-ff650000.aml-mipi.imx227",
      "mipi-csi-ff650000.aml-mipi.imx227.gdc",
      "mipi-csi-ff650000.aml-mipi.imx227.ge2d",
      "mipi-csi-ff650000.aml-mipi.imx227.isp",
      "mipi-csi-ff650000.aml-mipi.imx227.isp.arm-isp.camera_controller",
      "aml-light.gpio-light",
      "video-decoder-ffd00000.aml_video",
      "video-encoder-ffd00000.aml_he264_encoder",
      "display-ff900000.amlogic-display.display-coordinator",
      "audio-tdm-ff642000.aml_tdm.sherlock-audio-i2s-out",
      "audio-pdm-ff640000.aml_pdm.sherlock-audio-pdm-in",
      "gpio-controller-c-ff634400.aml-gpio.gpio.gpio-50.spi-13000.aml-spi-0.spi.spi-0-0.ot-radio-0.ot-radio",
      "mmc-ffe03000.aml-sd-emmc.sdmmc.sdmmc-sdio.sdmmc-sdio-1.wifi-1.brcmfmac-wlanphy",
      "nna-ff100000.aml-nna",
  };
  VerifyNodes(kPackagedCommonNodeMonikers);
#endif

  static const char* kTouchscreenNodeMonikers[] = {
      "i2c-1d000.aml-i2c.i2c.i2c-1-56.focaltech-touch-38.focaltouch-HidDevice",
      "i2c-1d000.aml-i2c.i2c.i2c-1-56.focaltech-touch.focaltouch-HidDevice",
      "i2c-1d000.aml-i2c.i2c.i2c-1-56.focaltouch-HidDevice",
      "i2c-1d000.aml-i2c.i2c.i2c-1-93.goodix-touch-5d.gt92xx-HidDevice",
      "i2c-1d000.aml-i2c.i2c.i2c-1-93.goodix-touch.gt92xx-HidDevice",
      "i2c-1d000.aml-i2c.i2c.i2c-1-93.gt92xx-HidDevice",
  };

  static const char* kBacklightNodeMonikers[] = {
      "i2c-1c000.aml-i2c.i2c.i2c-2-44.backlight-boe-2c",
      "i2c-1c000.aml-i2c.i2c.i2c-2-44.backlight-innolux-2c",
  };

  if (!HasNode("dt-root")) {
    static const char* kLegacyNodeMonikers[] = {
        "sherlock",
        // Thermal devices.
        "adc-9000.aml-saradc.0",
    };
    VerifyNodes(kLegacyNodeMonikers);
  }
  VerifyOneOf(kTouchscreenNodeMonikers);
  VerifyOneOf(kBacklightNodeMonikers);

  ASSERT_NO_FATAL_FAILURE(device_enumeration::WaitForClassDeviceCount("class/thermal", 2));
  ASSERT_NO_FATAL_FAILURE(device_enumeration::WaitForClassDeviceCount("class/adc", 4));
  ASSERT_NO_FATAL_FAILURE(device_enumeration::WaitForClassDeviceCount("class/temperature", 3));
}

}  // namespace
