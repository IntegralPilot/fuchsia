// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bus/drivers/platform/test/power-integration-test/test-power-integration-board.h"

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>

#include "tools/power_config/lib/cpp/power_config.h"

namespace power_integration_board {

zx::result<> PowerIntegrationBoard::Start(fdf::DriverContext context) {
  zx::result<fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>> client =
      context.incoming().Connect<fuchsia_hardware_platform_bus::Service::PlatformBus>();

  if (client.is_error()) {
    return zx::error_result(ZX_ERR_ACCESS_DENIED);
  }

  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus> &platform_bus = client.value();

  fdf::Arena arena('TEST');
  auto board_info = platform_bus.buffer(arena)->GetBoardInfo();

  if (board_info->is_error()) {
    return zx::error_result(ZX_ERR_IO);
  }

  {
    // Load our power config.
    zx::result open_result = context.incoming().Open<fuchsia_io::File>(
        "/pkg/data/power_config.fidl", fuchsia_io::Flags::kPermReadBytes);
    if (!open_result.is_ok() || !open_result->is_valid()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    zx::result config = power_config::Load(std::move(open_result.value()));
    if (config.is_error()) {
      return config.take_error();
    }

    fuchsia_hardware_platform_bus::Node pdev;
    pdev.name() = "composite_fake_child";
    pdev.vid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST;
    pdev.pid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST;
    pdev.did() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_PLATFORM_DEVICE_CHILD;
    pdev.power_config() = std::move(config.value().power_elements());

    auto bind_rules = std::vector{
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                                bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                                bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST),
        fdf::MakeAcceptBindRule(
            bind_fuchsia::PLATFORM_DEV_DID,
            bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_CHILD)};

    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_CHILD)};

    auto parents = std::vector{fuchsia_driver_framework::ParentSpec2{
        {.bind_rules = bind_rules, .properties = properties}}};

    auto composite_node_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "composite_fake_child",
        .parents2 = parents,
    }};

    auto result = platform_bus.buffer(arena)->AddCompositeNodeSpec(
        fidl::ToWire(arena, pdev), fidl::ToWire(arena, composite_node_spec));

    if (!result.ok()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (result->is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  {
    fuchsia_hardware_platform_bus::Node parent_pdev;
    parent_pdev.name() = "pdev_parent";
    parent_pdev.vid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST;
    parent_pdev.pid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST;
    parent_pdev.did() =
        bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_PLATFORM_DEVICE_PARENT;

    fuchsia_hardware_power::PowerLevel off;
    off.level() = 0;
    off.name() = "off";
    off.transitions() =
        std::vector<fuchsia_hardware_power::Transition>{fuchsia_hardware_power::Transition{{
            .target_level = 1,
            .latency_us = 200,
        }}};

    fuchsia_hardware_power::PowerLevel on;
    on.level() = 1;
    on.name() = "on";
    on.transitions() =
        std::vector<fuchsia_hardware_power::Transition>{fuchsia_hardware_power::Transition{{
            .target_level = 0,
            .latency_us = 5,
        }}};

    fuchsia_hardware_power::PowerElementConfiguration config;
    config.element() = fuchsia_hardware_power::PowerElement{{
        .name = "pe-fake-parent",
        .levels =
            std::vector<fuchsia_hardware_power::PowerLevel>{
                on,
                off,
            },

    }};

    parent_pdev.power_config() = std::vector<fuchsia_hardware_power::PowerElementConfiguration>{
        config,
    };

    auto bind_rules = std::vector{
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                                bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                                bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST),
        fdf::MakeAcceptBindRule(
            bind_fuchsia::PLATFORM_DEV_DID,
            bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_PARENT)};

    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_PARENT)};

    auto parents = std::vector{fuchsia_driver_framework::ParentSpec2{
        {.bind_rules = bind_rules, .properties = properties}}};

    auto composite_node_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "composite_fake_parent",
        .parents2 = parents,
    }};

    auto result = platform_bus.buffer(arena)->AddCompositeNodeSpec(
        fidl::ToWire(arena, parent_pdev), fidl::ToWire(arena, composite_node_spec));

    if (!result.ok()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (result->is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  // Create the parent device
  {
    fuchsia_hardware_platform_bus::Node fake_parent_device;
    fake_parent_device.name() = "fake_parent";
    fake_parent_device.vid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST;
    fake_parent_device.pid() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_POWER_TEST;
    fake_parent_device.did() = bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_FAKE_POWER_PARENT;

    auto result = platform_bus.buffer(arena)->NodeAdd(fidl::ToWire(arena, fake_parent_device));

    if (!result.ok()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (result->is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  return zx::ok();
}

}  // namespace power_integration_board

FUCHSIA_DRIVER_EXPORT2(power_integration_board::PowerIntegrationBoard);
