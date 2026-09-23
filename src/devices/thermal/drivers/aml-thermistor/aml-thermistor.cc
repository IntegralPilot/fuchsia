// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-thermistor.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fit/defer.h>
#include <lib/thermal/metadata.h>
#include <lib/zx/interrupt.h>
#include <string.h>
#include <zircon/types.h>

#include <fbl/ref_counted.h>

namespace thermal {

static constexpr uint32_t kMaxNtcChannels = 4;

zx_status_t AmlThermistor::Create(void* ctx, zx_device_t* parent) {
  zx_status_t status;

  std::unique_ptr<AmlThermistor> device(new AmlThermistor(parent));

  if ((status = device->DdkAdd(
                    ddk::DeviceAddArgs("thermistor-device").set_flags(DEVICE_ADD_NON_BINDABLE)) !=
                ZX_OK)) {
    zxlogf(ERROR, "%s: DdkAdd failed", __func__);
    return status;
  }
  [[maybe_unused]] auto* unused = device.release();

  return ZX_OK;
}

zx_status_t AmlThermistor::AddThermChannel(NtcChannel ch, NtcInfo info) {
  char adc_name[32];
  sprintf(adc_name, "adc-%u", ch.adc_channel);

  auto adc = DdkConnectFragmentFidlProtocol<fuchsia_hardware_adc::Service::Device>(adc_name);
  if (adc.is_error()) {
    zxlogf(ERROR, "Failed to connect to %s", adc_name);
    return adc.error_value();
  }

  std::unique_ptr<ThermistorChannel> dev(
      new ThermistorChannel(zxdev(), std::move(*adc), info, ch.pullup_ohms, ch.name));

  auto status = dev->DdkAdd(ddk::DeviceAddArgs(ch.name));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto ptr = dev.release();
  return ZX_OK;
}

void AmlThermistor::DdkInit(ddk::InitTxn txn) {
  NtcChannel ntc_channels[kMaxNtcChannels];
  size_t actual;

  auto status =
      DdkGetMetadata(NTC_CHANNELS_METADATA_PRIVATE, &ntc_channels, sizeof(ntc_channels), &actual);
  if (status != ZX_OK) {
    txn.Reply(status);
    return;
  }

  ZX_DEBUG_ASSERT(actual % sizeof(NtcChannel) == 0);
  size_t num_channels = actual / sizeof(NtcChannel);

  NtcInfo ntc_info[kMaxNtcChannels];
  status = DdkGetMetadata(NTC_PROFILE_METADATA_PRIVATE, &ntc_info, sizeof(ntc_info), &actual);
  if (status != ZX_OK) {
    txn.Reply(status);
    return;
  }
  ZX_DEBUG_ASSERT(actual % sizeof(NtcInfo) == 0);
  size_t num_profiles = actual / sizeof(NtcInfo);

  for (uint32_t i = 0; i < num_channels; i++) {
    if (ntc_channels[i].profile_idx >= num_profiles) {
      txn.Reply(ZX_ERR_INVALID_ARGS);
      return;
    }
    status = AddThermChannel(ntc_channels[i], ntc_info[ntc_channels[i].profile_idx]);
    if (status != ZX_OK) {
      txn.Reply(status);
      return;
    }
  }
  txn.Reply(ZX_OK);
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = AmlThermistor::Create;
  return ops;
}();

}  // namespace thermal

// clang-format off
ZIRCON_DRIVER(aml-thermistor, thermal::driver_ops, "thermistor", "0.1");
