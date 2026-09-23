// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.nodegroup.test/cpp/wire.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/nodegroupbind/test/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_nodegroup_test;
namespace bindlib = bind_fuchsia_nodegroupbind_test;

namespace {

// Name these differently than what the child expects, so we test that
// FDF renames these correctly.
const std::string_view kLeftName = "left-node";
const std::string_view kRightName = "right-node";
const std::string_view kOptionalName = "optional-node";

// Group 1 is created before creating both the left and right nodes.
fdf::CompositeNodeSpec NodeGroupOne() {
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

  auto parents = std::vector{
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_left,
          .properties = properties_left,
      }},
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_right,
          .properties = properties_right,
      }},
  };

  return {{.name = "test_group_1", .parents2 = parents}};
}

// Group 2 is created after creating the right node, but before creating the left node.
fdf::CompositeNodeSpec NodeGroupTwo() {
  auto bind_rules_left = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_TWO_LEFT),
  };

  auto properties_left = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_LEFT),
  };

  auto bind_rules_right = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_TWO_RIGHT),
  };

  auto properties_right = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_RIGHT),
  };

  auto parents = std::vector{
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_left,
          .properties = properties_left,
      }},
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_right,
          .properties = properties_right,
      }},
  };

  return {{.name = "test_group_2", .parents2 = parents}};
}

// Group 3 is created after creating both the left and right nodes.
fdf::CompositeNodeSpec NodeGroupThree() {
  auto bind_rules_left = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_THREE_LEFT),
  };

  auto properties_left = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_LEFT),
  };

  auto bind_rules_right = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_THREE_RIGHT),
  };

  auto properties_right = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_RIGHT),
  };

  auto parents = std::vector{
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_left,
          .properties = properties_left,
      }},
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_right,
          .properties = properties_right,
      }},
  };

  return {{.name = "test_group_3", .parents2 = parents}};
}

// Group 4 is created before creating the left, optional, and right nodes.
fdf::CompositeNodeSpec NodeGroupFour() {
  auto bind_rules_left = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_FOUR_LEFT),
  };

  auto properties_left = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_LEFT),
  };

  auto bind_rules_right = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_FOUR_RIGHT),
  };

  auto properties_right = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_RIGHT),
  };

  auto bind_rules_optional = std::vector{
      fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY,
                              bindlib::TEST_BIND_PROPERTY_FOUR_OPTIONAL),
  };

  auto properties_optional = std::vector{
      fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_DRIVER_OPTIONAL),
  };

  auto parents = std::vector{
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_left,
          .properties = properties_left,
      }},
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_right,
          .properties = properties_right,
      }},
      fdf::ParentSpec2{{
          .bind_rules = bind_rules_optional,
          .properties = properties_optional,
      }},

  };

  return {{.name = "test_group_4", .parents2 = parents}};
}

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
    node_client_.Bind(take_node(), dispatcher());
    // Add service "left".
    {
      auto device = [this](fidl::ServerEnd<ft::Device> server_end) mutable -> void {
        fidl::BindServer(dispatcher(), std::move(server_end), &this->left_server_);
      };
      ft::Service::InstanceHandler handler({.device = std::move(device)});
      zx::result<> status = outgoing()->AddService<ft::Service>(std::move(handler), kLeftName);
      if (status.is_error()) {
        fdf::error("Failed to add service {}", status);
      }
    }

    // Add service "right".
    {
      auto device = [this](fidl::ServerEnd<ft::Device> server_end) mutable -> void {
        fidl::BindServer(dispatcher(), std::move(server_end), &this->right_server_);
      };
      ft::Service::InstanceHandler handler({.device = std::move(device)});
      zx::result<> status = outgoing()->AddService<ft::Service>(std::move(handler), kRightName);
      if (status.is_error()) {
        fdf::error("Failed to add service {}", status);
      }
    }

    // Add service "optional".
    {
      auto device = [this](fidl::ServerEnd<ft::Device> server_end) mutable -> void {
        fidl::BindServer(dispatcher(), std::move(server_end), &this->optional_server_);
      };
      ft::Service::InstanceHandler handler({.device = std::move(device)});
      zx::result<> status = outgoing()->AddService<ft::Service>(std::move(handler), kOptionalName);
      if (status.is_error()) {
        fdf::error("Failed to add service {}", status);
      }
    }

    // Setup the node group manager client.
    auto dgm_client = context.incoming().Connect<fdf::CompositeNodeManager>();
    if (dgm_client.is_error()) {
      fdf::error("Failed to connect to NodeGroupManager: {}",
                 zx_status_get_string(dgm_client.error_value()));
      DropNode();
      return dgm_client.take_error();
    }

    composite_node_manager_.Bind(std::move(dgm_client.value()), dispatcher());

    TestGroupOne();
    TestGroupTwo();
    TestGroupThree();
    TestGroupFour();
    return zx::ok();
  }

 private:
  // Add node group
  // Add left
  // Add right
  void TestGroupOne() {
    fit::closure add_right = [this]() {
      auto right_result = AddChild(kRightName, 1, one_right_controller_,
                                   bindlib::TEST_BIND_PROPERTY_ONE_RIGHT, []() {});
      if (!right_result) {
        fdf::error("Failed to start right child.");
        DropNode();
      }
    };

    fit::closure add_left_then_right = [this, add_right = std::move(add_right)]() mutable {
      auto left_result = AddChild(kLeftName, 1, one_left_controller_,
                                  bindlib::TEST_BIND_PROPERTY_ONE_LEFT, std::move(add_right));
      if (!left_result) {
        fdf::error("Failed to start left child.");
        DropNode();
      }
    };

    AddSpec(NodeGroupOne(), std::move(add_left_then_right));
  }

  // Add right
  // Add node group
  // Add left
  void TestGroupTwo() {
    fit::closure add_left = [this]() mutable {
      auto left_result = AddChild(kLeftName, 2, two_left_controller_,
                                  bindlib::TEST_BIND_PROPERTY_TWO_LEFT, []() {});
      if (!left_result) {
        fdf::error("Failed to start left child.");
        DropNode();
      }
    };

    fit::closure add_spec_then_left = [this, add_left = std::move(add_left)]() mutable {
      AddSpec(NodeGroupTwo(), std::move(add_left));
    };

    auto right_result =
        AddChild(kRightName, 2, two_right_controller_, bindlib::TEST_BIND_PROPERTY_TWO_RIGHT,
                 std::move(add_spec_then_left));
    if (!right_result) {
      fdf::error("Failed to start right child.");
      DropNode();
    }
  }

  // Add left
  // Add right
  // Add node group
  void TestGroupThree() {
    fit::closure add_spec = [this]() mutable { AddSpec(NodeGroupThree(), []() {}); };

    fit::closure add_right_then_spec = [this, add_spec = std::move(add_spec)]() mutable {
      auto right_result = AddChild(kRightName, 3, three_right_controller_,
                                   bindlib::TEST_BIND_PROPERTY_THREE_RIGHT, std::move(add_spec));
      if (!right_result) {
        fdf::error("Failed to start right child.");
        DropNode();
      }
    };

    auto left_result =
        AddChild(kLeftName, 3, three_left_controller_, bindlib::TEST_BIND_PROPERTY_THREE_LEFT,
                 std::move(add_right_then_spec));
    if (!left_result) {
      fdf::error("Failed to start left child.");
      DropNode();
    }
  }

  // Add node group
  // Add left
  // Add optional
  // Add right
  void TestGroupFour() {
    fit::closure add_right = [this]() {
      auto right_result = AddChild(kRightName, 4, four_right_controller_,
                                   bindlib::TEST_BIND_PROPERTY_FOUR_RIGHT, []() {});
      if (!right_result) {
        fdf::error("Failed to start right child.");
        DropNode();
      }
    };

    fit::closure add_optional_then_right = [this, add_right = std::move(add_right)]() mutable {
      auto optional_result =
          AddChild(kOptionalName, 4, four_optional_controller_,
                   bindlib::TEST_BIND_PROPERTY_FOUR_OPTIONAL, std::move(add_right));
      if (!optional_result) {
        fdf::error("Failed to start optional child.");
        DropNode();
      }
    };

    fit::closure add_left_then_optional = [this, add_optional =
                                                     std::move(add_optional_then_right)]() mutable {
      auto left_result = AddChild(kLeftName, 4, four_left_controller_,
                                  bindlib::TEST_BIND_PROPERTY_FOUR_LEFT, std::move(add_optional));
      if (!left_result) {
        fdf::error("Failed to start left child.");
        DropNode();
      }
    };

    AddSpec(NodeGroupFour(), std::move(add_left_then_optional));
  }

  bool AddChild(std::string_view name, int group,
                fidl::SharedClient<fdf::NodeController>& controller, std::string_view property,
                fit::closure callback) {
    auto node_name = std::string(name) + "-" + std::to_string(group);
    // Set the properties of the node that a driver will bind to.
    fdf::NodeProperty2 node_property = fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, property);
    fdf::NodeAddArgs args({.name = node_name,
                           .offers2 = {{fdf::MakeOffer2<ft::Service>(name)}},
                           .properties2 = {{node_property}}});

    // Create endpoints of the `NodeController` for the node.
    auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
    if (endpoints.is_error()) {
      return false;
    }

    auto add_callback =
        [this, &controller, node_name, callback = std::move(callback),
         client = std::move(endpoints->client)](fidl::Result<fdf::Node::AddChild>& result) mutable {
          if (result.is_error()) {
            fdf::error("Adding child failed: {}", result.error_value().FormatDescription().c_str());
            DropNode();
            return;
          }

          controller.Bind(std::move(client), dispatcher());
          fdf::info("Successfully added child {}.", node_name.c_str());
          callback();
        };

    node_client_->AddChild({std::move(args), std::move(endpoints->server), {}})
        .Then(std::move(add_callback));
    return true;
  }

  void AddSpec(fdf::CompositeNodeSpec dev_group, fit::closure callback) {
    auto dev_group_name = dev_group.name();
    composite_node_manager_->AddSpec(std::move(dev_group))
        .Then([this, dev_group_name, callback = std::move(callback)](
                  fidl::Result<fdf::CompositeNodeManager::AddSpec>& create_result) {
          if (create_result.is_error()) {
            fdf::error("AddSpec failed: {}",
                       create_result.error_value().FormatDescription().c_str());
            DropNode();
            return;
          }

          auto name = dev_group_name.has_value() ? dev_group_name.value() : "";
          fdf::info("Succeeded adding node group {}.", name.c_str());
          callback();
        });
  }

  void DropNode() { node_client_.AsyncTeardown(); }

  fidl::SharedClient<fdf::NodeController> one_left_controller_;
  fidl::SharedClient<fdf::NodeController> one_right_controller_;

  fidl::SharedClient<fdf::NodeController> two_left_controller_;
  fidl::SharedClient<fdf::NodeController> two_right_controller_;

  fidl::SharedClient<fdf::NodeController> three_left_controller_;
  fidl::SharedClient<fdf::NodeController> three_right_controller_;

  fidl::SharedClient<fdf::NodeController> four_left_controller_;
  fidl::SharedClient<fdf::NodeController> four_right_controller_;
  fidl::SharedClient<fdf::NodeController> four_optional_controller_;

  fidl::SharedClient<fdf::Node> node_client_;
  fidl::SharedClient<fdf::CompositeNodeManager> composite_node_manager_;

  NumberServer left_server_ = NumberServer(1);
  NumberServer right_server_ = NumberServer(2);
  NumberServer optional_server_ = NumberServer(3);
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
