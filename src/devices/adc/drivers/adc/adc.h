// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_ADC_DRIVERS_ADC_ADC_H_
#define SRC_DEVICES_ADC_DRIVERS_ADC_ADC_H_

#include <fidl/fuchsia.hardware.adc/cpp/fidl.h>
#include <fidl/fuchsia.hardware.adcimpl/cpp/driver/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zx/interrupt.h>

#include <optional>

#include <fbl/mutex.h>

namespace adc {

class Adc;

class AdcDevice : public fidl::Server<fuchsia_hardware_adc::Device> {
 public:
  AdcDevice(fdf::ClientEnd<fuchsia_hardware_adcimpl::Device> adc_impl, uint32_t channel,
            std::string_view name, std::optional<uint8_t> resolution, Adc* adc);

  static zx::result<std::unique_ptr<AdcDevice>> Create(
      fdf::ClientEnd<fuchsia_hardware_adcimpl::Device> adc_impl,
      fuchsia_hardware_adcimpl::AdcChannel channel, Adc* adc);

  // fuchsia_hardware_adc required methods.
  void GetResolution(GetResolutionCompleter::Sync& completer) override;
  void GetSample(GetSampleCompleter::Sync& completer) override;
  void GetNormalizedSample(GetNormalizedSampleCompleter::Sync& completer) override;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController>& controller() {
    return controller_;
  }

 private:
  zx::result<> CreateDevfsNode();
  void Serve(fidl::ServerEnd<fuchsia_hardware_adc::Device> server) {
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server), this,
                         fidl::kIgnoreBindingClosure);
  }

  const fdf::WireSyncClient<fuchsia_hardware_adcimpl::Device> adc_impl_;
  const uint32_t channel_;
  const std::string name_;

  const std::optional<uint8_t> resolution_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::ServerBindingGroup<fuchsia_hardware_adc::Device> bindings_;
  driver_devfs::Connector<fuchsia_hardware_adc::Device> devfs_connector_;
};

class Adc : public fdf::DriverBase2 {
 private:
  static constexpr char kDeviceName[] = "adc";

 public:
  explicit Adc() : fdf::DriverBase2(kDeviceName) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

 private:
  friend class AdcDevice;

  std::vector<std::unique_ptr<AdcDevice>> devices_;
};

}  // namespace adc

#endif  // SRC_DEVICES_ADC_DRIVERS_ADC_ADC_H_
