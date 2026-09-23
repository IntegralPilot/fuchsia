// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_UTILS_H_
#define SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_UTILS_H_

#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.tap/cpp/fidl.h>
#include <fuchsia/wlan/common/cpp/fidl.h>

#include <string>

namespace wlan {

std::string RoleToString(fuchsia_wlan_common::WlanMacRole role);
void ConvertTapPhyConfig(fuchsia_wlan_softmac::WlanSoftmacQueryResponse* resp,
                         const fuchsia_wlan_tap::WlantapPhyConfig& tap_phy_config);
}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_UTILS_H_
