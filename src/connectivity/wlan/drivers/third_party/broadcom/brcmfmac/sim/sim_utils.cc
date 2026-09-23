// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_utils.h"

#include <netinet/in.h>
#include <zircon/assert.h>

#include <algorithm>

#include <wlan/drivers/macaddr.h>

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac::sim_utils {

zx_status_t WriteEthernetFrame(cpp20::span<uint8_t> out, MacAddr dst, MacAddr src, uint16_t type,
                               cpp20::span<const uint8_t> body) {
  if (out.size() < body.size() + kEthernetHeaderSize) {
    return ZX_ERR_INVALID_ARGS;
  }

  auto* hdr = reinterpret_cast<ethhdr*>(out.data());
  memcpy(hdr->h_dest, &dst, sizeof(dst));
  memcpy(hdr->h_source, &src, sizeof(src));
  hdr->h_proto = htons(type);
  memcpy(out.data() + kEthernetHeaderSize, body.data(), body.size());
  return ZX_OK;
}

std::vector<uint8_t> CreateEthernetFrame(MacAddr dst, MacAddr src, uint16_t type,
                                         cpp20::span<const uint8_t> body) {
  std::vector<uint8_t> out(body.size() + kEthernetHeaderSize);
  ZX_ASSERT(WriteEthernetFrame(out, dst, src, type, body) == ZX_OK);
  return out;
}

int8_t SnrDbFromSignalStrength(double signal_strength, double noise_level) {
  double snr = signal_strength - noise_level;
  if (snr > INT8_MAX) {
    return INT8_MAX;
  } else if (snr < INT8_MIN) {
    return INT8_MIN;
  }
  return static_cast<int8_t>(snr);
}

std::vector<uint8_t> CreateSsidIe(cpp20::span<const uint8_t> ssid) {
  // SSID IE format described in IEEE 802.11-2020 9.4.2.2.
  const uint8_t kSsidIeElementId = 0;
  const size_t kMaxSsidLength = 32;
  const uint8_t ssid_length = std::min(kMaxSsidLength, ssid.size());
  std::vector<uint8_t> ssid_ie;
  ssid_ie.insert(ssid_ie.end(), {kSsidIeElementId, ssid_length});
  ssid_ie.insert(ssid_ie.end(), ssid.begin(), std::next(ssid.begin(), ssid_length));
  return ssid_ie;
}

}  // namespace wlan::brcmfmac::sim_utils
