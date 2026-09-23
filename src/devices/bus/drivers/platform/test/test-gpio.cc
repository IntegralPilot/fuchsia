// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pinimpl/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>

#include "test.h"

#define DECL_GPIO_PIN(x)     \
  {                          \
    {                        \
      .pin = (x), .name = #x \
    }                        \
  }

namespace board_test {
namespace fpbus = fuchsia_hardware_platform_bus;

zx_status_t TestBoard::GpioInit() {
  static const std::vector<fuchsia_hardware_pinimpl::Pin> kGpioPins = {
      DECL_GPIO_PIN(1),
      DECL_GPIO_PIN(3),
      DECL_GPIO_PIN(5),
  };

  static const fuchsia_hardware_pinimpl::Metadata kMetadata{{.pins = kGpioPins}};

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> gpio_metadata{
      {{
          .id = fuchsia_hardware_pinimpl::Metadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  fpbus::Node gpio_dev{{
      .name = "gpio",
      .vid = PDEV_VID_TEST,
      .pid = PDEV_PID_PBUS_TEST,
      .did = PDEV_DID_TEST_GPIO,
      .metadata = std::move(gpio_metadata),
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('TGPI');
  auto result = pbus_.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, gpio_dev));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: DeviceAdd Gpio request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: DeviceAdd Gpio failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace board_test
