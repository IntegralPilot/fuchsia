// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_MAC_TEST_UTIL_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_MAC_TEST_UTIL_H_

#include <lib/sync/cpp/completion.h>
#include <lib/zx/event.h>

#include <vector>

#include <gtest/gtest.h>

#include "mac_interface.h"

namespace network {
namespace testing {

constexpr zx_signals_t kConfigurationChangedEvent = ZX_USER_SIGNAL_0;

constexpr inline bool IsValidMacFilterMode(netdev::MacFilterMode mode) {
  return mode == netdev::MacFilterMode::kPromiscuous ||
         mode == netdev::MacFilterMode::kMulticastPromiscuous ||
         mode == netdev::MacFilterMode::kMulticastFilter;
}

class FakeMacDeviceImpl : public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  FakeMacDeviceImpl();

  zx::result<std::shared_ptr<MacAddrDeviceInterface>> CreateChild(fdf_dispatcher_t* dispatcher);
  fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> Bind(fdf_dispatcher_t* dispatcher);

  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

  zx_status_t WaitConfigurationChanged();

  // Unbinds the server endpoint from the dispatcher, closing the connection.
  void Unbind();

  const fuchsia_net::wire::MacAddress& mac() { return mac_; }

  fidl::WireTableBuilder<fuchsia_hardware_network_driver::wire::Features>& features() {
    return features_;
  }

  netdev::wire::MacFilterMode mode() {
    EXPECT_TRUE(mode_.has_value());
    return mode_.value();
  }

  std::vector<MacAddress>& addresses() { return addresses_; }

 private:
  fidl::Arena<> arena_;
  fuchsia_net::wire::MacAddress mac_ = {0x00, 0x02, 0x03, 0x04, 0x05, 0x06};
  fidl::WireTableBuilder<fuchsia_hardware_network_driver::wire::Features> features_;
  std::optional<netdev::wire::MacFilterMode> mode_;
  std::vector<MacAddress> addresses_;
  zx::event event_;
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::MacAddr>> binding_;
};
}  // namespace testing
}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_MAC_TEST_UTIL_H_
