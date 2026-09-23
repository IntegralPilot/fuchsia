// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.dictionaryoffers.test/cpp/wire.h>
#include <fidl/fuchsia.driver.framework/cpp/common_types_format.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/nodegroupbind/test/cpp/bind.h>

namespace ft = fuchsia_dictionaryoffers_test;
namespace bindlib = bind_fuchsia_nodegroupbind_test;

namespace {

fuchsia_driver_framework::CompositeNodeSpec NodeGroupOne() {
  auto bind_rules_left = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_ONE_LEFT),
  };

  auto properties_left = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_LEFT),
  };

  auto bind_rules_right = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_ONE_RIGHT),
  };

  auto properties_right = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_RIGHT),
  };

  auto bind_rules_opt = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY,
                              bindlib::TEST_BIND_PROPERTY_FOUR_OPTIONAL),
  };

  auto properties_opt = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_OPTIONAL),
  };

  auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = bind_rules_left,
          .properties = properties_left,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = bind_rules_right,
          .properties = properties_right,
      }},
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = bind_rules_opt,
          .properties = properties_opt,
      }},
  };

  return {{.name = "test_group_1", .parents2 = parents}};
}

class RootDriver final : public fdf::DriverBase2, public fidl::WireServer<ft::ControlPlane> {
 public:
  RootDriver() : fdf::DriverBase2("root") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto control = [this](fidl::ServerEnd<ft::ControlPlane> server_end) -> void {
      fidl::BindServer(dispatcher(), std::move(server_end), this);
    };

    ft::ControlService::InstanceHandler handler({.control = std::move(control)});

    auto result = outgoing()->AddService<ft::ControlService>(std::move(handler));
    if (result.is_error()) {
      fdf::error("Failed to add Device service: {}", result.status_string());
      return result.take_error();
    }

    auto dgm_client = context.incoming().Connect<fuchsia_driver_framework::CompositeNodeManager>();
    if (dgm_client.is_error()) {
      fdf::error("Failed to connect to NodeGroupManager: {}",
                 zx_status_get_string(dgm_client.error_value()));
      return dgm_client.take_error();
    }

    fidl::Arena arena;
    fidl::WireResult add_spec_result =
        fidl::WireCall(*dgm_client)->AddSpec(fidl::ToWire(arena, NodeGroupOne()));
    if (!add_spec_result.ok()) {
      fdf::error("AddSpec call failed: {}", add_spec_result.FormatDescription());
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (add_spec_result->is_error()) {
      fdf::error("AddSpec failed: {}", add_spec_result->error_value());
      return zx::error(ZX_ERR_INTERNAL);
    }

    return zx::ok();
  }

  void Stop(fdf::StopCompleter completer) override {
    fdf::info("Stop");
    completer(zx::ok());
  }

 private:
  // fidl::WireServer<ft::ControlPlane>
  void AddChild(ft::wire::ControlPlaneAddChildRequest* request,
                AddChildCompleter::Sync& completer) override {
    fdf::info("adding child...");
    auto [node_controller_client_end, node_controller_server_end] =
        fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
    // We want to add an extra offer to the right child to make sure the child can connect to
    // both parents when one of them has a dictionary.
    if (request->args.name().get() == "right") {
      ZX_ASSERT(!request->args.has_offers2());
      auto offers = fidl::VectorView<fuchsia_driver_framework::wire::Offer>(arena_, 1);
      offers[0] = fdf::MakeOffer2<ft::ControlService>(arena_);
      request->args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena_)
                          .name(request->args.name())
                          .properties2(request->args.properties2())
                          .offers2(offers)
                          .Build();
    }
    fidl::WireResult result =
        fidl::WireCall(node())->AddChild(request->args, std::move(node_controller_server_end), {});

    if (!result.ok()) {
      completer.ReplyError(fuchsia_driver_framework::wire::NodeError::kInternal);
      return;
    }

    if (result->is_error()) {
      completer.ReplyError(result->error_value());
      return;
    }

    completer.ReplySuccess();
  }

  void Check(CheckCompleter::Sync& completer) override { completer.Reply(); }

  fidl::Arena<> arena_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
