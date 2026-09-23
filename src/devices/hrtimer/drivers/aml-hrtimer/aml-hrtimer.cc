// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/hrtimer/drivers/aml-hrtimer/aml-hrtimer.h"

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <zircon/syscalls-next.h>

namespace hrtimer {

zx::result<> AmlHrtimer::Start(fdf::DriverContext context) {
  config_ = context.take_config<aml_hrtimer_config::Config>();
  zx::result pdev_result =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_result.is_error()) {
    fdf::error("Failed to connect to pdev protocol: {}", pdev_result);
    return pdev_result.take_error();
  }
  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> pdev(
      std::move(pdev_result.value()));

  auto mmio = pdev->GetMmioById(0);
  if (!mmio.ok()) {
    fdf::error("Call to GetMmioById failed: {}", mmio.FormatDescription().c_str());
    return zx::error(mmio.status());
  }
  if (mmio->is_error()) {
    fdf::error("GetMmioById failed: {}", zx_status_get_string(mmio->error_value()));
    return mmio->take_error();
  }

  if (!mmio->value()->has_vmo() || !mmio->value()->has_size() || !mmio->value()->has_offset()) {
    fdf::error("GetMmioById returned invalid MMIO");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  zx::result mmio_buffer =
      fdf::MmioBuffer::Create(mmio->value()->offset(), mmio->value()->size(),
                              std::move(mmio->value()->vmo()), ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (mmio_buffer.is_error()) {
    fdf::error("Failed to map MMIO: {}", mmio_buffer);
    return zx::error(mmio_buffer.error_value());
  }

  zx::interrupt irqs[kNumberOfIrqs];
  uint32_t count = 0;
  for (auto& irq : irqs) {
    auto result_irq = pdev->GetInterruptById(count++, 0);
    if (!result_irq.ok()) {
      fdf::error("Call to GetInterruptById failed: {}", result_irq.FormatDescription().c_str());
      return zx::error(result_irq->error_value());
    }
    if (result_irq->is_error()) {
      fdf::error("GetInterruptById failed: {}", zx_status_get_string(result_irq->error_value()));
      return result_irq->take_error();
    }
    irq = std::move(result_irq->value()->irq);
  }

  std::optional<fidl::SyncClient<fuchsia_power_system::ActivityGovernor>> sag;

  auto sag_connect = context.incoming().Connect<fuchsia_power_system::ActivityGovernor>();
  if (!config_.enable_suspend()) {
    fdf::warn("fuchsia.power.SuspendEnabled config disabled, continue without power support");
  } else if (sag_connect.is_error() || !sag_connect->is_valid()) {
    fdf::warn("Failed to connect to SAG: {} continue without power support", sag_connect);
  } else {
    fidl::SyncClient<fuchsia_power_system::ActivityGovernor> local_sag(std::move(*sag_connect));
    sag.emplace(std::move(local_sag));
  }
  exposed_inspector_.emplace(context.CreateInspector(this));
  server_ = std::make_unique<hrtimer::AmlHrtimerServer>(
      dispatcher(), std::move(*mmio_buffer), std::move(sag), std::move(irqs[0]), std::move(irqs[1]),
      std::move(irqs[2]), std::move(irqs[3]), std::move(irqs[4]), std::move(irqs[5]),
      std::move(irqs[6]), std::move(irqs[7]), *exposed_inspector_);

  auto result_dev = outgoing()->component().AddUnmanagedProtocol<fuchsia_hardware_hrtimer::Device>(
      bindings_.CreateHandler(server_.get(), dispatcher(), fidl::kIgnoreBindingClosure),
      kDeviceName);
  if (result_dev.is_error()) {
    fdf::error("Failed to add input report service: {}", result_dev);
    return result_dev.take_error();
  }

  if (zx::result result_dev = CreateDevfsNode(); result_dev.is_error()) {
    fdf::error("Failed to export to devfs: {}", result_dev);
    return result_dev.take_error();
  }

  return zx::ok();
}

void AmlHrtimer::Stop(fdf::StopCompleter completer) {
  server_->ShutDown();
  completer(zx::ok());
}

zx::result<> AmlHrtimer::CreateDevfsNode() {
  fidl::Arena arena;
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector(std::move(connector.value()))
                   .class_name("hrtimer");

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, kDeviceName)
                  .devfs_args(devfs.Build())
                  .Build();

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
  ZX_ASSERT_MSG(node_endpoints.is_ok(), "Failed to create node endpoints: %s",
                node_endpoints.status_string());

  fidl::WireResult result = fidl::WireCall(node())->AddChild(
      args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
  if (!result.ok()) {
    fdf::error("Failed to add child {}", result.status_string());
    return zx::error(result.status());
  }
  controller_.Bind(std::move(controller_endpoints.client));
  node_.Bind(std::move(node_endpoints->client));
  return zx::ok();
}

}  // namespace hrtimer

FUCHSIA_DRIVER_EXPORT2(hrtimer::AmlHrtimer);
