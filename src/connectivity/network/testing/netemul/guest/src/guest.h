// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TESTING_NETEMUL_GUEST_SRC_GUEST_H_
#define SRC_CONNECTIVITY_NETWORK_TESTING_NETEMUL_GUEST_SRC_GUEST_H_

#include <fidl/fuchsia.netemul.guest/cpp/fidl.h>
#include <fidl/fuchsia.netemul.network/cpp/fidl.h>
#include <fidl/fuchsia.virtualization.guest.interaction/cpp/fidl.h>
#include <fidl/fuchsia.virtualization/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>

#include <optional>
#include <string>
#include <thread>

#include "fidl/fuchsia.virtualization/cpp/markers.h"
#include "lib/fidl/cpp/wire/internal/transport_channel.h"
#include "lib/fidl/cpp/wire/status.h"
#include "src/lib/fxl/macros.h"
#include "src/virtualization/lib/guest_interaction/client/client_impl.h"
#include "src/virtualization/lib/guest_interaction/platform_interface/platform_interface.h"

class GuestImpl : public fidl::WireServer<fuchsia_netemul_guest::Guest>,
                  public fidl::WireServer<fuchsia_net_virtualization::Network> {
 public:
  GuestImpl(std::string name, async::Loop& loop, fbl::unique_fd vsock_fd,
            fidl::SyncClient<fuchsia_virtualization::DebianGuestManager>& guest_manager,
            fidl::SyncClient<fuchsia_netemul_network::Network> network,
            fidl::internal::WireCompleter<::fuchsia_netemul_guest::Controller::CreateGuest>::Async
                create_guest_completer);

  ~GuestImpl() override;

  // Returns the name provided on creation.
  const std::string& GetName() const;

  // Returns whether the guest has been shutdown.
  bool IsShutdown() const;

  // Implements `fuchsia_netemul_guest::Guest`.
  void PutFile(PutFileRequestView request, PutFileCompleter::Sync& completer) override;

  // Implements `fuchsia_netemul_guest::Guest`.
  void GetFile(GetFileRequestView request, GetFileCompleter::Sync& completer) override;

  // Implements `fuchsia_netemul_guest::Guest`.
  void ExecuteCommand(ExecuteCommandRequestView request,
                      ExecuteCommandCompleter::Sync& completer) override;

  // Implements `fuchsia_net_virtualization::Network`.
  void AddPort(AddPortRequestView request, AddPortCompleter::Sync& completer) override;

  // Implements `fuchsia_netemul_guest::Guest`.
  void Shutdown(ShutdownCompleter::Sync& completer) override;

 private:
  void OnGuestUnbound(fidl::UnbindInfo info,
                      fidl::ServerEnd<fuchsia_netemul_guest::Guest> server_end);
  void OnTeardown();
  void DoShutdown();
  void CheckGuestValidForGuestFIDLMethod(const char* method_name) const;

  std::string name_;
  async::Loop& loop_;
  ClientImpl<PosixPlatform> guest_interaction_client_;
  thrd_t guest_interaction_service_thread_;
  fidl::SyncClient<fuchsia_netemul_network::Network> network_;
  fidl::SyncClient<fuchsia_virtualization::DebianGuestManager>& guest_manager_;
  std::optional<
      fidl::internal::WireCompleter<::fuchsia_netemul_guest::Controller::CreateGuest>::Async>
      create_guest_completer_;
  bool shutdown_ = false;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(GuestImpl);
};

#endif  // SRC_CONNECTIVITY_NETWORK_TESTING_NETEMUL_GUEST_SRC_GUEST_H_
