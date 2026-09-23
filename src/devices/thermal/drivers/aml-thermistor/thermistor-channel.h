// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_THERMAL_DRIVERS_AML_THERMISTOR_THERMISTOR_CHANNEL_H_
#define SRC_DEVICES_THERMAL_DRIVERS_AML_THERMISTOR_THERMISTOR_CHANNEL_H_

#include <fidl/fuchsia.hardware.adc/cpp/wire.h>
#include <fidl/fuchsia.hardware.temperature/cpp/wire.h>
#include <lib/ddk/device.h>
#include <lib/thermal/ntc.h>
#include <lib/zx/interrupt.h>

#include <string>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>
#include <fbl/mutex.h>
#include <fbl/ref_ptr.h>

namespace thermal {

class ThermistorDeviceTest;

namespace FidlTemperature = fuchsia_hardware_temperature;
class ThermistorChannel;
using DeviceType2 =
    ddk::Device<ThermistorChannel, ddk::Messageable<FidlTemperature::Device>::Mixin>;

class ThermistorChannel : public DeviceType2, public ddk::EmptyProtocol<ZX_PROTOCOL_TEMPERATURE> {
 public:
  ThermistorChannel(zx_device_t* device, fidl::ClientEnd<fuchsia_hardware_adc::Device> adc,
                    NtcInfo ntc_info, uint32_t pullup_ohms, const char* name)
      : DeviceType2(device), adc_(std::move(adc)), ntc_(ntc_info, pullup_ohms), name_(name) {}

  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override;
  void GetSensorName(GetSensorNameCompleter::Sync& completer) override;
  void DdkRelease() { delete this; }

 private:
  friend ThermistorDeviceTest;

  fidl::WireSyncClient<fuchsia_hardware_adc::Device> adc_;
  const Ntc ntc_;
  const std::string name_;
};

}  // namespace thermal

#endif  // SRC_DEVICES_THERMAL_DRIVERS_AML_THERMISTOR_THERMISTOR_CHANNEL_H_
