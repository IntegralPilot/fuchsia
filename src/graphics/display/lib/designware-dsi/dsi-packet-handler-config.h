// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DSI_PACKET_HANDLER_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DSI_PACKET_HANDLER_CONFIG_H_

#include <lib/mipi-dsi/mipi-dsi.h>
#include <zircon/assert.h>

#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"

namespace designware_dsi {

namespace internal {

constexpr bool IsDsiPixelStreamPacketFormatCompatibleWithDpiColorComponentMapping(
    mipi_dsi::DsiPixelStreamPacketFormat pixel_stream_packet_format,
    DpiColorComponentMapping dpi_color_component_mapping) {
  switch (pixel_stream_packet_format) {
    case mipi_dsi::DsiPixelStreamPacketFormat::k20BitYcbcr422:
    case mipi_dsi::DsiPixelStreamPacketFormat::k20BitYcbcr422LooselyPacked:
      return dpi_color_component_mapping == DpiColorComponentMapping::k20BitYcbcr422;
    case mipi_dsi::DsiPixelStreamPacketFormat::k24BitYcbcr422:
      return dpi_color_component_mapping == DpiColorComponentMapping::k24BitYcbcr422;
    case mipi_dsi::DsiPixelStreamPacketFormat::k16BitYcbcr422:
      return dpi_color_component_mapping == DpiColorComponentMapping::k16BitYcbcr422;
    case mipi_dsi::DsiPixelStreamPacketFormat::k30BitR10G10B10:
      return dpi_color_component_mapping == DpiColorComponentMapping::k30BitR10G10B10;
    case mipi_dsi::DsiPixelStreamPacketFormat::k36BitR12G12B12:
      return dpi_color_component_mapping == DpiColorComponentMapping::k36BitR12G12B12;
    case mipi_dsi::DsiPixelStreamPacketFormat::k12BitYcbcr420:
      return dpi_color_component_mapping == DpiColorComponentMapping::k12BitYcbcr420;
    case mipi_dsi::DsiPixelStreamPacketFormat::k16BitR5G6B5:
      return dpi_color_component_mapping == DpiColorComponentMapping::k16BitR5G6B5Config1 ||
             dpi_color_component_mapping == DpiColorComponentMapping::k16BitR5G6B5Config2 ||
             dpi_color_component_mapping == DpiColorComponentMapping::k16BitR5G6B5Config3;
    case mipi_dsi::DsiPixelStreamPacketFormat::k18BitR6G6B6:
    case mipi_dsi::DsiPixelStreamPacketFormat::k18BitR6G6B6LooselyPacked:
      return dpi_color_component_mapping == DpiColorComponentMapping::k18BitR6G6B6Config1 ||
             dpi_color_component_mapping == DpiColorComponentMapping::k18BitR6G6B6Config2;
    case mipi_dsi::DsiPixelStreamPacketFormat::k24BitR8G8B8:
      return dpi_color_component_mapping == DpiColorComponentMapping::k24BitR8G8B8;
    case mipi_dsi::DsiPixelStreamPacketFormat::kCompressed:
      return dpi_color_component_mapping == DpiColorComponentMapping::kDsc24Compressed;
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid MIPI-DSI pixel format: %d",
                      static_cast<int>(pixel_stream_packet_format));
}

}  // namespace internal

// Configures the DSI Packet Handler for packet timing and packet contents.
struct DsiPacketHandlerConfig {
  mipi_dsi::DsiVideoModePacketSequencing packet_sequencing;

  // The pixel format (data type) of pixel packets in Video mode.
  //
  // `pixel_stream_packet_format` must match the DPI input pixel type for
  // valid instances.
  mipi_dsi::DsiPixelStreamPacketFormat pixel_stream_packet_format;

  // True iff the packet that the DSI Packet Handler generates is compatible
  // // with the `dpi_color_component_mapping`.
  constexpr bool IsValid(DpiColorComponentMapping dpi_color_component_mapping) const;
};

constexpr bool DsiPacketHandlerConfig::IsValid(
    DpiColorComponentMapping dpi_color_component_mapping) const {
  if (!internal::IsDsiPixelStreamPacketFormatCompatibleWithDpiColorComponentMapping(
          pixel_stream_packet_format, dpi_color_component_mapping)) {
    return false;
  }
  return true;
}

}  // namespace designware_dsi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DSI_PACKET_HANDLER_CONFIG_H_
