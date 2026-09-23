// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_IWLWIFI_WLAN_CHANNEL_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_IWLWIFI_WLAN_CHANNEL_H_

#include <fidl/fuchsia.wlan.ieee80211/cpp/wire_types.h>

#include <cstdint>
#include <string>

namespace wlan {
namespace common {

struct Channel;

typedef uint16_t Mhz;

// IEEE Std 802.11-2016, Annex E
// Note the distinction of index for primary20 and index for center frequency.
// Fuchsia minimizes the use of the notion of center frequency,
// with following exceptions:
// - CBW80P80's secondary frequency segment
// - Frequency conversion at device drivers
constexpr Mhz kBaseFreq2Ghz = 2407;
constexpr Mhz kBaseFreq5Ghz = 5000;

bool IsValidChan2Ghz(const Channel& channel);
bool IsValidChan5Ghz(const Channel& channel);
bool IsValidChan(const Channel& channel);

Mhz GetCenterFreq(const Channel& channel);
uint8_t GetCenterChanIdx(const Channel& channel);

std::string ChanStr(const Channel& channel);
std::string ChanStrLong(const Channel& channel);

struct Channel {
  fuchsia_wlan_ieee80211::wire::ChannelNumber primary;
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth bandwidth;
  fuchsia_wlan_ieee80211::wire::ChannelNumber vht_secondary_80_channel;
};

const char* CbwSuffix(fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw);
const char* CbwStr(fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw);

}  // namespace common
}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_IWLWIFI_WLAN_CHANNEL_H_
