// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file is in C++, as it interfaces with C++-only libraries.

#include "third_party/iwlwifi/platform/ieee80211.h"

#include <fidl/fuchsia.wlan.ieee80211/cpp/wire_types.h>

#include "third_party/driver-lib/wlan/channel.h"
#include "third_party/driver-lib/wlan/ieee80211.h"

size_t ieee80211_get_header_len(const struct ieee80211_frame_header* fw) {
  return ieee80211_hdrlen(fw);
}

struct ieee80211_hw* ieee80211_alloc_hw(size_t priv_data_len, const struct ieee80211_ops* ops) {
  return nullptr;
}

fuchsia_wlan_ieee80211::wire::WlanBand convert_wlan_band_to_fidl(wlan_band_t band) {
  switch (band) {
    case WLAN_BAND_TWO_GHZ:
      return fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz;
    case WLAN_BAND_FIVE_GHZ:
      return fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz;
    default:
      return static_cast<fuchsia_wlan_ieee80211::wire::WlanBand>(band);
  }
}

bool ieee80211_is_valid_chan(struct wlan_channel_number primary) {
  wlan::common::Channel chan = {
      .primary =
          {
              .band = convert_wlan_band_to_fidl(primary.band),
              .number = primary.number,
          },
      .bandwidth = fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
  };

  return wlan::common::IsValidChan(chan);
}

uint16_t ieee80211_get_center_freq(struct wlan_channel_number ch_num) {
  wlan::common::Channel chan = {
      .primary =
          {
              .band = convert_wlan_band_to_fidl(ch_num.band),
              .number = ch_num.number,
          },
      .bandwidth = fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20,
  };

  return wlan::common::GetCenterFreq(chan);
}

bool ieee80211_has_protected(const struct ieee80211_frame_header* fh) {
  return ieee80211_pkt_is_protected(fh);
}

bool ieee80211_is_data(const struct ieee80211_frame_header* fh) {
  return ieee80211_get_frame_type(fh) == IEEE80211_FRAME_TYPE_DATA;
}

bool ieee80211_is_data_present(const struct ieee80211_frame_header* fh) {
  return ieee80211_is_data(fh) &&
         ((static_cast<uint8_t>(ieee80211_get_frame_subtype(fh)) & 0x40) == 0);
}

bool ieee80211_is_data_qos(const struct ieee80211_frame_header* fh) {
  return ieee80211_is_qos_data(fh);
}

uint8_t ieee80211_get_tid(const struct ieee80211_frame_header* fh) {
  const uint8_t* qos_ctl = reinterpret_cast<const uint8_t*>(fh) + ieee80211_get_qos_ctrl_offset(fh);
  return qos_ctl[0] & 0xF;
}

bool ieee80211_is_back_req(const struct ieee80211_frame_header* fh) {
  return (ieee80211_get_frame_type(fh) == IEEE80211_FRAME_TYPE_CTRL) &&
         (ieee80211_get_frame_subtype(fh) == IEEE80211_FRAME_SUBTYPE_BACK_REQ);
}
