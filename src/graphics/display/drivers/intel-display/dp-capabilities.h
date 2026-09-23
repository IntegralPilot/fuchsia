// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_CAPABILITIES_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_CAPABILITIES_H_

#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/inspect.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <optional>
#include <vector>

#include "src/graphics/display/drivers/intel-display/dp-aux-channel.h"
#include "src/graphics/display/drivers/intel-display/dpcd.h"

namespace intel_display {

// DpCapabilities is a utility for reading and storing DisplayPort capabilities
// supported by the display based on a copy of read-only DPCD capability
// registers. Drivers can also use PublishToInspect() to publish the data to
// inspect.
class DpCapabilities final {
 public:
  // Initializes the DPCD capability array with all zeros and the EDP DPCD capabilities as
  // non-present.
  DpCapabilities();

  // Allow copy.
  DpCapabilities(const DpCapabilities&) = default;
  DpCapabilities& operator=(const DpCapabilities&) = default;

  // Allow move.
  DpCapabilities(DpCapabilities&&) = default;
  DpCapabilities& operator=(DpCapabilities&&) = default;

  // Read and parse DPCD capabilities. Clears any previously initialized content
  static fpromise::result<DpCapabilities> Read(DpAuxChannel* dp_aux_channel);

  // Publish the capabilities fields to inspect node `caps_node`.
  void PublishToInspect(inspect::Node* caps_node) const;

  // Get the cached value of a DPCD register using its DPCD address.
  uint8_t dpcd_at(dpcd::Register address) const {
    ZX_ASSERT(address < dpcd::DPCD_SUPPORTED_LINK_RATE_START);
    return dpcd_[address - dpcd::DPCD_CAP_START];
  }

  // Get the cached value of a EDP DPCD register using its address. Asserts if the eDP capabilities
  // are not available.
  uint8_t edp_dpcd_at(dpcd::EdpRegister address) const {
    ZX_ASSERT(edp_dpcd_.has_value());
    ZX_ASSERT(address < dpcd::DPCD_EDP_RESERVED);
    ZX_ASSERT(address >= dpcd::DPCD_EDP_CAP_START);
    return edp_dpcd_->bytes[address - dpcd::DPCD_EDP_CAP_START];
  }

  template <typename T, dpcd::Register A>
  T dpcd_reg() const {
    T reg;
    reg.set_reg_value(dpcd_at(A));
    return reg;
  }

  // Asserts if eDP capabilities are not available.
  template <typename T, dpcd::EdpRegister A>
  T edp_dpcd_reg() const {
    T reg;
    reg.set_reg_value(edp_dpcd_at(A));
    return reg;
  }

  dpcd::Revision dpcd_revision() const {
    return static_cast<dpcd::Revision>(dpcd_[dpcd::DPCD_REV]);
  }

  std::optional<dpcd::EdpRevision> edp_revision() const {
    if (edp_dpcd_) {
      return edp_dpcd_->revision;
    }
    return std::nullopt;
  }

  // Total number of stream sinks within this Sink device.
  size_t sink_count() const { return sink_count_.count(); }

  // Maximum number of DisplayPort lanes.
  uint8_t max_lane_count() const { return max_lane_count_.lane_count_set(); }

  // True for SST mode displays that support the Enhanced Framing symbol sequence (see DP v1.4a
  // Section 2.2.1.2).
  bool enhanced_frame_capability() const { return max_lane_count_.enhanced_frame_enabled(); }

  // True for eDP displays that support the `backlight_enable` bit in the
  // dpcd::DPCD_EDP_DISPLAY_CTRL register (see dpcd.h).
  bool backlight_aux_power() const { return edp_dpcd_ && edp_dpcd_->backlight_aux_power; }

  // True for eDP displays that support backlight adjustment through the
  // dpcd::DPCD_EDP_BACKLIGHT_BRIGHTNESS_[MSB|LSB] registers.
  bool backlight_aux_brightness() const { return edp_dpcd_ && edp_dpcd_->backlight_aux_brightness; }

  // The list of supported link rates in ascending order, measured in units of Mbps/lane.
  const std::vector<uint32_t>& supported_link_rates_mbps() const {
    return supported_link_rates_mbps_;
  }

  // True if the contents of vector returned by `supported_link_rates_mbps()` was populated using
  // the  "Link Rate Table" method. If true, the link rate must be selected by writing the vector
  // index to the DPCD LINK_RATE_SET register. Otherwise, the selected link rate must be programmed
  // using the DPCD LINK_BW_SET register.
  bool use_link_rate_table() const { return use_link_rate_table_; }

 private:
  // DpCapabilities that are only present in eDP displays.
  struct Edp {
    Edp();

    std::array<uint8_t, dpcd::DPCD_EDP_RESERVED - dpcd::DPCD_EDP_CAP_START> bytes;
    dpcd::EdpRevision revision;
    bool backlight_aux_power = false;
    bool backlight_aux_brightness = false;
  };

  bool ProcessEdp(DpAuxChannel* dp_aux_channel);
  bool ProcessSupportedLinkRates(DpAuxChannel* dp_aux_channel);

  std::array<uint8_t, dpcd::DPCD_SUPPORTED_LINK_RATE_START - dpcd::DPCD_CAP_START> dpcd_;
  dpcd::SinkCount sink_count_;
  dpcd::LaneCount max_lane_count_;
  std::vector<uint32_t> supported_link_rates_mbps_;
  bool use_link_rate_table_ = false;

  std::optional<Edp> edp_dpcd_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_CAPABILITIES_H_
