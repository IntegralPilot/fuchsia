// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/vout-dsi.h"

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/common.h"
#include "src/graphics/display/drivers/amlogic-display/display-timing-mode-conversion.h"
#include "src/graphics/display/drivers/amlogic-display/logging.h"

namespace amlogic_display {

namespace {
constexpr display::ModeId kDsiDefaultModeId(1);
}  // namespace

zx::result<std::unique_ptr<VoutDsi>> VoutDsi::Create(fdf::Namespace& incoming,
                                                     display::PanelType panel_type,
                                                     inspect::Node node) {
  fdf::info("Fixed panel type is {}", static_cast<uint32_t>(panel_type));
  const PanelConfig* panel_config = GetPanelConfig(panel_type);
  if (panel_config == nullptr) {
    fdf::error("Failed to get panel config for panel {}", static_cast<uint32_t>(panel_type));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx::result<std::unique_ptr<DsiHost>> dsi_host_result =
      DsiHost::Create(incoming, panel_type, panel_config);
  if (dsi_host_result.is_error()) {
    fdf::error("Could not create DSI host: {}", dsi_host_result);
    return dsi_host_result.take_error();
  }
  std::unique_ptr<DsiHost> dsi_host = std::move(dsi_host_result).value();

  static constexpr char kPdevFragmentName[] = "pdev";
  zx::result<fidl::ClientEnd<fuchsia_hardware_platform_device::Device>> pdev_result =
      incoming.Connect<fuchsia_hardware_platform_device::Service::Device>(kPdevFragmentName);
  if (pdev_result.is_error()) {
    fdf::error("Failed to get the pdev client: {}", pdev_result);
    return pdev_result.take_error();
  }
  fidl::ClientEnd<fuchsia_hardware_platform_device::Device> platform_device =
      std::move(pdev_result).value();
  if (!platform_device.is_valid()) {
    fdf::error("Failed to get a valid platform device client");
    return zx::error(ZX_ERR_INTERNAL);
  }

  zx::result<std::unique_ptr<Clock>> clock_result =
      Clock::Create(platform_device, kBootloaderDisplayEnabled);
  if (clock_result.is_error()) {
    fdf::error("Could not create Clock: {}", clock_result);
    return clock_result.take_error();
  }
  std::unique_ptr<Clock> clock = std::move(clock_result).value();

  fbl::AllocChecker alloc_checker;
  std::unique_ptr<VoutDsi> vout = fbl::make_unique_checked<VoutDsi>(
      &alloc_checker, std::move(dsi_host), std::move(clock), panel_config, std::move(node));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for VoutDsi.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(vout));
}

zx::result<std::unique_ptr<VoutDsi>> VoutDsi::CreateForTesting(display::PanelType panel_type) {
  const PanelConfig* panel_config = GetPanelConfig(panel_type);
  if (panel_config == nullptr) {
    fdf::error("Failed to get panel config for panel {}", static_cast<uint32_t>(panel_type));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  fbl::AllocChecker alloc_checker;
  std::unique_ptr<VoutDsi> vout = fbl::make_unique_checked<VoutDsi>(
      &alloc_checker,
      /*dsi_host=*/nullptr, /*clock=*/nullptr, panel_config, inspect::Node{});
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for VoutDsi.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(vout));
}

VoutDsi::VoutDsi(std::unique_ptr<DsiHost> dsi_host, std::unique_ptr<Clock> dsi_clock,
                 const PanelConfig* panel_config, inspect::Node node)
    : node_(std::move(node)),
      dsi_host_(std::move(dsi_host)),
      clock_(std::move(dsi_clock)),
      panel_config_(*panel_config),
      mode_and_id_({
          .id = kDsiDefaultModeId,
          .mode = ToDisplayMode(panel_config->display_timing),
      }) {
  ZX_DEBUG_ASSERT(panel_config != nullptr);
  node_.RecordInt("vout_type", static_cast<int>(type()));
}

bool VoutDsi::SupportsHotplugDetection() const { return false; }

AddedDisplayInfo VoutDsi::CreateAddedDisplayInfo(display::DisplayId display_id) {
  return {
      .display_id = display_id,
      .preferred_modes = {mode_and_id_},
  };
}

zx::result<> VoutDsi::UpdateStateOnDisplayConnected() { return zx::ok(); }

void VoutDsi::DisplayDisconnected() {}

std::optional<display::Mode> VoutDsi::GetDisplayMode(display::ModeId mode_id) const {
  ZX_DEBUG_ASSERT(mode_id != display::kInvalidModeId);
  if (mode_id == mode_and_id_.id()) {
    return mode_and_id_.mode();
  }
  return std::nullopt;
}

zx::result<> VoutDsi::SubmitConfiguration(display::ModeId mode_id) {
  ZX_DEBUG_ASSERT_MSG(mode_id == mode_and_id_.id(), "Unsupported DSI mode ID: %" PRIu16,
                      mode_id.value());
  return zx::ok();
}

zx::result<> VoutDsi::PowerOff() {
  clock_->Disable();
  dsi_host_->Disable();
  return zx::ok();
}

zx::result<> VoutDsi::PowerOn() {
  zx::result<> clock_enable_result = clock_->Enable(panel_config_);
  if (!clock_enable_result.is_ok()) {
    fdf::error("Could not enable display clocks: {}", clock_enable_result);
    return clock_enable_result;
  }

  clock_->SetVideoOn(false);
  // Configure and enable DSI host interface.
  zx::result<> dsi_host_enable_result = dsi_host_->Enable(clock_->GetBitrate());
  if (!dsi_host_enable_result.is_ok()) {
    fdf::error("Could not enable DSI Host: {}", dsi_host_enable_result);
    return dsi_host_enable_result;
  }
  clock_->SetVideoOn(true);
  return zx::ok();
}

zx::result<> VoutDsi::SetFrameVisibility(bool frame_visible) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void VoutDsi::Dump() { LogPanelConfig(panel_config_); }

}  // namespace amlogic_display
