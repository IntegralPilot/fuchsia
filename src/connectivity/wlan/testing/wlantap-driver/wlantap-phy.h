// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_PHY_H_
#define SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_PHY_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.wlan.tap/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>

#include "wlantap-mac.h"

namespace wlan {

// Serves the WlantapPhy protocol, which allows the test suite to interact with the mock driver.
// This also implements the WlantapMac::Listener interface, which sends events to the test suite
// when specific WlanSoftmac calls have been made.
class WlantapPhy : public fidl::Server<fuchsia_wlan_tap::WlantapPhy>, public WlantapMac::Listener {
 public:
  WlantapPhy(
      zx::channel user_channel, const fuchsia_wlan_tap::WlantapPhyConfig& phy_config,
      std::function<zx::result<>(WlantapPhy::ShutdownCompleter::Async)> phy_impl_shutdown_callback);

  zx_status_t SetCountry(fuchsia_wlan_tap::SetCountryArgs args);

  // WlantapPhy protocol implementation
  void Shutdown(ShutdownCompleter::Sync& completer) override;
  void Rx(RxRequest& request, RxCompleter::Sync& completer) override;
  void ReportTxResult(ReportTxResultRequest& request,
                      ReportTxResultCompleter::Sync& completer) override;
  void ScanComplete(ScanCompleteRequest& request, ScanCompleteCompleter::Sync& completer) override;

  // WlantapMac::Listener impl
  void WlantapMacStart(fdf::ClientEnd<fuchsia_wlan_softmac::WlanSoftmacIfc> ifc_client) override;
  void WlantapMacStop() override;
  void WlantapMacQueueTx(const fuchsia_wlan_softmac::WlanTxPacket& pkt) override;
  void WlantapMacSetChannel(const fuchsia_wlan_ieee80211::ChannelNumber& channel,
                            fuchsia_wlan_ieee80211::ChannelBandwidth cbw,
                            const fuchsia_wlan_ieee80211::ChannelNumber& secondary80) override;
  void WlantapMacJoinBss(const fuchsia_wlan_driver::JoinBssRequest& config) override;
  void WlantapMacStartScan(uint64_t scan_id) override;
  void WlantapMacSetKey(const fuchsia_wlan_softmac::WlanKeyConfiguration& key_config) override;

 private:
  const fuchsia_wlan_tap::WlantapPhyConfig phy_config_;
  std::string name_;
  fidl::ServerBinding<fuchsia_wlan_tap::WlantapPhy> user_binding_;
  size_t report_tx_status_count_ = 0;
  fdf::Client<fuchsia_wlan_softmac::WlanSoftmacIfc> wlan_softmac_ifc_client_;
  std::function<zx::result<>(WlantapPhy::ShutdownCompleter::Async)> phy_impl_shutdown_callback_;
};

}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_PHY_H_
