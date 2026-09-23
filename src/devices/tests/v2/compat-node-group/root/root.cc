// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This header has to come first, and we define our ZX_PROTOCOL, so that
// we don't have to edit protodefs.h to add this test protocol.
#include <bind/fuchsia/compat/cpp/bind.h>
#define ZX_PROTOCOL_PARENT bind_fuchsia_compat::BIND_PROTOCOL_PARENT

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>

#include "src/devices/tests/v2/compat-node-group/root/root.h"

namespace root {

zx_status_t Root::Bind(void* ctx, zx_device_t* dev) {
  auto root_dev = std::make_unique<Root>(dev);
  auto status = root_dev->DdkAdd(ddk::DeviceAddArgs("root"));
  if (status != ZX_OK) {
    return status;
  }

  const uint32_t node_1_bind_rule_1_values[] = {10, 3};
  const ddk::BindRule node_1_bind_rules[] = {
      ddk::MakeAcceptBindRuleList("peep", node_1_bind_rule_1_values),
      ddk::MakeRejectBindRule("sandpiper", true),
  };

  const device_bind_prop_t node_1_properties[] = {
      ddk::MakeProperty(bind_fuchsia::PROTOCOL, 100u),
      ddk::MakeProperty(bind_fuchsia::USB_VID, 20u),
  };

  const uint32_t node_2_props_values_1[] = {88, 99};
  const ddk::BindRule node_2_bind_rules[] = {
      ddk::MakeAcceptBindRuleList(bind_fuchsia::PLATFORM_DEV_VID, node_2_props_values_1),
      ddk::MakeRejectBindRule(bind_fuchsia::PLATFORM_DEV_DID, 10u),
  };

  const device_bind_prop_t node_2_properties[] = {
      ddk::MakeProperty(bind_fuchsia::PROTOCOL, 20u),
  };

  status = root_dev->DdkAddCompositeNodeSpec(
      "test_composite", ddk::CompositeNodeSpec(node_1_bind_rules, node_1_properties)
                            .AddParentSpec(node_2_bind_rules, node_2_properties));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto ptr = root_dev.release();

  // Add a child that matches the first node group node.
  zx_device_str_prop_t node_props_1[] = {ddk::MakeStrProperty("peep", 10u)};
  auto node_dev_1 = std::make_unique<Root>(dev);
  status = node_dev_1->DdkAdd(ddk::DeviceAddArgs("parent_a")
                                  .set_str_props(node_props_1)
                                  .set_proto_id(bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto node_1_ptr = node_dev_1.release();

  // Add a child that matches the other node group node.
  zx_device_str_prop_t node_props_2[] = {
      ddk::MakeStrProperty(bind_fuchsia::PLATFORM_DEV_VID, 88u),
  };
  auto node_dev_2 = std::make_unique<Root>(dev);
  status = node_dev_2->DdkAdd(ddk::DeviceAddArgs("parent_b")
                                  .set_str_props(node_props_2)
                                  .set_proto_id(bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD));
  if (status != ZX_OK) {
    return status;
  }
  [[maybe_unused]] auto node_2_ptr = node_dev_2.release();

  return ZX_OK;
}

void Root::DdkRelease() { delete this; }

static zx_driver_ops_t root_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Root::Bind;
  return ops;
}();

}  // namespace root

ZIRCON_DRIVER(Root, root::root_ops, "zircon", "0.1");
