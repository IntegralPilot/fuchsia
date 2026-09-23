// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <wlan/drivers/macaddr.h>

namespace wlan {
namespace common {

// LINT.IfChange
const MacAddr kZeroMac({0x00, 0x00, 0x00, 0x00, 0x00, 0x00});
const MacAddr kBcastMac({0xff, 0xff, 0xff, 0xff, 0xff, 0xff});
// LINT.ThenChange(//src/connectivity/wlan/lib/ieee80211/src/mac_addr.rs)

}  // namespace common
}  // namespace wlan
