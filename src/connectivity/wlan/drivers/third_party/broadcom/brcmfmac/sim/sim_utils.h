// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_SIM_UTILS_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_SIM_UTILS_H_

#include <lib/stdcompat/span.h>
#include <netinet/if_ether.h>
#include <zircon/types.h>

#include <span>
#include <vector>

#include <wlan/drivers/macaddr.h>

namespace wlan::brcmfmac::sim_utils {

static constexpr size_t kEthernetHeaderSize = sizeof(ethhdr);

// Writes an ethernet frame to `out` with the given parameters.
zx_status_t WriteEthernetFrame(cpp20::span<uint8_t> out, wlan::common::MacAddr dst,
                               wlan::common::MacAddr src, uint16_t type,
                               cpp20::span<const uint8_t> body);

// Returns a newly allocated vector containing an ethernet frame with the given parameters.
std::vector<uint8_t> CreateEthernetFrame(wlan::common::MacAddr dst, wlan::common::MacAddr src,
                                         uint16_t type, cpp20::span<const uint8_t> body);

// Derive SNR from RSSI & Noise. Restrict it to int8_t.
int8_t SnrDbFromSignalStrength(double signal_strength, double noise_level);

// Constructs an information element for the given SSID bytes.
// 802.11-2020 9.4.2.2 limits SSID data to 32 bytes; anything beyond that limit
// will be silently dropped by this function.
std::vector<uint8_t> CreateSsidIe(cpp20::span<const uint8_t> ssid);

}  // namespace wlan::brcmfmac::sim_utils

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_SIM_SIM_UTILS_H_
