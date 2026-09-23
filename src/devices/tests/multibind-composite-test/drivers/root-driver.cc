// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/multibind-composite-test/drivers/root-driver.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>
#include <bind/multibind/test/cpp/bind.h>

namespace root_driver {

namespace {

// Node a bind rules and properties.
const ddk::BindRule kNodeARules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD),
    ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, static_cast<uint32_t>(1)),
};

const device_bind_prop_t kNodeAProperties[] = {
    ddk::MakeProperty(bind_multibind_test::NODE_ID, bind_multibind_test::NODE_ID_A),
};

// Node b bind rules and properties.
const ddk::BindRule kNodeBRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD),
    ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, static_cast<uint32_t>(2)),
};

const device_bind_prop_t kNodeBProperties[] = {
    ddk::MakeProperty(bind_multibind_test::NODE_ID, bind_multibind_test::NODE_ID_B),
};

// Node c bind rules and properties.
const ddk::BindRule kNodeCRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD),
    ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, static_cast<uint32_t>(3)),
};

const device_bind_prop_t kNodeCProperties[] = {
    ddk::MakeProperty(bind_multibind_test::NODE_ID, bind_multibind_test::NODE_ID_C),
};

// Node d bind rules and properties.
const ddk::BindRule kNodeDRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD),
    ddk::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, static_cast<uint32_t>(4)),
};

const device_bind_prop_t kNodeDProperties[] = {
    ddk::MakeProperty(bind_multibind_test::NODE_ID, bind_multibind_test::NODE_ID_D),
};

}  // namespace

zx_status_t RootDriver::Bind(void* ctx, zx_device_t* dev) {
  auto root_dev = std::make_unique<RootDriver>(dev);
  zx_status_t status = root_dev->DdkAdd(ddk::DeviceAddArgs("root"));
  if (status != ZX_OK) {
    return status;
  }

  const std::string kNodes[] = {"node_a", "node_b", "node_c", "node_d"};
  uint32_t instance_id = 1;
  for (auto& node : kNodes) {
    zx_device_str_prop_t node_props[] = {
        ddk::MakeStrProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD),
        ddk::MakeStrProperty(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, instance_id++),
    };

    auto node_dev = std::make_unique<RootDriver>(dev);
    status = node_dev->DdkAdd(ddk::DeviceAddArgs(node.c_str())
                                  .set_str_props(node_props)
                                  .set_proto_id(bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD)
                                  .set_flags(DEVICE_ADD_ALLOW_MULTI_COMPOSITE));
    if (status != ZX_OK) {
      return status;
    }
    [[maybe_unused]] auto node_dev_ptr = node_dev.release();
  }

  // Add composite node spec 1.
  status = root_dev->DdkAddCompositeNodeSpec("spec_1",
                                             ddk::CompositeNodeSpec(kNodeARules, kNodeAProperties)
                                                 .AddParentSpec(kNodeCRules, kNodeCProperties)
                                                 .AddParentSpec(kNodeDRules, kNodeDProperties));
  if (status != ZX_OK) {
    return status;
  }

  // Add composite node spec 2.
  status = root_dev->DdkAddCompositeNodeSpec("spec_2",
                                             ddk::CompositeNodeSpec(kNodeDRules, kNodeDProperties)
                                                 .AddParentSpec(kNodeBRules, kNodeBProperties));
  if (status != ZX_OK) {
    return status;
  }

  [[maybe_unused]] auto ptr = root_dev.release();

  return ZX_OK;
}

void RootDriver::DdkRelease() { delete this; }

static zx_driver_ops_t root_driver_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = RootDriver::Bind;
  return ops;
}();

}  // namespace root_driver

ZIRCON_DRIVER(RootDriver, root_driver::root_driver_driver_ops, "zircon", "0.1");
