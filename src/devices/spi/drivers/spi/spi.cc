// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "spi.h"

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/fit/function.h>

#include <bind/fuchsia/cpp/bind.h>
#include <fbl/alloc_checker.h>

#include "spi-child.h"
#include "src/devices/spi/drivers/spi/spi_config.h"

namespace spi {

zx::result<> SpiDriver::Start(fdf::DriverContext context) {
  incoming_ = context.take_incoming();
  node_token_ = context.take_node_token();

  zx::result metadata_result =
      fdf_metadata::GetMetadata<fuchsia_hardware_spi_businfo::SpiBusMetadata>(*incoming_);
  if (metadata_result.is_error()) {
    fdf::error("Failed to get SPI metadata: {}", metadata_result);
    return metadata_result.take_error();
  }
  fuchsia_hardware_spi_businfo::SpiBusMetadata& metadata = metadata_result.value();

  if (!metadata.bus_id()) {
    fdf::error("No bus ID metadata provided");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  bus_id_ = *metadata.bus_id();

  zx::result scheduler_role_name_result =
      fdf_metadata::GetMetadataIfExists<fuchsia_scheduler::RoleName>(*incoming_);
  if (scheduler_role_name_result.is_error()) {
    fdf::error("Failed to get scheduler role name: {}", scheduler_role_name_result);
    return scheduler_role_name_result.take_error();
  }
  if (scheduler_role_name_result.value().has_value()) {
    const auto& scheduler_role_name = scheduler_role_name_result.value().value();

    zx::result result = fdf::SynchronizedDispatcher::Create(
        {}, "SPI", [](fdf_dispatcher_t*) {}, scheduler_role_name.role());
    if (result.is_error()) {
      fdf::error("Failed to create SynchronizedDispatcher: {}", result);
      return result.take_error();
    }

    // If scheduler role metadata was provided, create a new dispatcher using the role, and use that
    // dispatcher instead of the default dispatcher passed to this method.
    fidl_dispatcher_.emplace(*std::move(result));

    fdf::debug("Using dispatcher with role \"{}\"", scheduler_role_name.role().c_str());
  }

  zx::result spi_impl_client_end = incoming()->Connect<fuchsia_hardware_spiimpl::Service::Device>();
  if (spi_impl_client_end.is_error()) {
    return spi_impl_client_end.take_error();
  }

  fdf::WireSharedClient spi_impl(*std::move(spi_impl_client_end), fidl_dispatcher()->get());

  const auto config = context.take_config<spi_config::Config>();
  if (config.expose_debug_capabilities()) {
    if (zx::result<> result = AddTestChild(spi_impl.Clone()); result.is_error()) {
      return result;
    }
  }

  zx::result child = AddOwnedChild(kChildNodeName);
  if (child.is_error()) {
    fdf::error("Failed to add owned child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  if (metadata.channels()) {
    if (zx::result result = AddChildren(metadata, std::move(spi_impl), config); result.is_error()) {
      return result.take_error();
    }
  } else {
    fdf::info("No channels supplied.");
  }

  return zx::ok();
}

zx::result<> SpiDriver::AddTestChild(
    fdf::WireSharedClient<fuchsia_hardware_spiimpl::SpiImpl> client) {
  fdf::Arena arena('SPI_');
  if (auto result = client.buffer(arena)->ReleaseRegisteredVmos(
          fuchsia_hardware_spiimpl::kLoopbackChipSelect);
      !result.ok()) {
    fdf::error("Failed to release registered VMOs: {}", result.error().FormatDescription());
  }

  fuchsia_hardware_spi::TestService::InstanceHandler handler({
      .test = test_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });

  auto serve_result = outgoing()->AddService<fuchsia_hardware_spi::TestService>(std::move(handler));
  if (serve_result.is_error()) {
    fdf::error("Failed to add SPI test service: {}", serve_result);
    return serve_result.take_error();
  }

  fbl::AllocChecker ac;

  test_child_ = std::unique_ptr<SpiChild>(
      new (&ac) SpiChild(std::move(client), fuchsia_hardware_spiimpl::kLoopbackChipSelect,
                         /*has_siblings=*/true, fidl_dispatcher()));

  if (!ac.check()) {
    fdf::error("Out of memory");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok();
}

zx::result<> SpiDriver::AddChildren(const fuchsia_hardware_spi_businfo::SpiBusMetadata& metadata,
                                    fdf::WireSharedClient<fuchsia_hardware_spiimpl::SpiImpl> client,
                                    const spi_config::Config& config) {
  bool has_siblings = metadata.channels()->size() > 1;
  for (auto& channel : *metadata.channels()) {
    const auto cs = channel.cs().value_or(0);
    const auto vid = channel.vid().value_or(0);
    const auto pid = channel.pid().value_or(0);
    const auto did = channel.did().value_or(0);

    char name[20];
    snprintf(name, sizeof(name), "spi-%u-%u", bus_id_, cs);

    fdf::Arena arena('SPI_');
    // Release any leftover registered VMOs in case we're rebinding.
    if (auto result = client.buffer(arena)->ReleaseRegisteredVmos(cs); !result.ok()) {
      fdf::error("Failed to release registered VMOs: {}", result.error().FormatDescription());
    }

    auto offers = std::vector{
        fdf::MakeOffer2<fuchsia_hardware_spi::Service>(arena, name),
    };

    if (config.enable_suspend()) {
      // Forward PowerTokenService to our parent if suspend is enabled.
      fuchsia_hardware_power::PowerTokenService::InstanceHandler handler({
          .token_provider =
              [this](fidl::ServerEnd<fuchsia_hardware_power::PowerTokenProvider> server) {
                zx::result<> result =
                    incoming()->Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
                        std::move(server));
                if (result.is_error()) {
                  fdf::warn("Failed to connect to power token service: {}", result);
                }
              },
      });

      zx::result result = outgoing()->AddService<fuchsia_hardware_power::PowerTokenService>(
          std::move(handler), name);
      if (result.is_error()) {
        fdf::error("Failed to add power token service: {}", result);
        return result.take_error();
      }

      offers.emplace_back(fdf::MakeOffer2<fuchsia_hardware_power::PowerTokenService>(arena, name));
    }

    fbl::AllocChecker ac;

    std::unique_ptr<SpiChild> dev(
        new (&ac) SpiChild(client.Clone(), cs, has_siblings, fidl_dispatcher()));

    if (!ac.check()) {
      fdf::error("Out of memory");
      return zx::error(ZX_ERR_NO_MEMORY);
    }

    auto serve_result =
        outgoing()->AddService<fuchsia_hardware_spi::Service>(dev->CreateInstanceHandler(), name);
    if (serve_result.is_error()) {
      fdf::error("Failed to add SPI service: {}", serve_result);
      return serve_result.take_error();
    }

    std::vector<fuchsia_driver_framework::wire::NodeProperty2> props{
        fdf::MakeProperty2(arena, bind_fuchsia::SERVICE, "fuchsia.hardware.spi.Service"),
    };
    if (channel.global_id().has_value()) {
      props.push_back(fdf::MakeProperty2(arena, bind_fuchsia::ID, *channel.global_id()));
    }
    if (vid || pid || did) {
      props.push_back(fdf::MakeProperty2(arena, bind_fuchsia::PLATFORM_DEV_VID, vid));
      props.push_back(fdf::MakeProperty2(arena, bind_fuchsia::PLATFORM_DEV_PID, pid));
      props.push_back(fdf::MakeProperty2(arena, bind_fuchsia::PLATFORM_DEV_DID, did));
    }

    auto connector = dev->BindDevfs();
    if (connector.is_error()) {
      return connector.take_error();
    }

    auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                     .connector(*std::move(connector))
                     .connector_supports(fuchsia_device_fs::ConnectionType::kDevice)
                     .class_name("spi")
                     .Build();

    const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                          .name(arena, name)
                          .offers2(offers)
                          .properties2(props)
                          .devfs_args(devfs)
                          .Build();

    auto [controller_client, controller_server] =
        fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    auto result = fidl::WireCall(child_.node_)->AddChild(args, std::move(controller_server), {});
    if (!result.ok()) {
      fdf::error("Failed to add SPI child node: {}", result.error().FormatDescription().c_str());
      return zx::error(result.status());
    }
    if (result->is_error()) {
      fdf::error("Failed to add SPI child node");
      return zx::error(ZX_ERR_INTERNAL);
    }

    children_.push_back(std::move(dev));
  }

  return zx::ok();
}

void SpiDriver::ConnectSpiLoopback(
    fuchsia_hardware_spi::wire::TestConnectSpiLoopbackRequest* request,
    ConnectSpiLoopbackCompleter::Sync& completer) {
  if (test_child_) {
    test_child_->Bind(std::move(request->server));
    completer.ReplySuccess();
  } else {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
}

void SpiDriver::Get(GetCompleter::Sync& completer) {
  zx::event token;
  if (zx_status_t status = node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token); status == ZX_OK) {
    completer.ReplySuccess(std::move(token));
  } else {
    completer.ReplyError(status);
  }
}

void SpiDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_spi::Test> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown FIDL method ordinal {:x}", metadata.method_ordinal);
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace spi

FUCHSIA_DRIVER_EXPORT2(spi::SpiDriver);
