// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.nodemanager.test/cpp/wire.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/nodemanagerbind/test/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_nodemanager_test;
namespace bindlib = bind_fuchsia_nodemanagerbind_test;

namespace {

const std::string_view kLeftName = "left-resource";
const std::string_view kRightName = "right-resource";

class NumberServer : public fidl::WireServer<ft::Device> {
 public:
  explicit NumberServer(uint32_t number) : number_(number) {}

  void GetNumber(GetNumberCompleter::Sync& completer) override { completer.Reply(number_); }

 private:
  uint32_t number_;
};

class RootDriver : public fdf::DriverBase2 {
 public:
  RootDriver() : fdf::DriverBase2("root") {}

  zx::result<> Start(fdf::DriverContext context) override {
    // Add service "left".
    {
      ft::Service::InstanceHandler handler({
          .device =
              bindings_.CreateHandler(&left_server_, dispatcher(), fidl::kIgnoreBindingClosure),
      });
      zx::result<> status = outgoing()->AddService<ft::Service>(std::move(handler), kLeftName);
      if (status.is_error()) {
        fdf::error("Failed to add service {}", status);
        return status.take_error();
      }
    }

    // Add service "right".
    {
      ft::Service::InstanceHandler handler({
          .device =
              bindings_.CreateHandler(&right_server_, dispatcher(), fidl::kIgnoreBindingClosure),
      });
      zx::result<> status = outgoing()->AddService<ft::Service>(std::move(handler), kRightName);
      if (status.is_error()) {
        fdf::error("Failed to add service {}", status);
        return status.take_error();
      }
    }

    // Setup the NodeManager client.
    auto nm_client = context.incoming().Connect<fdf::NodeManager>();
    if (nm_client.is_error()) {
      fdf::error("Failed to connect to NodeManager: {}",
                 zx_status_get_string(nm_client.error_value()));

      return nm_client.take_error();
    }

    node_manager_.Bind(std::move(nm_client.value()), dispatcher());

    auto left_result =
        ProvideResource(kLeftName, 1, bindlib::TEST_DEVICE_TYPE_LEFT_SPKR, left_controller_);
    if (!left_result) {
      fdf::error("Failed to provide left resource.");
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto right_result =
        ProvideResource(kRightName, 1, bindlib::TEST_DEVICE_TYPE_RIGHT_SPKR, right_controller_);
    if (!right_result) {
      fdf::error("Failed to provide right resource.");
      return zx::error(ZX_ERR_INTERNAL);
    }

    // Construct Node2 with two dependencies.
    fdf::Node2 node2;
    node2.name("test_node");

    std::vector<fdf::Dependency> dependencies;

    // Dependency 1: Left resource.
    {
      fdf::ResourceProperty prop{{
          .key = std::string(bindlib::TEST_DEVICE_TYPE),
          .value = fdf::NodePropertyValue::WithStringValue(
              std::string(bindlib::TEST_DEVICE_TYPE_LEFT_SPKR)),
      }};
      fdf::Selector selector{{
          .include_properties = {{std::move(prop)}},
      }};
      fdf::Dependency dep{{
          .selector = std::move(selector),
      }};
      dependencies.push_back(std::move(dep));
    }

    // Dependency 2: Right resource.
    {
      fdf::ResourceProperty prop{{
          .key = std::string(bindlib::TEST_DEVICE_TYPE),
          .value = fdf::NodePropertyValue::WithStringValue(
              std::string(bindlib::TEST_DEVICE_TYPE_RIGHT_SPKR)),
      }};
      fdf::Selector selector{{
          .include_properties = {{std::move(prop)}},
      }};
      fdf::Dependency dep{{
          .selector = std::move(selector),
      }};
      dependencies.push_back(std::move(dep));
    }

    node2.dependencies(std::move(dependencies));

    auto controller_endpoints = fidl::Endpoints<fdf::NodeController>::Create();
    node_controller_ = std::move(controller_endpoints.client);

    fidl::Arena arena;
    node_manager_->AddNode(fidl::ToWire(arena, node2), std::move(controller_endpoints.server), {})
        .Then([](fidl::WireUnownedResult<fdf::NodeManager::AddNode>& result) {
          if (!result.ok()) {
            fdf::error("AddNode failed: {}", result.FormatDescription());
            return;
          }
          if (result->is_error()) {
            fdf::error("AddNode failed with error");
            return;
          }
          fdf::info("Successfully added node.");
        });

    return zx::ok();
  }

 private:
  bool ProvideResource(std::string_view name, uint32_t instance, std::string_view property,
                       fidl::ClientEnd<fdf::ResourceController>& resource_controller) {
    auto resource_name = std::string(name) + "-" + std::to_string(instance);
    fdf::NodeProperty2 resource_property = fdf::MakeProperty2(bindlib::TEST_DEVICE_TYPE, property);
    std::vector<fdf::NodeProperty2> properties{{resource_property}};
    std::vector<fdf::Offer> offers{{fdf::MakeOffer2<ft::Service>(name)}};

    auto result = fdf::DriverBase2::ProvideResource(resource_name, properties, offers);
    if (result.is_error()) {
      fdf::error("ProvideResource() failed for {}: {}", std::string(resource_name),
                 result.status_string());
      return false;
    }

    resource_controller = std::move(result.value());
    fdf::info("Successfully provided resource {}.", resource_name.c_str());
    return true;
  }

  fidl::ClientEnd<fdf::ResourceController> left_controller_;
  fidl::ClientEnd<fdf::ResourceController> right_controller_;
  fidl::ClientEnd<fdf::NodeController> node_controller_;
  fidl::WireSharedClient<fdf::NodeManager> node_manager_;

  NumberServer left_server_ = NumberServer(1);
  NumberServer right_server_ = NumberServer(2);

  fidl::ServerBindingGroup<ft::Device> bindings_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
