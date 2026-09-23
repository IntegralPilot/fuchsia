// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test.h"

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/platform-defs.h>

#include "test-resources.h"

namespace board_test {
namespace fpbus = fuchsia_hardware_platform_bus;

zx_status_t TestBoard::TestInit() {
  fpbus::Node test_dev;
  test_dev.name() = "test-parent";
  test_dev.vid() = PDEV_VID_TEST;
  test_dev.pid() = PDEV_PID_PBUS_TEST;
  test_dev.did() = PDEV_DID_TEST_PARENT;

  fpbus::UserspaceIrq irq_spec{{
      .irq = 42,
      .controller_id = 1,
  }};
  fpbus::Irq irq{{
      .irq = fpbus::IrqSpec::WithUserspaceIrq(std::move(irq_spec)),
      .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
  }};
  test_dev.irq() = std::vector<fpbus::Irq>{std::move(irq)};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('TEST');
  auto result = pbus_.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, test_dev));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: DeviceAdd Test request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: DeviceAdd Test failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t TestBoard::InterruptControllerInit() {
  fpbus::Node int_dev;
  int_dev.name() = "test-interrupt-controller";
  int_dev.vid() = PDEV_VID_TEST;
  int_dev.pid() = PDEV_PID_PBUS_TEST;
  int_dev.did() = PDEV_DID_TEST_INTERRUPT_CONTROLLER;
  int_dev.interrupt_controller_id() = 1;

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('TEST');
  auto result = pbus_.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, int_dev));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: DeviceAdd Interrupt Controller request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: DeviceAdd Interrupt Controller failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

}  // namespace board_test
