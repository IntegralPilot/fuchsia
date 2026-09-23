// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_CTL_H_
#define SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_CTL_H_

#include <fidl/fuchsia.wlan.tap/cpp/fidl.h>

#include "wlantap-driver-context.h"
#include "wlantap-phy-impl.h"

namespace wlan {

class WlantapCtlServer : public fidl::Server<fuchsia_wlan_tap::WlantapCtl> {
 public:
  explicit WlantapCtlServer(WlantapDriverContext context)
      : driver_context_(context), logger_(driver_context_.logger()) {}

  // WlantapCtl protocol implementation
  void CreatePhy(CreatePhyRequest& request, CreatePhyCompleter::Sync& completer) override;

 private:
  zx_status_t AddWlanPhyChild(std::string_view name,
                              fidl::ServerEnd<fuchsia_driver_framework::NodeController> server);
  zx_status_t ServeWlanPhyProtocol(std::string_view name, std::shared_ptr<WlanPhyDevice> impl);

  WlantapDriverContext driver_context_;
  const fdf::Logger* logger_;
};

}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_CTL_H_
