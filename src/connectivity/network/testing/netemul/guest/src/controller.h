// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TESTING_NETEMUL_GUEST_SRC_CONTROLLER_H_
#define SRC_CONNECTIVITY_NETWORK_TESTING_NETEMUL_GUEST_SRC_CONTROLLER_H_

#include <fidl/fuchsia.netemul.guest/cpp/fidl.h>
#include <fidl/fuchsia.netemul.network/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>

#include <optional>

#include "src/connectivity/network/testing/netemul/guest/src/guest.h"

class ControllerImpl : public fidl::WireServer<fuchsia_netemul_guest::Controller>,
                       public fidl::WireServer<fuchsia_net_virtualization::Control> {
 public:
  ControllerImpl(async::Loop& loop,
                 fidl::SyncClient<fuchsia_virtualization::DebianGuestManager> guest_manager);

  // Implements `fuchsia_netemul_guest::Controller`.
  void CreateGuest(CreateGuestRequestView request, CreateGuestCompleter::Sync& completer) override;

  // Implements `fuchsia_net_virtualization::Control`.
  void CreateNetwork(CreateNetworkRequestView request,
                     CreateNetworkCompleter::Sync& completer) override;

 private:
  bool HasActiveGuest() const;
  async::Loop& loop_;
  fidl::SyncClient<fuchsia_virtualization::DebianGuestManager> guest_manager_;
  struct GuestSession {
    GuestSession(
        std::string name, async::Loop& loop, fbl::unique_fd vsock_fd,
        fidl::SyncClient<fuchsia_virtualization::DebianGuestManager>& guest_manager,
        fidl::SyncClient<fuchsia_netemul_network::Network> network,
        fidl::internal::WireCompleter<::fuchsia_netemul_guest::Controller::CreateGuest>::Async
            create_guest_completer);
    GuestImpl impl;
    bool create_network_called = {};
  };
  std::optional<GuestSession> guest_session_;
};

#endif  // SRC_CONNECTIVITY_NETWORK_TESTING_NETEMUL_GUEST_SRC_CONTROLLER_H_
