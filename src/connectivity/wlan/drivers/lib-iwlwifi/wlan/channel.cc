// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "channel.h"

#include <zircon/assert.h>

namespace fwlan_ieee80211 = ::fuchsia_wlan_ieee80211::wire;

// TODO(porce): Look up constants from the operating class table.
// No need to use constexpr in this prototype.
namespace wlan {
namespace common {

bool IsValidChan2Ghz(const Channel& channel) {
  if (channel.primary.band != fwlan_ieee80211::WlanBand::kTwoGhz) {
    return false;
  }

  uint8_t p = channel.primary.number;

  if (p < 1 || p > 14) {
    return false;
  }

  switch (channel.bandwidth) {
    case fwlan_ieee80211::ChannelBandwidth::kCbw20:
      return true;
    case fwlan_ieee80211::ChannelBandwidth::kCbw40:
      return (p <= 7);
    case fwlan_ieee80211::ChannelBandwidth::kCbw40Below:
      return (p >= 5);
    default:
      return false;
  }
}

bool IsValidChan5Ghz(const Channel& channel) {
  if (channel.primary.band != fwlan_ieee80211::WlanBand::kFiveGhz) {
    return false;
  }

  uint8_t p = channel.primary.number;

  if (p < 36 || p > 173) {
    return false;
  }
  if (p > 64 && p < 100) {
    return false;
  }
  if (p > 144 && p < 149) {
    return false;
  }
  if (p <= 144 && (p % 4 != 0)) {
    return false;
  }
  if (p >= 149 && (p % 4 != 1)) {
    return false;
  }

  switch (channel.bandwidth) {
    case fwlan_ieee80211::ChannelBandwidth::kCbw20:
      break;
    case fwlan_ieee80211::ChannelBandwidth::kCbw40:
      if (p <= 144 && (p % 8 != 4)) {
        return false;
      }
      if (p >= 149 && (p % 8 != 5)) {
        return false;
      }
      break;
    case fwlan_ieee80211::ChannelBandwidth::kCbw40Below:
      if (p <= 144 && (p % 8 != 0)) {
        return false;
      }
      if (p >= 149 && (p % 8 != 1)) {
        return false;
      }
      break;
    case fwlan_ieee80211::ChannelBandwidth::kCbw80:
      if (p == 165) {
        return false;
      }
      break;
    case fwlan_ieee80211::ChannelBandwidth::kCbw80P80: {
      if (channel.vht_secondary_80_channel.band != channel.primary.band) {
        return false;
      }
      uint8_t s = channel.vht_secondary_80_channel.number;
      if (!(s == 42 || s == 58 || s == 106 || s == 122 || s == 138 || s == 155)) {
        return false;
      }

      uint8_t ccfs0 = GetCenterChanIdx(channel);
      uint8_t ccfs1 = s;
      uint8_t gap = (ccfs0 >= ccfs1) ? (ccfs0 - ccfs1) : (ccfs1 - ccfs0);
      if (gap <= 16) {
        return false;
      }
      break;
    }
    case fwlan_ieee80211::ChannelBandwidth::kCbw160: {
      if (p >= 132) {
        return false;
      }
      break;
    }
    default:
      return false;
  }

  return true;
}

bool IsValidChan(const Channel& channel) {
  return IsValidChan2Ghz(channel) || IsValidChan5Ghz(channel);
}

Mhz GetCenterFreq(const Channel& channel) {
  ZX_DEBUG_ASSERT(IsValidChan(channel));

  Mhz spacing = 5;
  Mhz channel_starting_frequency;
  if (channel.primary.band == fwlan_ieee80211::WlanBand::kTwoGhz) {
    channel_starting_frequency = kBaseFreq2Ghz;
  } else {
    // 5 GHz
    channel_starting_frequency = kBaseFreq5Ghz;
  }

  // IEEE Std 802.11-2016, 21.3.14
  return channel_starting_frequency + spacing * GetCenterChanIdx(channel);
}

// Returns the channel index corresponding to the first frequency segment's
// center frequency
uint8_t GetCenterChanIdx(const Channel& channel) {
  uint8_t p = channel.primary.number;
  switch (channel.bandwidth) {
    case fwlan_ieee80211::ChannelBandwidth::kCbw20:
      return p;
    case fwlan_ieee80211::ChannelBandwidth::kCbw40:
      return p + 2;
    case fwlan_ieee80211::ChannelBandwidth::kCbw40Below:
      return p - 2;
    case fwlan_ieee80211::ChannelBandwidth::kCbw80:
    case fwlan_ieee80211::ChannelBandwidth::kCbw80P80:
      if (p <= 48) {
        return 42;
      } else if (p <= 64) {
        return 58;
      } else if (p <= 112) {
        return 106;
      } else if (p <= 128) {
        return 122;
      } else if (p <= 144) {
        return 138;
      } else if (p <= 161) {
        return 155;
      } else {
        // Not reachable
        return p;
      }
    case fwlan_ieee80211::ChannelBandwidth::kCbw160:
      // See IEEE Std 802.11-2016 Table 9-252 and 9-253.
      // Note CBW160 has only one frequency segment, regardless of
      // encodings on CCFS0 and CCFS1 in VHT Operation Information IE.
      if (p <= 64) {
        return 50;
      } else if (p <= 128) {
        return 114;
      } else {
        // Not reachable
        return p;
      }
    default:
      return channel.primary.number;
  }
}

const char* CbwSuffix(fwlan_ieee80211::ChannelBandwidth cbw) {
  switch (cbw) {
    case fwlan_ieee80211::ChannelBandwidth::kCbw20:
      return "";  // Vanilla plain 20 MHz bandwidth
    case fwlan_ieee80211::ChannelBandwidth::kCbw40:
      return "+";  // SCA, often denoted by "+1"
    case fwlan_ieee80211::ChannelBandwidth::kCbw40Below:
      return "-";  // SCB, often denoted by "-1"
    case fwlan_ieee80211::ChannelBandwidth::kCbw80:
      return "V";  // VHT 80 MHz
    case fwlan_ieee80211::ChannelBandwidth::kCbw160:
      return "W";  // VHT Wave2 160 MHz
    case fwlan_ieee80211::ChannelBandwidth::kCbw80P80:
      return "P";  // VHT Wave2 80Plus80 (not often obvious, but P is the first
                   // alphabet)
    default:
      return "?";  // Invalid
  }
}

const char* CbwStr(fwlan_ieee80211::ChannelBandwidth cbw) {
  switch (cbw) {
    case fwlan_ieee80211::ChannelBandwidth::kCbw20:
      return "CBW20";
    case fwlan_ieee80211::ChannelBandwidth::kCbw40:
      return "CBW40";
    case fwlan_ieee80211::ChannelBandwidth::kCbw40Below:
      return "CBW40B";
    case fwlan_ieee80211::ChannelBandwidth::kCbw80:
      return "CBW80";
    case fwlan_ieee80211::ChannelBandwidth::kCbw160:
      return "CBW160";
    case fwlan_ieee80211::ChannelBandwidth::kCbw80P80:
      return "CBW80P80";
    default:
      return "Invalid";
  }
}

std::string ChanStr(const Channel& channel) {
  char buf[8 + 1];
  if (channel.bandwidth != fwlan_ieee80211::ChannelBandwidth::kCbw80P80) {
    std::snprintf(buf, sizeof(buf), "%u%s", channel.primary.number, CbwSuffix(channel.bandwidth));
  } else {
    std::snprintf(buf, sizeof(buf), "%u+%u%s", channel.primary.number,
                  channel.vht_secondary_80_channel.number, CbwSuffix(channel.bandwidth));
  }
  return std::string(buf);
}

std::string ChanStrLong(const Channel& channel) {
  char buf[16 + 1];
  if (channel.bandwidth != fwlan_ieee80211::ChannelBandwidth::kCbw80P80) {
    std::snprintf(buf, sizeof(buf), "%u %s", channel.primary.number, CbwStr(channel.bandwidth));
  } else {
    std::snprintf(buf, sizeof(buf), "%u+%u %s", channel.primary.number,
                  channel.vht_secondary_80_channel.number, CbwStr(channel.bandwidth));
  }
  return std::string(buf);
}

}  // namespace common
}  // namespace wlan
