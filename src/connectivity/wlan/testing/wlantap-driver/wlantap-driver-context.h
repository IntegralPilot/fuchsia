// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_DRIVER_CONTEXT_H_
#define SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_DRIVER_CONTEXT_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>

#include <memory>

namespace wlan {

// This contains references to all the objects needed to add child nodes and serve protocols.
// Whereas in DFv1 we only needed to pass around a zx_device_t*, in DFv2 there are a few more
// classes we need, so this class encapsulates all of them for convenience.
class WlantapDriverContext {
 public:
  WlantapDriverContext(fdf::Logger* logger, std::shared_ptr<fdf::OutgoingDirectory> outgoing,
                       fidl::SyncClient<fuchsia_driver_framework::Node>* node_client)
      : logger_(logger), outgoing_(std::move(outgoing)), node_client_(node_client) {}

  const fdf::Logger* logger() const { return logger_; }
  const std::shared_ptr<fdf::OutgoingDirectory>& outgoing() const { return outgoing_; }
  const fidl::SyncClient<fuchsia_driver_framework::Node>& node_client() const {
    ZX_ASSERT(node_client_);
    return *node_client_;
  }

 private:
  const fdf::Logger* logger_;
  const std::shared_ptr<fdf::OutgoingDirectory> outgoing_;
  const fidl::SyncClient<fuchsia_driver_framework::Node>* node_client_;
};

}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_DRIVER_CONTEXT_H_
