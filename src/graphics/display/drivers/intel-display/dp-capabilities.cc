// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/dp-capabilities.h"

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <algorithm>
#include <cstdint>
#include <string>

#include "src/graphics/display/drivers/intel-display/dp-aux-channel.h"
#include "src/graphics/display/drivers/intel-display/dpcd.h"

namespace intel_display {

namespace {

std::string DpcdRevisionToString(dpcd::Revision rev) {
  switch (rev) {
    case dpcd::Revision::k1_0:
      return "DPCD r1.0";
    case dpcd::Revision::k1_1:
      return "DPCD r1.1";
    case dpcd::Revision::k1_2:
      return "DPCD r1.2";
    case dpcd::Revision::k1_3:
      return "DPCD r1.3";
    case dpcd::Revision::k1_4:
      return "DPCD r1.4";
  }
  return "unknown";
}

std::string EdpDpcdRevisionToString(dpcd::EdpRevision rev) {
  switch (rev) {
    case dpcd::EdpRevision::k1_1:
      return "eDP v1.1 or lower";
    case dpcd::EdpRevision::k1_2:
      return "eDP v1.2";
    case dpcd::EdpRevision::k1_3:
      return "eDP v1.3";
    case dpcd::EdpRevision::k1_4:
      return "eDP v1.4";
    case dpcd::EdpRevision::k1_4a:
      return "eDP v1.4a";
    case dpcd::EdpRevision::k1_4b:
      return "eDP v1.4b";
  }
  return "unknown";
}

}  // namespace

DpCapabilities::DpCapabilities() { dpcd_.fill(0); }

DpCapabilities::Edp::Edp() { bytes.fill(0); }

// static
fpromise::result<DpCapabilities> DpCapabilities::Read(DpAuxChannel* dp_aux_channel) {
  DpCapabilities caps;

  if (!dp_aux_channel->DpcdRead(dpcd::DPCD_CAP_START, caps.dpcd_.data(), caps.dpcd_.size())) {
    fdf::trace("Failed to read dpcd capabilities");
    return fpromise::error();
  }

  auto dsp_present =
      caps.dpcd_reg<dpcd::DownStreamPortPresent, dpcd::DPCD_DOWN_STREAM_PORT_PRESENT>();
  if (dsp_present.is_branch()) {
    auto dsp_count = caps.dpcd_reg<dpcd::DownStreamPortCount, dpcd::DPCD_DOWN_STREAM_PORT_COUNT>();
    fdf::debug("Found branch with {} ports", dsp_count.count());
  }

  if (!dp_aux_channel->DpcdRead(dpcd::DPCD_SINK_COUNT, caps.sink_count_.reg_value_ptr(), 1)) {
    fdf::error("Failed to read DisplayPort sink count");
    return fpromise::error();
  }

  caps.max_lane_count_ = caps.dpcd_reg<dpcd::LaneCount, dpcd::DPCD_MAX_LANE_COUNT>();
  if (caps.max_lane_count() != 1 && caps.max_lane_count() != 2 && caps.max_lane_count() != 4) {
    fdf::error("Unsupported DisplayPort lane count: {}", caps.max_lane_count());
    return fpromise::error();
  }

  if (!caps.ProcessEdp(dp_aux_channel)) {
    return fpromise::error();
  }

  if (!caps.ProcessSupportedLinkRates(dp_aux_channel)) {
    return fpromise::error();
  }

  ZX_ASSERT(!caps.supported_link_rates_mbps_.empty());
  return fpromise::ok(std::move(caps));
}

bool DpCapabilities::ProcessEdp(DpAuxChannel* dp_aux_channel) {
  // Check if the Display Control registers reserved for eDP are available.
  auto edp_config = dpcd_reg<dpcd::EdpConfigCap, dpcd::DPCD_EDP_CONFIG>();
  if (!edp_config.dpcd_display_ctrl_capable()) {
    return true;
  }

  fdf::trace("eDP registers are available");

  edp_dpcd_.emplace();
  if (!dp_aux_channel->DpcdRead(dpcd::DPCD_EDP_CAP_START, edp_dpcd_->bytes.data(),
                                edp_dpcd_->bytes.size())) {
    fdf::error("Failed to read eDP capabilities");
    return false;
  }

  edp_dpcd_->revision = dpcd::EdpRevision(edp_dpcd_at(dpcd::DPCD_EDP_REV));

  auto general_cap1 = edp_dpcd_reg<dpcd::EdpGeneralCap1, dpcd::DPCD_EDP_GENERAL_CAP1>();
  auto backlight_cap = edp_dpcd_reg<dpcd::EdpBacklightCap, dpcd::DPCD_EDP_BACKLIGHT_CAP>();

  edp_dpcd_->backlight_aux_power =
      general_cap1.tcon_backlight_adjustment_cap() && general_cap1.backlight_aux_enable_cap();
  edp_dpcd_->backlight_aux_brightness =
      general_cap1.tcon_backlight_adjustment_cap() && backlight_cap.brightness_aux_set_cap();

  return true;
}

bool DpCapabilities::ProcessSupportedLinkRates(DpAuxChannel* dp_aux_channel) {
  ZX_ASSERT(supported_link_rates_mbps_.empty());

  // According to eDP v1.4b, Table 4-24, a device supporting eDP version v1.4 and higher can support
  // link rate selection by way of both the DPCD MAX_LINK_RATE register and the "Link Rate Table"
  // method via DPCD SUPPORTED_LINK_RATES registers.
  //
  // The latter method can represent more values than the former (which is limited to only 4
  // discrete values). Hence we attempt to use the "Link Rate Table" method first.
  use_link_rate_table_ = false;
  if (edp_dpcd_ && edp_dpcd_->revision >= dpcd::EdpRevision::k1_4) {
    constexpr size_t kBufferSize =
        dpcd::DPCD_SUPPORTED_LINK_RATE_END - dpcd::DPCD_SUPPORTED_LINK_RATE_START + 1;
    std::array<uint8_t, kBufferSize> link_rates;
    if (dp_aux_channel->DpcdRead(dpcd::DPCD_SUPPORTED_LINK_RATE_START, link_rates.data(),
                                 kBufferSize)) {
      for (size_t i = 0; i < link_rates.size(); i += 2) {
        uint16_t value = link_rates[i] | (static_cast<uint16_t>(link_rates[i + 1] << 8));

        // From the eDP specification: "A table entry containing the value 0 indicates that the
        // entry and all entries at higher DPCD addressess contain invalid link rates."
        if (value == 0) {
          break;
        }

        // Each valid entry indicates a nominal per-lane link rate equal to `value * 200kHz`. We
        // convert value to MHz: `value * 200 / 1000 ==> value / 5`.
        supported_link_rates_mbps_.push_back(value / 5);
      }
    }

    use_link_rate_table_ = !supported_link_rates_mbps_.empty();
  }

  // Fall back to the MAX_LINK_RATE register if the Link Rate Table method is not supported.
  if (supported_link_rates_mbps_.empty()) {
    uint32_t max_link_rate = dpcd_reg<dpcd::LinkBw, dpcd::DPCD_MAX_LINK_RATE>().link_bw();

    // All link rates including and below the maximum are supported.
    switch (max_link_rate) {
      case dpcd::LinkBw::k8100Mbps:
        supported_link_rates_mbps_.push_back(8100);
        __FALLTHROUGH;
      case dpcd::LinkBw::k5400Mbps:
        supported_link_rates_mbps_.push_back(5400);
        __FALLTHROUGH;
      case dpcd::LinkBw::k2700Mbps:
        supported_link_rates_mbps_.push_back(2700);
        __FALLTHROUGH;
      case dpcd::LinkBw::k1620Mbps:
        supported_link_rates_mbps_.push_back(1620);
        break;
      case 0:
        fdf::error("Device did not report supported link rates");
        return false;
      default:
        fdf::error("Unsupported max link rate: {}", max_link_rate);
        return false;
    }

    // Make sure the values are in ascending order.
    std::reverse(supported_link_rates_mbps_.begin(), supported_link_rates_mbps_.end());
  }

  return true;
}

void DpCapabilities::PublishToInspect(inspect::Node* caps_node) const {
  ZX_ASSERT(caps_node);
  caps_node->RecordString("dpcd_revision", DpcdRevisionToString(dpcd_revision()));
  caps_node->RecordUint("sink_count", sink_count());
  caps_node->RecordUint("max_lane_count", max_lane_count());

  {
    auto node = caps_node->CreateUintArray("supported_link_rates_mbps_per_lane",
                                           supported_link_rates_mbps().size());
    for (size_t i = 0; i < supported_link_rates_mbps().size(); ++i) {
      node.Add(i, supported_link_rates_mbps()[i]);
    }
    caps_node->Record(std::move(node));
  }

  {
    const std::string value =
        edp_revision().has_value() ? EdpDpcdRevisionToString(*edp_revision()) : "not supported";
    caps_node->RecordString("edp_revision", value);
  }
}

}  // namespace intel_display
