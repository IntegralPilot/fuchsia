// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/ddk/metadata.h>
#include <lib/device-protocol/display-panel.h>

#include "goodix-gt6853.h"

namespace touch {

namespace {

// There are three config files, one for each DDIC. A config file may contain multiple configs; the
// correct one is chosen based on the sensor ID reported by the touch controller.
inline const char* GetPanelTouchConfigPath(display::PanelType panel_type) {
  switch (panel_type) {
    case display::PanelType::kKdKd070d82FitipowerJd9364:
    case display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson:
      return GT6853_CONFIG_9364_PATH;
    case display::PanelType::kKdKd070d82FitipowerJd9365:
    case display::PanelType::kBoeTv070wsmFitipowerJd9365:
      return GT6853_CONFIG_9365_PATH;
    default:
      return nullptr;
  }
}

inline const char* GetPanelName(display::PanelType panel_type) {
  switch (panel_type) {
    case display::PanelType::kKdKd070d82FitipowerJd9364:
      return "kd_fiti9364";
    case display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson:
      return "boe_fiti9364";
    case display::PanelType::kKdKd070d82FitipowerJd9365:
      return "kd_fiti9365";
    case display::PanelType::kBoeTv070wsmFitipowerJd9365:
      return "boe_fiti9365";
    default:
      return "unknown";
  }
}

}  // namespace

zx::result<display::PanelType> Gt6853Device::GetDisplayPanelType() {
  size_t actual = 0;
  display::PanelType panel_type;
  zx_status_t status = DdkGetFragmentMetadata("pdev", DEVICE_METADATA_DISPLAY_PANEL_TYPE,
                                              &panel_type, sizeof(display::PanelType), &actual);
  if (status != ZX_OK) {
    zxlogf(WARNING, "Failed to get panel type from panel config metadata: %s",
           zx_status_get_string(status));
    return zx::error(status);
  }
  if (actual != sizeof(display::PanelType)) {
    zxlogf(WARNING, "Expected metadata size %zu, got %zu", sizeof(display::PanelType), actual);
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok(panel_type);
}

zx::result<fuchsia_mem::wire::Range> Gt6853Device::GetConfigFileVmo() {
  zx::result<display::PanelType> panel_type_result = GetDisplayPanelType();

  if (panel_type_result.is_error()) {
    // The only case to let through is when metadata isn't provided, which
    // could happen after netbooting. All other unexpected conditions are
    // fatal, which should help them be discovered more easily.
    if (panel_type_result.status_value() == ZX_ERR_NOT_FOUND) {
      config_status_.Set("skipped, no metadata");
      return zx::ok(fuchsia_mem::wire::Range{});
    }
    zxlogf(ERROR, "Failed to get panel type: %s", panel_type_result.status_string());
    return panel_type_result.take_error();
  }

  display::PanelType panel_type = std::move(panel_type_result).value();

  // The panel can only be identified correctly by the bootloader for EVT boards
  // and beyond. This driver won't be used on boards earlier than EVT, so not
  // finding the panel ID is an error.
  if (panel_type == display::PanelType::kUnknown) {
    zxlogf(ERROR, "Display panel type unknown");
    return zx::error(ZX_ERR_INTERNAL);
  }

  panel_type_id_ = root_.CreateUint("panel_type_id", static_cast<uint32_t>(panel_type));
  panel_type_ = root_.CreateString("panel_type", GetPanelName(panel_type));

  const char* config_path = GetPanelTouchConfigPath(panel_type);
  if (!config_path) {
    zxlogf(ERROR, "Failed to find config for panel type %u", panel_type);
    return zx::error(ZX_ERR_INTERNAL);
  }

  // There's a chance we can proceed without a config, but we should always have one on Nelson, so
  // error out if it can't be loaded.
  fuchsia_mem::wire::Range config = {};
  zx_status_t status =
      load_firmware(parent(), config_path, config.vmo.reset_and_get_address(), &config.size);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to load config binary: %d", status);
    return zx::error(status);
  }

  return zx::ok(std::move(config));
}

}  // namespace touch
