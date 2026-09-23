// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.powermanager.root.test/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <memory>
#include <string>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/powermanager_bindlib/cpp/bind.h>

namespace {

class RootDriver : public fdf::DriverBase2,
                   public fidl::WireServer<fuchsia_powermanager_root_test::Device> {
  static constexpr std::string_view driver_name = "root-device";
  static constexpr std::string_view parent_node_name = "sys";
  static constexpr std::string_view child_node_name = "platform";

 public:
  RootDriver()
      : fdf::DriverBase2(driver_name),
        devfs_connector_(fit::bind_member<&RootDriver::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_ = context.take_incoming();
    node_name_ = context.node_name().value_or("");

    auto parent_result = CreateParentNode();
    ZX_ASSERT_MSG(parent_result.is_ok(), "Failed to create /dev/sys: %s",
                  parent_result.status_string());

    auto child_result = CreateChildNode();
    ZX_ASSERT_MSG(child_result.is_ok(), "Failed to create /dev/sys/platform: %s",
                  child_result.status_string());

    return zx::ok();
  }

 private:
  zx::result<> CreateParentNode() {
    fidl::Arena arena;

    auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                    .name(arena, parent_node_name)
                    .Build();

    // Create endpoints of the `NodeController` for the node.
    auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
    ZX_ASSERT_MSG(node_endpoints.is_ok(), "Failed to create node endpoints: %s",
                  node_endpoints.status_string());

    fidl::WireResult result = fidl::WireCall(node())->AddChild(
        args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
    if (!result.ok()) {
      FDF_SLOG(ERROR, "Failed to add child", KV("status", result.status_string()));
      return zx::error(result.status());
    }

    controllers_.emplace_back(std::move(controller_endpoints.client));
    nodes_.emplace_back(std::move(node_endpoints->client));

    return zx::ok();
  }

  zx::result<> CreateChildNode() {
    fidl::Arena arena;

    zx::result connector = devfs_connector_.Bind(dispatcher());
    if (connector.is_error()) {
      return connector.take_error();
    }

    auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                     .connector(std::move(connector.value()))
                     .connector_supports(fuchsia_device_fs::ConnectionType::kController)
                     .class_name("test");

    auto properties = fidl::VectorView<fuchsia_driver_framework::wire::NodeProperty2>(arena, 1);
    properties[0] = fdf::MakeProperty2(arena, bind_fuchsia::PROTOCOL,
                                       bind_fuchsia_powermanager_driver::BIND_PROTOCOL_PLATFORM);

    auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                    .name(arena, child_node_name)
                    .properties2(properties)
                    .devfs_args(devfs.Build())
                    .Build();

    // Create endpoints of the `NodeController` for the node.
    auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    fidl::WireResult result =
        fidl::WireCall(nodes_.back())->AddChild(args, std::move(controller_endpoints.server), {});
    if (!result.ok()) {
      FDF_SLOG(ERROR, "Failed to add child", KV("status", result.status_string()));
      return zx::error(result.status());
    }

    controllers_.emplace_back(std::move(controller_endpoints.client));

    // TODO(b/313922891): This is a temporary solution for a test to add a driver without disrupting
    // other tests.
    auto temp_properties =
        fidl::VectorView<fuchsia_driver_framework::wire::NodeProperty2>(arena, 1);
    temp_properties[0] = fdf::MakeProperty2(arena, bind_fuchsia::PROTOCOL,
                                            bind_fuchsia_powermanager_driver::BIND_PROTOCOL_TEMP);

    auto temp_args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                         .name(arena, "temp")
                         .properties2(temp_properties)
                         .Build();

    // Create endpoints of the `NodeController` for the node.
    auto temp_controller_endpoints =
        fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

    fidl::WireResult temp_result =
        fidl::WireCall(nodes_.back())
            ->AddChild(temp_args, std::move(temp_controller_endpoints.server), {});
    if (!temp_result.ok()) {
      FDF_SLOG(ERROR, "Failed to add child", KV("status", temp_result.status_string()));
      return zx::error(temp_result.status());
    }

    controllers_.emplace_back(std::move(temp_controller_endpoints.client));

    return zx::ok();
  }

  void Serve(fidl::ServerEnd<fuchsia_powermanager_root_test::Device> server) {
    bindings_.AddBinding(dispatcher(), std::move(server), this, fidl::kIgnoreBindingClosure);
  }

  // fidl::WireServer<fuchsia_powermanager_root_test::Device>
  void Ping(PingCompleter::Sync& completer) override { completer.Reply(); }

  fidl::ServerBindingGroup<fuchsia_powermanager_root_test::Device> bindings_;

  std::unique_ptr<fdf::Namespace> incoming_;
  std::string node_name_;

  std::vector<fidl::ClientEnd<fuchsia_driver_framework::Node>> nodes_;
  std::vector<fidl::WireSyncClient<fuchsia_driver_framework::NodeController>> controllers_;
  driver_devfs::Connector<fuchsia_powermanager_root_test::Device> devfs_connector_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
