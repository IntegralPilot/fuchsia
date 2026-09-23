// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bus/drivers/platform/test/power-integration-test/test-power-parent-device.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/driver/power/cpp/element-description-builder.h>
#include <lib/driver/power/cpp/power-support.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>

namespace fake_parent_device {

zx::result<> FakeParent::Start(fdf::DriverContext context) {
  node_.Bind(take_node());
  fdf::Arena arena('TEST');

  auto device_service =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>(
          "platform-device");
  if (!device_service.is_ok()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  fdf_power::PowerElementConfiguration power_config;
  {
    fidl::SyncClient<fuchsia_hardware_platform_device::Device> device_client(
        std::move(device_service.value()));
    fidl::Result fidl_config = device_client->GetPowerConfiguration();
    if (fidl_config.is_error()) {
      fdf::error("Failed to get power element configuration.");
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (fidl_config.value().config().empty()) {
      fdf::error("Missing power element configuration.");
      return zx::error(ZX_ERR_INTERNAL);
    }
    zx::result result =
        fdf_power::PowerElementConfiguration::FromFidl(fidl_config.value().config()[0]);
    if (result.is_error()) {
      FDF_SLOG(ERROR, "Failed to convert power element configuration from FIDL.",
               KV("status", result.status_string()));
      return result.take_error();
    }
    power_config = std::move(result.value());
  }

  // connect to power broker
  auto power_broker_req = context.incoming().Connect<fuchsia_power_broker::Topology>();
  {
    if (power_broker_req.is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    fit::result<fdf_power::Error, fdf_power::TokenMap> token_result =
        fdf_power::GetDependencyTokens(context.incoming(), power_config);
    if (token_result.is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    fdf_power::TokenMap tokens = std::move(token_result.value());

    fdf_power::ElementDesc description =
        fdf_power::ElementDescBuilder(power_config, std::move(tokens)).Build();

    fidl::ClientEnd<fuchsia_power_broker::Topology> broker = std::move(power_broker_req.value());
    auto add_result = fdf_power::AddElement(broker, description);

    topology_client_ =
        fidl::WireClient<fuchsia_power_broker::Topology>(std::move(broker), dispatcher());

    if (!add_result.is_ok()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  // Add a child
  {
    // First we'll set up a FIDL server that provides a token which gives
    // our child access to our power element.
    server_ = std::make_unique<fake_parent_device::FakeParentServer>(power_config.element.name);
    {
      auto result = outgoing()->AddService<fuchsia_hardware_power::PowerTokenService>(
          fuchsia_hardware_power::PowerTokenService::InstanceHandler({
              .token_provider =
                  bindings_.CreateHandler(server_.get(), dispatcher(), fidl::kIgnoreBindingClosure),
          }),
          power_config.element.name);

      if (!result.is_ok()) {
        return zx::error(ZX_ERR_INTERNAL);
      }
    }

    server2_ =
        std::make_unique<fake_parent_device::FakeParentServer>("test-parent-default-element");
    {
      auto result = outgoing()->AddService<fuchsia_hardware_power::PowerTokenService>(
          fuchsia_hardware_power::PowerTokenService::InstanceHandler({
              .token_provider = bindings_.CreateHandler(server2_.get(), dispatcher(),
                                                        fidl::kIgnoreBindingClosure),
          }));

      if (!result.is_ok()) {
        return zx::error(ZX_ERR_INTERNAL);
      }
    }

    // Next create a node for our child to bind to.
    fuchsia_driver_framework::NodeAddArgs node_args;
    node_args.name() = "fake-child";
    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_CHILD),
    };
    node_args.properties2() = properties;

    // Create the offer for our token provider so the child can access it.
    {
      std::vector<fuchsia_component_decl::NameMapping> mappings =
          std::vector<fuchsia_component_decl::NameMapping>{
              {{.source_name = power_config.element.name,
                .target_name = power_config.element.name}},
              {{.source_name = "default", .target_name = "default"}},
          };

      fuchsia_component_decl::OfferService services = fuchsia_component_decl::OfferService{{
          .source_name = std::string(fuchsia_hardware_power::PowerTokenService::Name),
          .target_name = std::string(fuchsia_hardware_power::PowerTokenService::Name),
          .source_instance_filter = std::vector<std::string>{power_config.element.name, "default"},
          .renamed_instances = mappings,
      }};

      auto component_offer_decl = fuchsia_component_decl::Offer::WithService(services);
      node_args.offers2() = std::vector<fuchsia_driver_framework::Offer>{
          fuchsia_driver_framework::Offer::WithZirconTransport(component_offer_decl),
      };
    }

    // The token service is ready, the child is defined, so we're ready to
    // add the child device node
    {
      auto endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>().value();
      auto result = node_->AddChild(fidl::ToWire(arena, std::move(node_args)),
                                    std::move(endpoints.server), {});

      if (!result.ok()) {
        return zx::error(ZX_ERR_INTERNAL);
      }

      if (result->is_error()) {
        return zx::error(ZX_ERR_INTERNAL);
      }

      child_controller_.Bind(std::move(endpoints.client));
    }
  }

  return zx::ok();
}

void FakeParentServer::GetToken(GetTokenCompleter::Sync& completer) {
  zx_handle_t mine;
  zx_event_create(0, &mine);
  zx_handle_t yours;
  zx_handle_duplicate(mine, ZX_RIGHT_SAME_RIGHTS, &yours);
  completer.ReplySuccess(zx::event(yours));
}
void FakeParentServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_power::PowerTokenProvider> md,
    fidl::UnknownMethodCompleter::Sync& completer) {}

}  // namespace fake_parent_device

FUCHSIA_DRIVER_EXPORT2(fake_parent_device::FakeParent);
