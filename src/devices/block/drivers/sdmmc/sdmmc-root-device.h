// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_ROOT_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_ROOT_DEVICE_H_

#include <fidl/fuchsia.hardware.sdmmc/cpp/wire.h>
#include <fuchsia/hardware/sdmmc/cpp/banjo.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/power/cpp/suspend.h>
#include <lib/zx/result.h>

#include "sdio-controller-device.h"
#include "sdmmc-block-device.h"
#include "src/devices/block/drivers/sdmmc/sdmmc_config.h"

namespace sdmmc {

constexpr uint32_t kInitializationFrequencyHz = 400'000;

class SdmmcDevice;

class SdmmcRootDevice : public fdf::DriverBase2, public fdf_power::Suspendable<SdmmcRootDevice> {
 public:
  explicit SdmmcRootDevice() : fdf::DriverBase2("sdmmc") {}

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;

  // Called by children (or grandchildren) of this device for invoking AddChild() or instantiating
  // compat::DeviceServer.
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& root_node() { return root_node_; }
  std::string_view driver_name() const { return name(); }
  const std::shared_ptr<fdf::Namespace>& driver_incoming() const { return incoming_; }
  std::shared_ptr<fdf::OutgoingDirectory>& driver_outgoing() { return outgoing(); }
  async_dispatcher_t* driver_async_dispatcher() const { return dispatcher(); }
  const fdf::UnownedSynchronizedDispatcher& driver_dispatcher() const {
    return DriverBase2::driver_dispatcher();
  }
  const std::optional<std::string>& driver_node_name() const { return node_name_; }
  inspect::ComponentInspector& driver_inspector() { return *component_inspector_; }
  const sdmmc_config::Config& config() const { return config_; }
  const zx::event& power_element_token() const { return power_element_token_; }

  zx::event node_token() const {
    zx::event copy;
    if (node_token_.is_valid()) {
      node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &copy);
    }
    return copy;
  }

  // Visible for testing.
  const std::variant<std::monostate, std::unique_ptr<SdioControllerDevice>,
                     std::unique_ptr<SdmmcBlockDevice>>&
  child_device() const {
    return child_device_;
  }

  void Suspend(fdf_power::SuspendCompleter completer) override {
    const auto* block_device = std::get_if<std::unique_ptr<SdmmcBlockDevice>>(&child_device_);

    if (block_device && block_device->get()->SuspendEnabled()) {
      block_device->get()->Suspend(std::move(completer));
      return;
    }
    const auto* sdio_device = std::get_if<std::unique_ptr<SdioControllerDevice>>(&child_device_);
    if (sdio_device && sdio_device->get()->SuspendEnabled()) {
      sdio_device->get()->Suspend(std::move(completer));
      return;
    }

    completer();
  }

  void Resume(fdf_power::ResumeCompleter completer) override {
    const auto* block_device = std::get_if<std::unique_ptr<SdmmcBlockDevice>>(&child_device_);

    if (block_device && block_device->get()->SuspendEnabled()) {
      block_device->get()->Resume(std::move(completer));
      return;
    }

    const auto* sdio_device = std::get_if<std::unique_ptr<SdioControllerDevice>>(&child_device_);
    if (sdio_device && sdio_device->get()->SuspendEnabled()) {
      sdio_device->get()->Resume(std::move(completer));
      return;
    }

    completer();
  }

  bool SuspendEnabled() override { return config_.enable_suspend(); }
  // Used by fdf_power::Suspendable.
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> take_power_element_runner() {
    return std::move(power_element_runner_);
  }

 protected:
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
  virtual zx_status_t Init(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata);

  std::variant<std::monostate, std::unique_ptr<SdioControllerDevice>,
               std::unique_ptr<SdmmcBlockDevice>>
      child_device_;

 private:
  // Returns the SDMMC metadata with default values for any fields that are not present (or if the
  // metadata itself is not present). Returns an error if the metadata could not be decoded.
  zx::result<fuchsia_hardware_sdmmc::SdmmcMetadata> GetMetadata();

  template <class DeviceType>
  zx::result<std::unique_ptr<SdmmcDevice>> MaybeAddDevice(
      const std::string& name, std::unique_ptr<SdmmcDevice> sdmmc,
      const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata);

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<std::string> node_name_;
  zx::event power_element_token_;
  zx::event node_token_;
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> power_element_runner_;

  std::optional<inspect::ComponentInspector> component_inspector_;
  sdmmc_config::Config config_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_ROOT_DEVICE_H_
