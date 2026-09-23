// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.resourceboundcomposite.test/cpp/wire.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/resourceboundcompositebind/test/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_resourceboundcomposite_test;
namespace bindlib = bind_fuchsia_resourceboundcompositebind_test;

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
      }
    }

    // Setup the composite node manager client.
    auto dgm_client = context.incoming().Connect<fdf::CompositeNodeManager>();
    if (dgm_client.is_error()) {
      fdf::error("Failed to connect to CompositeNodeManager: {}",
                 zx_status_get_string(dgm_client.error_value()));

      return dgm_client.take_error();
    }

    composite_node_manager_.Bind(std::move(dgm_client.value()), dispatcher());

    auto right_result = ProvideResource(kRightName, 1, bindlib::TEST_DEVICE_TYPE_RIGHT_SPKR);
    if (!right_result) {
      fdf::error("Failed to provide right resource.");
      return zx::error(ZX_ERR_INTERNAL);
    }

    std::vector<fdf::NodeProperty2> properties = {
        fdf::MakeProperty2(bindlib::TEST_DEVICE_TYPE, bindlib::TEST_DEVICE_TYPE_LEFT_SPKR)};
    std::vector<fdf::Offer> offers{{fdf::MakeOffer2<ft::Service>(kLeftName)}};
    auto left_result = fdf::DriverBase2::AddChild(kLeftName, properties, offers);
    if (left_result.is_error()) {
      fdf::error("Failed to add left child.");
      return left_result.take_error();
    }

    auto parents = std::vector{
        fdf::ParentSpec2{{
            .bind_rules =
                {
                    fdf::MakeAcceptBindRule(bindlib::TEST_DEVICE_TYPE,
                                            bindlib::TEST_DEVICE_TYPE_LEFT_SPKR),
                },
            .properties =
                {
                    fdf::MakeProperty2(bindlib::TEST_DEVICE_TYPE,
                                       bindlib::TEST_DEVICE_TYPE_LEFT_SPKR),
                },
        }},
        fdf::ParentSpec2{{
            .bind_rules =
                {
                    fdf::MakeAcceptBindRule(bindlib::TEST_DEVICE_TYPE,
                                            bindlib::TEST_DEVICE_TYPE_RIGHT_SPKR),
                },
            .properties =
                {
                    fdf::MakeProperty2(bindlib::TEST_DEVICE_TYPE,
                                       bindlib::TEST_DEVICE_TYPE_RIGHT_SPKR),
                },
        }},
    };

    AddSpec(fdf::CompositeNodeSpec{{.name = "test_composite", .parents2 = parents}});
    return zx::ok();
  }

 private:
  bool ProvideResource(std::string_view name, int group, std::string_view property) {
    auto resource_name = std::string(name) + "-" + std::to_string(group);
    fdf::NodeProperty2 resource_property = fdf::MakeProperty2(bindlib::TEST_DEVICE_TYPE, property);
    std::vector<fdf::NodeProperty2> properties{{resource_property}};
    std::vector<fdf::Offer> offers{{fdf::MakeOffer2<ft::Service>(name)}};

    auto result = fdf::DriverBase2::ProvideResource(resource_name, properties, offers);
    if (result.is_error()) {
      fdf::error("ProvideResource() failed: {}", result.status_string());
      return false;
    }

    resource_controller_ = std::move(result.value());
    fdf::info("Successfully provided resource {}.", resource_name.c_str());
    return true;
  }

  void AddSpec(fdf::CompositeNodeSpec dev_group) {
    auto dev_group_name = dev_group.name();
    composite_node_manager_->AddSpec(std::move(dev_group))
        .Then([dev_group_name](fidl::Result<fdf::CompositeNodeManager::AddSpec>& create_result) {
          if (create_result.is_error()) {
            fdf::error("AddSpec failed: {}",
                       create_result.error_value().FormatDescription().c_str());
            return;
          }

          auto name = dev_group_name.has_value() ? dev_group_name.value() : "";
          fdf::info("Succeeded adding composite node {}.", name.c_str());
        });
  }

  fidl::ClientEnd<fdf::ResourceController> resource_controller_;
  fidl::SharedClient<fdf::CompositeNodeManager> composite_node_manager_;

  NumberServer left_server_ = NumberServer(1);
  NumberServer right_server_ = NumberServer(2);

  fidl::ServerBindingGroup<ft::Device> bindings_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
