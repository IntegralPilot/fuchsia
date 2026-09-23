// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/aml-g12-tdm/composite.h"

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/platform-device/cpp/pdev.h>

namespace audio::aml_g12 {

zx::result<> Driver::CreateDevfsNode() {
  fidl::Arena arena;
  zx::result connector = devfs_connector_.Bind(server_->dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs{{
      .connector{std::move(connector.value())},
      .class_name{"audio-composite"},
  }};

  zx::result child = AddOwnedChild(kDriverName, devfs);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

zx::result<> Driver::Start(fdf::DriverContext context) {
  zx::result pdev_client =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client.is_error() || !pdev_client->is_valid()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client);
    return pdev_client.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client.value())};
  // We get one MMIO per engine.
  // TODO(https://fxbug.dev/42082341): If we change the engines underlying AmlTdmDevice objects such
  // that they take an MmioView, then we can get only one MmioBuffer here, own it in this driver and
  // pass MmioViews to the underlying AmlTdmDevice objects.
  std::array<std::optional<fdf::MmioBuffer>, kNumberOfTdmEngines> mmios;
  for (size_t i = 0; i < kNumberOfTdmEngines; ++i) {
    // There is one MMIO region with index 0 used by this driver.
    zx::result mmio = pdev.MapMmio(0);
    if (mmio.is_error()) {
      fdf::error("Failed to map MMIO: {}", mmio);
      return zx::error(mmio.status_value());
    }
    mmios[i] = std::make_optional(std::move(mmio.value()));
  }

  // There is one BTI with index 0 used by this driver.
  zx::result bti = pdev.GetBti(0);
  if (bti.is_error()) {
    fdf::error("Failed to get BTI: {}", bti);
    return zx::error(bti.status_value());
  }

  zx::result clock_gate_result =
      context.incoming().Connect<fuchsia_hardware_clock::Service::Clock>(kClockGateParentName);
  if (clock_gate_result.is_error() || !clock_gate_result->is_valid()) {
    fdf::error("Connect to clock-gate failed: {}", clock_gate_result);
    return zx::error(clock_gate_result.error_value());
  }
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> gate_client(
      std::move(clock_gate_result.value()));

  zx::result clock_pll_result =
      context.incoming().Connect<fuchsia_hardware_clock::Service::Clock>(kClockPllParentName);
  if (clock_pll_result.is_error() || !clock_pll_result->is_valid()) {
    fdf::error("Connect to clock-pll failed: {}", clock_pll_result);
    return zx::error(clock_pll_result.error_value());
  }
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> pll_client(
      std::move(clock_pll_result.value()));

  std::array<std::string_view, kNumberOfPipelines> sclk_gpio_names = {
      kGpioTdmASclkParentName,
      kGpioTdmBSclkParentName,
      kGpioTdmCSclkParentName,
  };
  std::vector<SclkPin> sclk_clients;
  for (auto& sclk_gpio_name : sclk_gpio_names) {
    zx::result gpio_result =
        context.incoming().Connect<fuchsia_hardware_gpio::Service::Device>(sclk_gpio_name);
    if (gpio_result.is_error() || !gpio_result->is_valid()) {
      fdf::error("Connect to GPIO {} failed: {}", sclk_gpio_name, gpio_result);
      return zx::error(gpio_result.error_value());
    }

    zx::result pin_result =
        context.incoming().Connect<fuchsia_hardware_pin::Service::Device>(sclk_gpio_name);
    if (pin_result.is_error() || !pin_result->is_valid()) {
      fdf::error("Connect to Pin {} failed: {}", sclk_gpio_name, pin_result);
      return zx::error(pin_result.error_value());
    }

    SclkPin sclk_pin{fidl::WireSyncClient(*std::move(gpio_result)),
                     fidl::WireSyncClient(*std::move(pin_result))};
    // Only save the clients if we can communicate with them (we use methods with no side
    // effects) since optional nodes are valid even if they are not configured in the board driver.
    auto gpio_read_result = sclk_pin.gpio->Read();
    auto pin_configure_result = sclk_pin.pin->Configure({});
    if (gpio_read_result.ok() && pin_configure_result.ok()) {
      sclk_clients.emplace_back(std::move(sclk_pin));
    }
  }

  zx::result device_info = pdev.GetDeviceInfo();
  if (device_info.is_error()) {
    fdf::error("Failed to get device info: {}", device_info);
    return device_info.take_error();
  }

  if (device_info->vid == PDEV_VID_GENERIC && device_info->pid == PDEV_PID_GENERIC &&
      device_info->did == PDEV_DID_DEVICETREE_NODE) {
    // TODO(https://fxbug.dev/318736574) : Remove and rely only on GetDeviceInfo.
    zx::result board_info = pdev.GetBoardInfo();
    if (board_info.is_error()) {
      fdf::error("Failed to get board info: {}", board_info);
      return board_info.take_error();
    }

    if (board_info->vid == PDEV_VID_KHADAS) {
      switch (board_info->pid) {
        case PDEV_PID_VIM3:
          device_info->pid = PDEV_PID_AMLOGIC_A311D;
          break;
        default:
          fdf::error("Unsupported PID {:#x} for VID {:#x}", board_info->pid, board_info->vid);
          return zx::error(ZX_ERR_NOT_SUPPORTED);
      }
    } else if (board_info->vid == PDEV_VID_GOOGLE) {
      switch (board_info->pid) {
        case PDEV_PID_SHERLOCK:
          device_info->pid = PDEV_PID_AMLOGIC_T931;
          break;
        case PDEV_PID_ASTRO:
          device_info->pid = PDEV_PID_AMLOGIC_S905D2;
          break;
        default:
          fdf::error("Unsupported PID {:#x} for VID {:#x}", board_info->pid, board_info->vid);
          return zx::error(ZX_ERR_NOT_SUPPORTED);
      }
    } else {
      fdf::error("Unsupported VID {:#x}", board_info->vid);
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
  }

  metadata::AmlVersion aml_version = {};
  switch (device_info->pid) {
    case PDEV_PID_AMLOGIC_A311D:
      aml_version = metadata::AmlVersion::kA311D;
      break;
    case PDEV_PID_AMLOGIC_T931:
      [[fallthrough]];
    case PDEV_PID_AMLOGIC_S905D2:
      aml_version = metadata::AmlVersion::kS905D2G;  // Also works with T931G.
      break;
    case PDEV_PID_AMLOGIC_S905D3:
      aml_version = metadata::AmlVersion::kS905D3G;
      break;
    case PDEV_PID_AMLOGIC_A5:
      aml_version = metadata::AmlVersion::kA5;
      break;
    case PDEV_PID_AMLOGIC_A1:
      aml_version = metadata::AmlVersion::kA1;
      break;
    default:
      fdf::error("Unsupported PID {:#x}", device_info->pid);
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  inspector_.emplace(context.CreateInspector(this));
  auto recorder = std::make_unique<Recorder>(inspector_->root());
  // Populate Inspect with nodes specific to AudioCompositeServer.
  for (size_t idx = 0u; idx < kNumberOfTdmEngines; ++idx) {
    recorder->PopulateRingBuffer("tdm engine #" + std::to_string(idx),
                                 AudioCompositeServer::kRingBufferIds[idx],
                                 /* supports_active_channels= */ true,
                                 /* outgoing= */ idx < 3);
  }
  for (size_t idx = 0u; idx < kNumberOfPipelines; ++idx) {
    recorder->PopulateDai("pipeline #" + std::to_string(idx), AudioCompositeServer::kDaiIds[idx]);
  }

  server_ = std::make_unique<AudioCompositeServer>(
      std::move(mmios), std::move(bti.value()), dispatcher(), aml_version, std::move(gate_client),
      std::move(pll_client), std::move(sclk_clients), std::move(recorder));

  zx::result result =
      outgoing()->component().AddUnmanagedProtocol<fuchsia_hardware_audio::Composite>(
          bindings_.CreateHandler(server_.get(), dispatcher(), fidl::kIgnoreBindingClosure),
          kDriverName);
  if (result.is_error()) {
    fdf::error("Failed to add composite protocol: {}", result);
    return result.take_error();
  }

  if (zx::result result = CreateDevfsNode(); result.is_error()) {
    fdf::error("Failed to export to devfs: {}", result);
    return result.take_error();
  }

  fdf::info("Driver started");

  return zx::ok();
}

}  // namespace audio::aml_g12

FUCHSIA_DRIVER_EXPORT2(audio::aml_g12::Driver);
