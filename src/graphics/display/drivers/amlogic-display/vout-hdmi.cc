// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/vout-hdmi.h"

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <algorithm>
#include <ranges>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/display-timing-mode-conversion.h"

namespace amlogic_display {

namespace {

std::optional<display::DisplayTiming> GetDisplayTimingFromModeId(
    std::span<const display::DisplayTiming> timings, display::ModeId mode_id) {
  ZX_DEBUG_ASSERT(mode_id != display::kInvalidModeId);
  size_t index = mode_id.value() - 1;
  if (index >= timings.size()) {
    return std::nullopt;
  }
  return timings[index];
}

}  // namespace

zx::result<std::unique_ptr<VoutHdmi>> VoutHdmi::Create(fdf::Namespace& incoming, inspect::Node node,
                                                       uint8_t visual_debug_level) {
  zx::result<std::unique_ptr<HdmiHost>> hdmi_host_result = HdmiHost::Create(incoming);
  if (hdmi_host_result.is_error()) {
    fdf::error("Could not create HDMI host: {}", hdmi_host_result);
    return hdmi_host_result.take_error();
  }

  fbl::AllocChecker alloc_checker;
  std::unique_ptr<VoutHdmi> vout = fbl::make_unique_checked<VoutHdmi>(
      &alloc_checker, std::move(hdmi_host_result).value(), std::move(node), visual_debug_level);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for VoutHdmi.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(vout));
}

VoutHdmi::VoutHdmi(std::unique_ptr<HdmiHost> hdmi_host, inspect::Node node,
                   uint8_t visual_debug_level)
    : node_(std::move(node)),
      hdmi_host_(std::move(hdmi_host)),
      visual_debug_level_(visual_debug_level) {
  node_.RecordInt("vout_type", static_cast<int>(type()));
}

bool VoutHdmi::SupportsHotplugDetection() const { return true; }

AddedDisplayInfo VoutHdmi::CreateAddedDisplayInfo(display::DisplayId display_id) {
  ZX_DEBUG_ASSERT(!timings_.is_empty());
  fbl::Vector<display::ModeAndId> preferred_modes;
  fbl::AllocChecker alloc_checker;

  const size_t preferred_modes_count =
      std::min(timings_.size(), size_t{AddedDisplayInfo::kMaxPreferredModes});
  preferred_modes.reserve(timings_.size(), &alloc_checker);
  ZX_DEBUG_ASSERT(alloc_checker.check());

  for (uint16_t i = 0; i < preferred_modes_count; ++i) {
    ZX_DEBUG_ASSERT_MSG(preferred_modes.size() < preferred_modes.capacity(),
                        "The push_back() below was not supposed to allocate memory, but it might");
    preferred_modes.push_back(
        display::ModeAndId({.id = display::ModeId(i + 1), .mode = ToDisplayMode(timings_[i])}),
        &alloc_checker);
    ZX_DEBUG_ASSERT_MSG(alloc_checker.check(),
                        "The push_back() above failed to allocate memory; "
                        "it was not supposed to allocate at all");
  }

  return {
      .display_id = display_id,
      .preferred_modes = std::move(preferred_modes),
  };
}

zx::result<> VoutHdmi::UpdateStateOnDisplayConnected() {
  // For HDMI, this involves reading the EDID and determining the supported display modes.
  auto read_extended_edid_result = hdmi_host_->ReadExtendedEdid();
  if (read_extended_edid_result.is_error()) {
    // HdmiTransmitter::ReadExtendedEdid() already logs errors.
    return read_extended_edid_result.take_error();
  }
  fit::result<const char*, edid::Edid> edid_result =
      edid::Edid::Create(std::move(read_extended_edid_result).value());
  if (edid_result.is_error()) {
    fdf::error("Failed to parse EDID: {}", edid_result.error_value());
    return zx::error(ZX_ERR_INTERNAL);
  }
  edid::Edid edid = std::move(edid_result).value();

  zx::result<fbl::Vector<display::DisplayTiming>> edid_timings_result =
      edid.GetSupportedDisplayTimings();
  if (edid_timings_result.is_error()) {
    fdf::error("Failed to get supported display timings from EDID: {}",
               edid_timings_result.status_string());
    return edid_timings_result.take_error();
  }
  fbl::Vector<display::DisplayTiming> edid_timings = std::move(edid_timings_result).value();

  // Filter and shrink edid_timings to contain only supported timings.
  auto removed_subrange =
      std::ranges::remove_if(edid_timings, [&](const display::DisplayTiming& timing) {
        return !hdmi_host_->IsDisplayTimingSupported(timing);
      });
  ZX_DEBUG_ASSERT(removed_subrange.size() >= 0);
  ZX_DEBUG_ASSERT(removed_subrange.size() <= edid_timings.size());
  edid_timings.resize(edid_timings.size() - removed_subrange.size());

  if (edid_timings.is_empty()) {
    fdf::error("None of the EDID timings is supported. The new display cannot be added.");
    return zx::error(ZX_ERR_INTERNAL);
  }

  edid_ = std::move(edid);
  timings_ = std::move(edid_timings);

  // A new connected display is not yet set up with any display timing.
  current_mode_id_ = display::kInvalidModeId;
  return zx::ok();
}

void VoutHdmi::DisplayDisconnected() { hdmi_host_->HostOff(); }

std::optional<display::Mode> VoutHdmi::GetDisplayMode(display::ModeId mode_id) const {
  ZX_DEBUG_ASSERT(mode_id != display::kInvalidModeId);
  std::optional<display::DisplayTiming> get_timing_result =
      GetDisplayTimingFromModeId(timings_, mode_id);
  if (!get_timing_result.has_value()) {
    return std::nullopt;
  }
  return ToDisplayMode(get_timing_result.value());
}

zx::result<> VoutHdmi::SubmitConfiguration(display::ModeId mode_id) {
  ZX_DEBUG_ASSERT(mode_id != display::kInvalidModeId);
  if (mode_id == current_mode_id_) {
    return zx::ok();
  }

  std::optional<display::DisplayTiming> timing_result =
      GetDisplayTimingFromModeId(timings_, mode_id);
  ZX_DEBUG_ASSERT(timing_result.has_value());

  display::DisplayTiming timing = std::move(timing_result).value();
  ZX_DEBUG_ASSERT(hdmi_host_->IsDisplayTimingSupported(timing));

  zx_status_t status = hdmi_host_->ModeSet(timing);
  if (status != ZX_OK) {
    fdf::error("Failed to set HDMI display timing: {}", zx::make_result(status));
    return zx::error(status);
  }

  current_mode_id_ = mode_id;
  return zx::ok();
}

zx::result<> VoutHdmi::PowerOff() {
  hdmi_host_->HostOff();
  return zx::ok();
}

zx::result<> VoutHdmi::PowerOn() {
  zx::result<> hdmi_host_on_result = zx::make_result(hdmi_host_->HostOn());
  if (!hdmi_host_on_result.is_ok()) {
    fdf::error("Could not enable HDMI host: {}", hdmi_host_on_result);
    return hdmi_host_on_result;
  }

  // Powering on the display panel also resets the display mode set on the
  // display. This clears the display mode set previously to force a Vout
  // modeset to be performed on the next SubmitConfiguration().
  current_mode_id_ = display::kInvalidModeId;
  return zx::ok();
}

zx::result<> VoutHdmi::SetFrameVisibility(bool frame_visible) {
  // On HDMI video output, when the frames are invisible, the encoder
  // outputs a **green** background indicating that the display engine
  // front end is idle.
  static constexpr uint8_t kVisualDebugLevelInfoProduct = 1;

  // The following values are calculated using the conversion formulas defined
  // in the following standards:
  //
  // Rec. ITU-R BT.709-6, Parameter values for the HDTV1 standards for
  // production and international programme exchange, June 2015.
  // - Section 3 "Signal format", item 3.4 "Quantization of RGB, luminance
  //   and colour-difference signals", page 4.
  // - Section 3 "Signal format", item 3.5 "Derivation of luminance and
  //   colour difference signals via quantized RGB signals", page 4.

  // Black (R = 0, G = 0, B = 0) is (Y = 0, Cb = 512, Cr = 512) in YCbCr.
  static constexpr YCbCrColor kBlack = {.y = 0, .cb = 512, .cr = 512};

  // Green (R = 0, G = 128, B = 0) is (Y = 378, Cb = 339, Cr = 308) in YCbCr.
  static constexpr YCbCrColor kGreen = {.y = 378, .cb = 339, .cr = 308};

  if (visual_debug_level_ >= kVisualDebugLevelInfoProduct) {
    hdmi_host_->ReplaceEncoderPixelColorWithColor(!frame_visible, kGreen);
  } else {
    hdmi_host_->ReplaceEncoderPixelColorWithColor(!frame_visible, kBlack);
  }
  return zx::ok();
}

void VoutHdmi::Dump() {
  if (current_mode_id_ == display::kInvalidModeId) {
    fdf::info("No display mode is currently set.");
    return;
  }
  std::optional<display::DisplayTiming> current_timing_optional =
      GetDisplayTimingFromModeId(timings_, current_mode_id_);
  ZX_DEBUG_ASSERT(current_timing_optional.has_value());
  display::DisplayTiming timing = std::move(current_timing_optional).value();

  fdf::info("HDMI Display Timing:");

  fdf::info("horizontal_active_px = {}", timing.horizontal_active_px);
  fdf::info("horizontal_front_porch_px = {}", timing.horizontal_front_porch_px);
  fdf::info("horizontal_sync_width_px = {}", timing.horizontal_sync_width_px);
  fdf::info("horizontal_back_porch_px = {}", timing.horizontal_back_porch_px);
  fdf::info("vertical_active_lines = {}", timing.vertical_active_lines);
  fdf::info("vertical_front_porch_lines = {}", timing.vertical_front_porch_lines);
  fdf::info("vertical_sync_width_lines = {}", timing.vertical_sync_width_lines);
  fdf::info("vertical_back_porch_lines = {}", timing.vertical_back_porch_lines);
  fdf::info("pixel_clock_frequency_hz = {}", timing.pixel_clock_frequency_hz);
  fdf::info("fields_per_frame (enum) = {}", static_cast<uint32_t>(timing.fields_per_frame));
  fdf::info("hsync_polarity (enum) = {}", static_cast<uint32_t>(timing.hsync_polarity));
  fdf::info("vsync_polarity (enum) = {}", static_cast<uint32_t>(timing.vsync_polarity));
  fdf::info("vblank_alternates = {}", timing.vblank_alternates);
  fdf::info("pixel_repetition = {}", timing.pixel_repetition);
}

}  // namespace amlogic_display
