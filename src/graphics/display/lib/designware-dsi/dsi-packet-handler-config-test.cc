// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-dsi/dsi-packet-handler-config.h"

#include <lib/mipi-dsi/mipi-dsi.h>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"

namespace designware_dsi {

namespace {

TEST(DsiPacketHandlerConfigTest, ValidityFor20BitYcbcr422Loosely) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format =
          mipi_dsi::DsiPixelStreamPacketFormat::k20BitYcbcr422LooselyPacked,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor24BitYcbcr422) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k24BitYcbcr422,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor16BitYcbcr422) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k16BitYcbcr422,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor30BitR10G10B10) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k30BitR10G10B10,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor36BitR12G12B12) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k36BitR12G12B12,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor12BitYcbcr420) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k12BitYcbcr420,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor16BitR5G6B5) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k16BitR5G6B5,
  };
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor18BitR6G6B6) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k18BitR6G6B6,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor18BitR6G6B6Loosely) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k18BitR6G6B6LooselyPacked,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityFor24BitR8G8B8) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k24BitR8G8B8,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

TEST(DsiPacketHandlerConfigTest, ValidityForCompressed) {
  static constexpr DsiPacketHandlerConfig kConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::kCompressed,
  };
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitR5G6B5Config3));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config1));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k18BitR6G6B6Config2));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitR8G8B8));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k20BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k24BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k16BitYcbcr422));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k30BitR10G10B10));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k36BitR12G12B12));
  EXPECT_FALSE(kConfig.IsValid(DpiColorComponentMapping::k12BitYcbcr420));
  EXPECT_TRUE(kConfig.IsValid(DpiColorComponentMapping::kDsc24Compressed));
}

}  // namespace

}  // namespace designware_dsi
