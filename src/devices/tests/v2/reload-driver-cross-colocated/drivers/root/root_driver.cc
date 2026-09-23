// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/reloaddriverbind/test/cpp/bind.h>

#include "src/devices/tests/v2/reload-driver-cross-colocated/driver_helpers.h"

namespace bindlib = bind_fuchsia_reloaddriverbind_test;
namespace helpers = reload_test_driver_helpers;

namespace {

class RootDriver : public fdf::DriverBase2 {
 public:
  RootDriver() : fdf::DriverBase2("root") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    node_client_.Bind(take_node());

    auto parent_right = fuchsia_driver_framework::ParentSpec2{{
        .bind_rules =
            {
                fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY,
                                        bindlib::TEST_BIND_PROPERTY_TARGET_2),
            },
        .properties =
            {
                fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY,
                                   bindlib::TEST_BIND_PROPERTY_TARGET_2),
            },
    }};

    auto parent_left = fuchsia_driver_framework::ParentSpec2{{
        .bind_rules =
            {
                fdf::MakeAcceptBindRule(bindlib::TEST_BIND_PROPERTY,
                                        bindlib::TEST_BIND_PROPERTY_LEAF),
            },
        .properties =
            {
                fdf::MakeProperty2(bindlib::TEST_BIND_PROPERTY, bindlib::TEST_BIND_PROPERTY_LEAF),
            },
    }};

    auto spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "child_b",
        .parents2 = {{
            parent_right,
            parent_left,
        }},
        .driver_host = "shared-host",
    }};

    auto cnm_client = incoming_ptr->Connect<fuchsia_driver_framework::CompositeNodeManager>();
    if (cnm_client.is_error()) {
      FDF_LOGL(ERROR, logger(), "Failed to connect to CompositeNodeManager: %s",
               cnm_client.status_string());
      return cnm_client.take_error();
    }

    fidl::SyncClient<fuchsia_driver_framework::CompositeNodeManager> composite_node_manager(
        std::move(cnm_client.value()));
    auto result = composite_node_manager->AddSpec(spec);
    if (result.is_error()) {
      FDF_LOGL(ERROR, logger(), "Failed to AddSpec: %s",
               result.error_value().FormatDescription().c_str());
      return zx::error(ZX_ERR_INTERNAL);
    }

    zx::result res_left = helpers::AddChild(logger(), "left_parent", node_client_,
                                            bindlib::TEST_BIND_PROPERTY_LEFT_PARENT);
    if (res_left.is_error()) {
      return res_left.take_error();
    }
    left_parent_controller_.Bind(std::move(res_left.value()));

    zx::result res_right = helpers::AddChild(logger(), "right_parent", node_client_,
                                             bindlib::TEST_BIND_PROPERTY_RIGHT_PARENT);
    if (res_right.is_error()) {
      return res_right.take_error();
    }
    right_parent_controller_.Bind(std::move(res_right.value()));

    return helpers::SendAck(logger(), context.node_name().value_or("None"), incoming_ptr, name());
  }

 private:
  fidl::SyncClient<fuchsia_driver_framework::Node> node_client_;
  fidl::SyncClient<fuchsia_driver_framework::NodeController> left_parent_controller_;
  fidl::SyncClient<fuchsia_driver_framework::NodeController> right_parent_controller_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(RootDriver);
