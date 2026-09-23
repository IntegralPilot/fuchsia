// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/serial/drivers/aml-uart/aml-uart-dfv2.h"

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/serial/cpp/bind.h>

namespace serial {

namespace {

constexpr std::string_view kPdevName = "pdev";
constexpr std::string_view kChildName = "aml-uart";

}  // namespace

zx::result<> AmlUartV2::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  driver_config_ = context.take_config<aml_uart_config::Config>();

  auto pdev_client_end =
      incoming->Connect<fuchsia_hardware_platform_device::Service::Device>(kPdevName);
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  if (zx::result result =
          mac_address_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming);
      result.is_error()) {
    fdf::error("Failed to forward mac address metadata: {}", result);
    return result.take_error();
  }

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_serial::SerialPortInfo>(
      fuchsia_hardware_serial::SerialPortInfo::kSerializableName);
  if (metadata.is_error()) {
    if (metadata.status_value() == ZX_ERR_NOT_FOUND) {
      fdf::debug("Serial port info metadata not found.");
    } else {
      fdf::error("Failed to get metadata: {}", metadata);
      return metadata.take_error();
    }
  } else {
    serial_port_info_ = {
        .serial_class = metadata->serial_class(),
        .serial_vid = metadata->serial_vid(),
        .serial_pid = metadata->serial_pid(),
    };
  }

  zx::result mmio = pdev.MapMmio(0);
  if (mmio.is_error()) {
    FDF_SLOG(ERROR, "Failed to map mmio.", KV("status", mmio.status_string()));
    return mmio.take_error();
  }

  fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag;
  if (driver_config_.enable_suspend()) {
    zx::result result = incoming->Connect<fuchsia_power_system::ActivityGovernor>();
    if (result.is_error() || !result->is_valid()) {
      fdf::warn("Failed to connect to activity governor: {}", result);
      return result.take_error();
    }
    sag = std::move(result.value());
  }

  aml_uart_.emplace(std::move(pdev), serial_port_info_, std::move(mmio.value()), std::move(sag));

  // Default configuration for the case that serial_impl_config is not called.
  constexpr uint32_t kDefaultBaudRate = 115200;
  constexpr uint32_t kDefaultConfig = fuchsia_hardware_serialimpl::kSerialDataBits8 |
                                      fuchsia_hardware_serialimpl::kSerialStopBits1 |
                                      fuchsia_hardware_serialimpl::kSerialParityNone;
  aml_uart_->Config(kDefaultBaudRate, kDefaultConfig);

  fuchsia_hardware_serialimpl::Service::InstanceHandler handler({
      .device =
          [this](fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server_end) {
            serial_impl_bindings_.AddBinding(driver_dispatcher()->get(), std::move(server_end),
                                             &aml_uart_.value(), fidl::kIgnoreBindingClosure);
          },
  });
  zx::result<> add_result =
      outgoing()->AddService<fuchsia_hardware_serialimpl::Service>(std::move(handler), kChildName);
  if (add_result.is_error()) {
    fdf::error("Failed to add fuchsia_hardware_serialimpl::Service {}", add_result.status_string());
    return add_result.take_error();
  }

  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(kChildName),
  };
  std::optional mac_address_offer = mac_address_metadata_server_.CreateOffer();
  if (mac_address_offer.has_value()) {
    offers.push_back(std::move(mac_address_offer.value()));
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {{
      fdf::MakeProperty2(bind_fuchsia::SERIAL_CLASS,
                         static_cast<uint32_t>(aml_uart_->serial_port_info().serial_class)),
  }};

  auto result = AddChild(std::string(kChildName), properties, offers);
  if (result.is_error()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return result.take_error();
  }

  fdf::info("Successfully started aml-uart-dfv2 driver.");
  return zx::ok();
}

void AmlUartV2::Stop(fdf::StopCompleter completer) {
  if (aml_uart_.has_value()) {
    aml_uart_->Enable(false);
  }

  completer(zx::ok());
}

AmlUart& AmlUartV2::aml_uart_for_testing() {
  ZX_ASSERT(aml_uart_.has_value());
  return aml_uart_.value();
}

}  // namespace serial

FUCHSIA_DRIVER_EXPORT2(serial::AmlUartV2);
