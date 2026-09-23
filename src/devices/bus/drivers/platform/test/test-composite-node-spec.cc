// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>

#include "test.h"

namespace board_test {

zx_status_t TestBoard::CompositeNodeSpecInit() {
  fuchsia_hardware_platform_bus::Node dev;
  dev.name() = "node_a";
  dev.vid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST;
  dev.pid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_PBUS_TEST;
  dev.did() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_COMPOSITE_NODE_SPEC;

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('DVGP');

  auto bind_rules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                              bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                              bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_PBUS_TEST),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                              bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_PARENT_SPEC),
  };

  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_DEVICE),
  };

  auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = bind_rules,
          .properties = properties,
      }},
  };

  auto composite_node_spec = fuchsia_driver_framework::CompositeNodeSpec{
      {.name = "composite_node_spec", .parents2 = parents}};
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, dev), fidl::ToWire(fidl_arena, composite_node_spec));

  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec composite_node_spec(dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec composite_node_spec(dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace board_test
