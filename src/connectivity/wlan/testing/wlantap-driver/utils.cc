// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "utils.h"

#include <fuchsia/wlan/common/cpp/fidl.h>

#include <wlan/common/channel.h>
#include <wlan/common/element.h>

#include "fidl/fuchsia.wlan.common/cpp/common_types.h"
#include "fidl/fuchsia.wlan.ieee80211/cpp/natural_types.h"
#include "fidl/fuchsia.wlan.softmac/cpp/natural_types.h"

namespace wlan {

void ConvertTapPhyConfig(fuchsia_wlan_softmac::WlanSoftmacQueryResponse* resp,
                         const fuchsia_wlan_tap::WlantapPhyConfig& tap_phy_config) {
  resp->sta_addr(tap_phy_config.sta_addr());
  resp->mac_role(tap_phy_config.mac_role());
  resp->supported_phys(tap_phy_config.supported_phys());
  resp->hardware_capability(tap_phy_config.hardware_capability());

  size_t const band_cap_count =
      std::min(tap_phy_config.bands().size(), static_cast<size_t>(fuchsia_wlan_common::kMaxBands));

  std::vector<fuchsia_wlan_softmac::WlanSoftmacBandCapability> softmac_band_caps;

  for (size_t i = 0; i < band_cap_count; i++) {
    auto tap_band_caps = tap_phy_config.bands()[i];
    fuchsia_wlan_softmac::WlanSoftmacBandCapability softmac_band_cap;
    softmac_band_cap.band(tap_band_caps.band());

    if (tap_band_caps.ht_caps().has_value()) {
      softmac_band_cap.ht_caps(tap_band_caps.ht_caps().value());
    }

    if (tap_band_caps.vht_caps().has_value()) {
      softmac_band_cap.vht_caps(tap_band_caps.vht_caps().value());
    }

    auto basic_rate_count = std::min<size_t>(tap_band_caps.rates().size(),
                                             fuchsia_wlan_ieee80211::kMaxSupportedBasicRates);
    std::vector<uint8_t> basic_rates(basic_rate_count);
    std::copy(tap_band_caps.rates().begin(), tap_band_caps.rates().begin() + basic_rate_count,
              basic_rates.begin());
    softmac_band_cap.basic_rates(std::move(basic_rates));

    auto primary_channel_count =
        std::min<size_t>(tap_band_caps.primary_channels().size(),
                         fuchsia::wlan::ieee80211::MAX_UNIQUE_CHANNEL_NUMBERS);
    std::vector<fuchsia_wlan_ieee80211::ChannelNumber> primary_channels(
        tap_band_caps.primary_channels().begin(),
        tap_band_caps.primary_channels().begin() + primary_channel_count);
    softmac_band_cap.primary_channels(std::move(primary_channels));
    softmac_band_caps.push_back(std::move(softmac_band_cap));
  }

  resp->band_caps(std::move(softmac_band_caps));
}

std::string RoleToString(fuchsia_wlan_common::WlanMacRole role) {
  switch (role) {
    case fuchsia_wlan_common::WlanMacRole::kClient:
      return "client";
    case fuchsia_wlan_common::WlanMacRole::kAp:
      return "ap";
    case fuchsia_wlan_common::WlanMacRole::kMesh:
      return "mesh";
    default:
      return "invalid";
  }
}

}  // namespace wlan
